//! One absolute signal-to-exit deadline and the out-of-band standalone watchdog.

use std::collections::BTreeSet;
use std::future::Future;
use std::task::Poll;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use skein::cx::Cx;
use temper_engine::Daemon;
use temper_engine_io::http::EngineHttpServer;
use temper_engine_service::TraceRetentionTask;
use temper_protocol_worker::{ShutdownBlocker, ShutdownBlockerKind, ShutdownEscalationStage};
use temper_worker::{WorkerComponentHandle, WorkerEmergencyShutdownHandle, WorkerShutdownReport};

mod watchdog;
use watchdog::StandaloneShutdownCoordinator;

/// One monotonic deadline measured from the instant a termination signal is
/// actually observed. Cooperative work stops at `emergency_kill_at`; the final
/// allowance is reserved exclusively for out-of-band KILL and process death.
#[derive(Clone, Copy, Debug)]
pub(super) struct StandaloneShutdownDeadline {
    signal_received_at: Instant,
    emergency_kill_at: Instant,
    absolute_deadline: Instant,
}

impl StandaloneShutdownDeadline {
    pub(super) fn from_signal(
        signal_received_at: Instant,
        budget: Duration,
    ) -> Result<Self, String> {
        let absolute_deadline = signal_received_at.checked_add(budget).ok_or_else(|| {
            "standalone shutdown deadline exceeds the monotonic clock".to_string()
        })?;
        let emergency_kill_at = absolute_deadline
            .checked_sub(temper_config::STANDALONE_FINAL_KILL_ALLOWANCE)
            .ok_or_else(|| {
                "standalone shutdown budget is shorter than final KILL allowance".to_string()
            })?;
        Ok(Self {
            signal_received_at,
            emergency_kill_at,
            absolute_deadline,
        })
    }

    pub(super) fn signal_received_at(self) -> Instant {
        self.signal_received_at
    }

    pub(super) fn emergency_kill_at(self) -> Instant {
        self.emergency_kill_at
    }

    pub(super) fn absolute_deadline(self) -> Instant {
        self.absolute_deadline
    }

    pub(super) fn remaining_before_emergency(self) -> Duration {
        self.emergency_kill_at
            .saturating_duration_since(Instant::now())
    }

    pub(super) fn remaining_before_termination(self) -> Duration {
        self.absolute_deadline
            .saturating_duration_since(Instant::now())
    }

    pub(super) fn http_drain_allowance(self) -> Duration {
        temper_config::STANDALONE_HTTP_DRAIN_ALLOWANCE.min(self.remaining_before_emergency())
    }

    /// Runs one orchestration wait against the same absolute cooperative
    /// deadline. The future is polled before the timer so completion exactly at
    /// the boundary is accepted without manufacturing another timeout window.
    pub(super) async fn wait<F>(self, cx: &Cx, future: F) -> Option<F::Output>
    where
        F: Future,
    {
        let mut future = std::pin::pin!(future);
        let mut timer = std::pin::pin!(temper_engine_io::runtime::sleep_for(
            cx,
            self.remaining_before_emergency(),
        ));
        std::future::poll_fn(|task_cx| {
            if let Poll::Ready(output) = future.as_mut().poll(task_cx) {
                return Poll::Ready(Some(output));
            }
            if timer.as_mut().poll(task_cx).is_ready() {
                return Poll::Ready(None);
            }
            Poll::Pending
        })
        .await
    }

    pub(super) fn blocker(
        self,
        kind: ShutdownBlockerKind,
        owner_scope: &str,
        owner_name: &str,
    ) -> ShutdownBlocker {
        let first_seen_millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
            .unwrap_or(0);
        ShutdownBlocker::new(
            kind,
            ShutdownEscalationStage::EmergencyKill,
            owner_scope,
            owner_name,
        )
        .with_timing(
            first_seen_millis,
            0,
            u64::try_from(self.remaining_before_termination().as_millis()).unwrap_or(u64::MAX),
        )
    }
}

/// Runs the standalone-only shutdown ordering. Every async join races the same
/// cooperative deadline; the independently armed terminator owns the final
/// allowance even when the single-threaded runtime stops polling this future.
pub(super) async fn orchestrate(
    cx: &Cx,
    signal_received_at: Instant,
    budget: Duration,
    daemon: &Daemon,
    worker: &mut WorkerComponentHandle,
    server: &mut EngineHttpServer,
    trace_retention: &mut Option<TraceRetentionTask>,
) -> Result<(), String> {
    let deadline = StandaloneShutdownDeadline::from_signal(signal_received_at, budget)?;
    let emergency = worker.emergency_shutdown_handle();
    let coordinator = match StandaloneShutdownCoordinator::arm(deadline, emergency.clone()) {
        Ok(coordinator) => coordinator,
        Err(error) => {
            tracing::error!(
                target: "temper::standalone",
                service = "standalone",
                event = "standalone.shutdown.watchdog_arm_failed",
                %error,
                "standalone cannot guarantee its absolute shutdown deadline"
            );
            emergency.request_emergency_kill();
            std::process::abort();
        }
    };
    tracing::debug!(
        target: "temper::standalone",
        service = "standalone",
        event = "standalone.shutdown.deadline_armed",
        signal_to_arm_millis = u64::try_from(deadline.signal_received_at().elapsed().as_millis())
            .unwrap_or(u64::MAX),
        budget_millis = u64::try_from(budget.as_millis()).unwrap_or(u64::MAX),
        "standalone absolute shutdown deadline armed"
    );

    // Fence daemon result, context, claim, and Forge-apply admission before any
    // worker can relinquish local ownership.
    let daemon_shutdown = match deadline.wait(cx, daemon.begin_shutdown()).await {
        Some(handle) => handle,
        None => {
            bounded_crash_handoff(
                deadline,
                vec![deadline.blocker(
                    ShutdownBlockerKind::ComponentTask,
                    "daemon",
                    "admission_fence",
                )],
                &emergency,
            )
            .await
        }
    };
    let daemon_blockers = daemon_shutdown.report().blockers.clone();

    // Start HTTP drain only after worker registry intake and every current
    // AttemptFence are closed. Worker cooperative, forced, and hard escalation
    // consume this same absolute deadline.
    let worker_report = worker
        .shutdown_bounded_after_fence(deadline.emergency_kill_at(), || {
            server.begin_drain(deadline.http_drain_allowance());
        })
        .await;
    let Some(joined_assignments) = exact_joined_assignments(&worker_report) else {
        bounded_crash_handoff(
            deadline,
            worker_report.unresolved_blockers.clone(),
            &emergency,
        )
        .await;
    };

    if deadline.wait(cx, daemon_shutdown.wait_for_join()).await != Some(true) {
        let blockers = if daemon_blockers.is_empty() {
            vec![deadline.blocker(
                ShutdownBlockerKind::ComponentTask,
                "daemon",
                "admitted_operations",
            )]
        } else {
            daemon_blockers
        };
        bounded_crash_handoff(deadline, blockers, &emergency).await;
    }

    if let Some(retention) = trace_retention.as_mut() {
        retention.begin_stop();
        if deadline.wait(cx, retention.wait_stopped()).await.is_none() {
            bounded_crash_handoff(
                deadline,
                vec![deadline.blocker(
                    ShutdownBlockerKind::ComponentTask,
                    "trace_retention",
                    "retention_task",
                )],
                &emergency,
            )
            .await;
        }
    }

    if deadline
        .wait(
            cx,
            daemon.release_joined_assignments_for_shutdown(&joined_assignments),
        )
        .await
        .is_none()
    {
        bounded_crash_handoff(
            deadline,
            vec![deadline.blocker(
                ShutdownBlockerKind::ComponentTask,
                "daemon",
                "joined_assignment_release",
            )],
            &emergency,
        )
        .await;
    }

    if deadline.wait(cx, server.wait_for_drain()).await.is_none() {
        bounded_crash_handoff(
            deadline,
            vec![deadline.blocker(
                ShutdownBlockerKind::ComponentTask,
                "http",
                "drain_completion",
            )],
            &emergency,
        )
        .await;
    }

    temper_worker::StandaloneShutdownSummaryEvent::new(
        temper_worker::StandaloneShutdownDisposition::GracefulExit,
        std::iter::empty(),
    )
    .emit();
    coordinator.disarm()
}

fn exact_joined_assignments(
    report: &WorkerShutdownReport,
) -> Option<BTreeSet<temper_engine::AssignmentAttemptIdentity>> {
    report.unresolved_blockers.is_empty().then(|| {
        report
            .joined_attempts
            .iter()
            .map(|attempt| {
                temper_engine::AssignmentAttemptIdentity::new(
                    attempt.worker_id.clone(),
                    attempt.job_id.clone(),
                    Some(attempt.attempt_id.clone()),
                )
            })
            .collect()
    })
}

async fn bounded_crash_handoff(
    deadline: StandaloneShutdownDeadline,
    mut blockers: Vec<ShutdownBlocker>,
    emergency: &WorkerEmergencyShutdownHandle,
) -> ! {
    if blockers.is_empty() {
        blockers.push(deadline.blocker(
            ShutdownBlockerKind::RegistryState,
            "standalone",
            "unknown_shutdown_owner",
        ));
    }
    let remaining =
        u64::try_from(deadline.remaining_before_termination().as_millis()).unwrap_or(u64::MAX);
    for blocker in &mut blockers {
        blocker.escalation_stage = ShutdownEscalationStage::EmergencyKill;
        blocker.deadline_remaining_millis = remaining;
        temper_worker::StandaloneShutdownBlockerEvent::new(blocker.clone()).emit();
    }
    temper_worker::StandaloneShutdownSummaryEvent::new(
        temper_worker::StandaloneShutdownDisposition::BoundedCrashHandoff,
        blockers,
    )
    .emit();

    // Even if this synchronous request blocks, the independent absolute-
    // deadline thread still terminates without dropping process owners.
    emergency.request_emergency_kill();
    std::future::pending::<()>().await;
    unreachable!("the standalone absolute-deadline watchdog terminates the process")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_release_set_requires_a_proven_worker_report() {
        let joined = temper_worker::WorkerAttemptIdentity {
            worker_id: "worker-a".to_string(),
            job_id: "job-a".to_string(),
            attempt_id: "attempt-a".to_string(),
            generation: 7,
        };
        let mut report = WorkerShutdownReport {
            joined_attempts: vec![joined],
            unresolved_blockers: Vec::new(),
        };
        let release = exact_joined_assignments(&report).expect("proven report releases");
        assert_eq!(release.len(), 1);
        assert_eq!(
            release.iter().next().unwrap().attempt_id.as_deref(),
            Some("attempt-a")
        );

        report.unresolved_blockers.push(ShutdownBlocker::new(
            ShutdownBlockerKind::TerminalTraceAck,
            ShutdownEscalationStage::HardKill,
            "trace",
            "terminal_ack",
        ));
        assert!(
            exact_joined_assignments(&report).is_none(),
            "partial worker proof must preserve every assignment"
        );
    }

    #[test]
    fn every_shutdown_stage_consumes_one_absolute_deadline() {
        temper_engine_io::block_on_with(move |cx, _handle| async move {
            let started = Instant::now();
            let deadline = StandaloneShutdownDeadline {
                signal_received_at: started,
                emergency_kill_at: started + Duration::from_millis(700),
                absolute_deadline: started + Duration::from_millis(750),
            };

            // Worker join, admitted daemon result handling, trace
            // acknowledgement/retention, exact release, and an initial drain
            // wait all spend the same budget rather than refreshing it.
            for stage in [
                "worker_join",
                "daemon_result_join",
                "trace_acknowledgement",
                "joined_assignment_release",
                "http_drain_progress",
            ] {
                assert!(
                    deadline
                        .wait(
                            &cx,
                            temper_engine_io::runtime::sleep_for(&cx, Duration::from_millis(100),),
                        )
                        .await
                        .is_some(),
                    "{stage} should finish inside the shared deadline"
                );
            }

            assert!(
                deadline
                    .wait(&cx, std::future::pending::<()>())
                    .await
                    .is_none(),
                "the final drain blocker must stop at the original deadline"
            );
            assert!(
                started.elapsed() < Duration::from_millis(1_100),
                "stage waits composed fresh timeout windows"
            );
        });
    }
}
