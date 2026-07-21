// SPDX-License-Identifier: MPL-2.0

//! Cancellation-phase worker/daemon restart acceptance matrix.

use std::fs;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use temper_protocol_worker::{FailureClass, JobResult, ResultStatus};
use temper_testing::real_stack::{
    FakeModelResponse, HermeticIssueSpec, HermeticRealStack, HermeticRealStackBuilder, PausePermit,
    PausePoint, ReachedPause,
};
use temper_worker::WorkerLivenessLimits;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CancellationRestartPhase {
    Running,
    CancelRequested,
    Quiesced,
    ResultRecorded,
    UncertainDelivery,
    PostAckPreCompaction,
}

// Nextest assigns these six fault-injection worlds to the exclusive
// `native-process-fixtures` resource class because it launches each test in a
// separate process. Keep this in-binary lock as equivalent protection for
// `cargo test`, whose default scheduler runs all six in one process.
static RESTART_MATRIX_LOCK: Mutex<()> = Mutex::new(());

impl CancellationRestartPhase {
    fn pause_point(self) -> PausePoint {
        match self {
            Self::Running => PausePoint::AgentSessionStarted,
            Self::CancelRequested => PausePoint::WorkerCancelRequested,
            Self::Quiesced => PausePoint::WorkerQuiesced,
            Self::ResultRecorded => PausePoint::WorkerResultRecorded,
            Self::UncertainDelivery => PausePoint::WorkerResultDeliveryResolved,
            Self::PostAckPreCompaction => PausePoint::WorkerResultAcknowledged,
        }
    }

    fn result_was_durable(self) -> bool {
        matches!(
            self,
            Self::ResultRecorded | Self::UncertainDelivery | Self::PostAckPreCompaction
        )
    }
}

#[test]
fn restart_at_running_reuses_dirty_workspace_and_session_once() {
    run_cancellation_restart_phase(CancellationRestartPhase::Running);
}

#[test]
fn restart_at_cancel_requested_reuses_dirty_workspace_and_session_once() {
    run_cancellation_restart_phase(CancellationRestartPhase::CancelRequested);
}

#[test]
fn restart_at_quiesced_reuses_dirty_workspace_and_session_once() {
    run_cancellation_restart_phase(CancellationRestartPhase::Quiesced);
}

#[test]
fn restart_at_result_recorded_replays_one_retryable_result() {
    run_cancellation_restart_phase(CancellationRestartPhase::ResultRecorded);
}

#[test]
fn restart_with_uncertain_delivery_converges_idempotently() {
    run_cancellation_restart_phase(CancellationRestartPhase::UncertainDelivery);
}

#[test]
fn restart_post_ack_pre_compaction_replays_and_compacts_once() {
    run_cancellation_restart_phase(CancellationRestartPhase::PostAckPreCompaction);
}

fn run_cancellation_restart_phase(phase: CancellationRestartPhase) {
    let _matrix_guard = RESTART_MATRIX_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    temper_engine_io::block_on_with(move |cx, handle| async move {
        let initial_limits = if phase == CancellationRestartPhase::Running {
            WorkerLivenessLimits::default()
        } else {
            restart_watchdog_limits()
        };
        let mut stack = HermeticRealStackBuilder::new()
            .issue(HermeticIssueSpec::ready_code(
                format!("Restart cancellation at {phase:?}"),
                "Preserve dirty tracked/untracked work and the coordination session.\n\n<!-- temper:workflow\n{\"kind\":\"code\"}\n-->",
            ))
            .fake_model_response(FakeModelResponse::write_file(
                "service/RESTART_CONVERGED.md",
                "restart converged\n",
                "Restarted cancellation converged exactly once.",
            ))
            .worker_liveness_limits(initial_limits)
            // This matrix exercises watchdog cancellation and durable result
            // replay, not lease-heartbeat convergence. Frequent heartbeats
            // rewrite the same MemoryForge issue while retryable result apply
            // clears its claim, making acknowledgement timing depend on CAS
            // contention under loaded CI. Keep that independent protocol out
            // of this fault-injection window.
            .worker_heartbeat_interval(Duration::from_secs(300))
            .apply_grace(Duration::ZERO)
            .build(&handle)
            .await
            .expect("restart phase world builds");

        assert_eq!(
            stack
                .enqueue_scanned_role_work(stack.clock().now())
                .await
                .expect("initial work enqueues"),
            1
        );
        let running = stack.pause_hooks().arm(PausePoint::AgentSessionStarted);
        let phase_pause = (phase != CancellationRestartPhase::Running)
            .then(|| stack.pause_hooks().arm(phase.pause_point()));
        let first_capacity_release = stack.pause_hooks().arm(PausePoint::WorkerCapacityReleased);
        stack.start_worker(&handle);

        let running = await_pause(&cx, running, "initial agent session").await;
        write_restart_dirty_state(&stack);
        let phase_reached = match phase_pause {
            Some(pause) => Some(await_pause(&cx, pause, "cancellation phase").await),
            None => None,
        };
        assert_eq!(
            stack.pause_hooks().reached_count(phase.pause_point()),
            1,
            "phase hook must be reached exactly once before restart"
        );

        // Both process-local components are replaced while the worker-owned
        // effect is paused at the named phase. Dropping the old hook future is
        // the transport/process-loss fault; no old completion may cross into
        // the replacement worker.
        stack.crash_worker().await;
        stack.replace_daemon(&handle).await;
        drop(phase_reached);
        drop(running);

        let transient = if phase.result_was_durable() {
            assert_eq!(
                stack.pending_result_count().expect("durable outbox count"),
                1,
                "result-recorded and delivery phases must survive in the outbox"
            );
            // An uncertain response may now be the specified retryable 503
            // while an ownership check is unresolved. Only a matching release
            // acknowledgement proves pre-crash convergence.
            let delivery_already_converged =
                phase == CancellationRestartPhase::PostAckPreCompaction;
            if delivery_already_converged {
                assert!(
                    stack.open_recovery_barrier().await.is_empty(),
                    "accepted pre-crash delivery must leave no startup orphan"
                );
            }
            let replay_converged = stack
                .pause_hooks()
                .arm(PausePoint::ResultApplicationCompleted);
            stack.start_worker(&handle);
            let replay_converged =
                await_result_convergence(&cx, replay_converged, "replayed result application")
                    .await;
            replay_converged.release();
            let transient = await_retryable_result(&mut stack, &cx).await;
            wait_for_outbox_count(&stack, &cx, 0).await;
            if !delivery_already_converged {
                assert!(
                    stack.open_recovery_barrier().await.is_empty(),
                    "exact replay must reattach the staged claim before orphan rollback"
                );
            }
            transient
        } else {
            assert_eq!(
                stack.pending_result_count().expect("empty outbox count"),
                0,
                "pre-record restart must not invent a terminal payload"
            );
            assert_eq!(
                stack.open_recovery_barrier().await.len(),
                1,
                "startup must roll back the interrupted durable claim"
            );
            if phase == CancellationRestartPhase::Running {
                stack.set_worker_liveness_limits(restart_watchdog_limits());
            }
            let retry_running = stack.pause_hooks().arm(PausePoint::AgentSessionStarted);
            // Observe the daemon's successful application boundary instead of
            // racing the worker's follow-up acknowledgement task. Retryable
            // replies do not reach this hook, so it still proves that the
            // durable claim converged before outbox compaction is checked.
            let retry_converged = stack
                .pause_hooks()
                .arm(PausePoint::ResultApplicationCompleted);
            assert_eq!(
                stack
                    .enqueue_scanned_role_work(stack.clock().now())
                    .await
                    .expect("interrupted work re-enqueues"),
                1
            );
            stack.start_worker(&handle);
            let retry_running = await_pause(&cx, retry_running, "timeout retry session").await;
            assert_restart_dirty_state(&stack);
            let retry_converged =
                await_result_convergence(&cx, retry_converged, "retryable result application")
                    .await;
            retry_converged.release();
            let transient = await_retryable_result(&mut stack, &cx).await;
            drop(retry_running);
            transient
        };
        assert_eq!(transient.status, ResultStatus::Failure);
        assert_eq!(
            transient.failure.as_ref().map(|failure| failure.class),
            Some(FailureClass::Transient)
        );

        drop(first_capacity_release);
        wait_for_checkpoint_count(&stack, &cx, PausePoint::WorkerCapacityReleased, 1).await;
        assert_eq!(
            stack
                .pause_hooks()
                .reached_count(PausePoint::WorkerCapacityReleased),
            1,
            "the retryable terminal record releases capacity once"
        );
        assert_one_retryable_publication(&stack);
        wait_for_outbox_count(&stack, &cx, 0).await;

        // Reset both process-local components before the successful retry. The
        // timed-out worker can leave already-queued protocol completions in the
        // current daemon after its durable result is acknowledged; those belong
        // to the crashed incarnation and must not race the next dispatch. The
        // recovery barrier also proves the accepted retryable result left no
        // durable assignment behind.
        stack.crash_worker().await;
        stack.set_worker_liveness_limits(WorkerLivenessLimits::default());
        stack.replace_daemon(&handle).await;
        assert!(
            stack.open_recovery_barrier().await.is_empty(),
            "accepted retryable result must leave no assignment to recover"
        );

        // The next attempt must attach to the same coordination-key checkout
        // and session, then commit both interrupted tracked and untracked work.
        let resumed_session = stack.pause_hooks().arm(PausePoint::AgentSessionStarted);
        assert_eq!(
            stack
                .enqueue_scanned_role_work(stack.clock().now())
                .await
                .expect("post-timeout retry enqueues"),
            1
        );
        stack.start_worker(&handle);
        let resumed_session = await_pause(&cx, resumed_session, "successful retry session").await;
        assert_restart_dirty_state(&stack);
        resumed_session.release();
        let success = await_success_result(&mut stack, &cx).await;
        assert_eq!(success.repos.len(), 1);
        let branch = &success.repos[0].branch.name;
        assert_eq!(
            stack
                .origin_file(stack.primary_repo_path(), branch, "README.md")
                .expect("tracked dirty file pushed"),
            "interrupted tracked cancellation edit\n"
        );
        assert_eq!(
            stack
                .origin_file(
                    stack.primary_repo_path(),
                    branch,
                    "UNTRACKED-CANCELLATION.txt"
                )
                .expect("untracked dirty file pushed"),
            "interrupted untracked cancellation edit\n"
        );
        assert_eq!(
            stack
                .wait_for_pull_request_count(&cx, 1, Duration::from_secs(10))
                .await
                .expect("one converged implementation PR")
                .len(),
            1
        );
        wait_for_checkpoint_count(&stack, &cx, PausePoint::WorkerCapacityReleased, 2).await;
        assert_eq!(
            stack
                .pause_hooks()
                .reached_count(PausePoint::WorkerCapacityReleased),
            2,
            "retryable and successful attempts each release exactly one permit"
        );
        let publications = stack.published_results();
        assert_eq!(
            publications
                .iter()
                .filter(|result| {
                    result.failure.as_ref().map(|failure| failure.class)
                        == Some(FailureClass::Transient)
                })
                .count(),
            1,
            "replay must not publish another retryable attempt"
        );
        assert_eq!(
            publications
                .iter()
                .filter(|result| result.status == ResultStatus::Success)
                .count(),
            1,
            "late pre-restart work must not publish a stale success"
        );
        assert_eq!(
            publications.len(),
            2,
            "only the retryable cancellation and its successful retry may publish"
        );
        wait_for_outbox_count(&stack, &cx, 0).await;
        assert_eq!(stack.pending_result_count().expect("compacted outbox"), 0);
        assert_eq!(stack.persisted_session_count().expect("session count"), 1);
        stack.crash_worker().await;
    });
}

fn restart_watchdog_limits() -> WorkerLivenessLimits {
    WorkerLivenessLimits {
        // Real helper-backed containment adds bounded process startup/join work
        // to checkout preparation. A cold, loaded CI host can take more than
        // two seconds to reach AgentSessionStarted, so leave enough startup
        // headroom while keeping the no-progress cancellation test bounded.
        max_no_progress: Duration::from_secs(5),
        graceful_cancellation_grace: Duration::from_secs(1),
        forced_termination_grace: Duration::from_secs(1),
        ..WorkerLivenessLimits::default()
    }
}

fn write_restart_dirty_state(stack: &HermeticRealStack) {
    let checkout = stack
        .workspace_checkout(stack.primary_repo_path())
        .expect("prepared restart checkout");
    fs::write(
        checkout.join("README.md"),
        "interrupted tracked cancellation edit\n",
    )
    .expect("write tracked cancellation edit");
    fs::write(
        checkout.join("UNTRACKED-CANCELLATION.txt"),
        "interrupted untracked cancellation edit\n",
    )
    .expect("write untracked cancellation edit");
}

fn assert_restart_dirty_state(stack: &HermeticRealStack) {
    let checkout = stack
        .workspace_checkout(stack.primary_repo_path())
        .expect("reused restart checkout");
    assert_eq!(
        fs::read_to_string(checkout.join("README.md")).expect("tracked edit retained"),
        "interrupted tracked cancellation edit\n"
    );
    assert_eq!(
        fs::read_to_string(checkout.join("UNTRACKED-CANCELLATION.txt"))
            .expect("untracked edit retained"),
        "interrupted untracked cancellation edit\n"
    );
    assert_eq!(stack.persisted_session_count().expect("session reuse"), 1);
}

async fn await_pause(cx: &skein::cx::Cx, pause: PausePermit, description: &str) -> ReachedPause {
    skein::time::timeout(
        temper_engine_io::runtime::timer_now(cx),
        Duration::from_secs(20),
        Box::pin(pause.arrived()),
    )
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {description}"))
}

async fn await_result_convergence(
    cx: &skein::cx::Cx,
    pause: PausePermit,
    description: &str,
) -> ReachedPause {
    // Result application may return retryable failures before durable claim
    // convergence succeeds. Keep this bound beyond the 32-second replay slot
    // while retaining an actionable failure for a genuinely stuck run.
    skein::time::timeout(
        temper_engine_io::runtime::timer_now(cx),
        Duration::from_secs(90),
        Box::pin(pause.arrived()),
    )
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {description}"))
}

async fn await_retryable_result(stack: &mut HermeticRealStack, cx: &skein::cx::Cx) -> JobResult {
    loop {
        let result = stack
            .await_worker_result(cx, Duration::from_secs(20))
            .await
            .expect("retryable worker result");
        if result.failure.as_ref().map(|failure| failure.class) == Some(FailureClass::Transient) {
            return result;
        }
    }
}

async fn await_success_result(stack: &mut HermeticRealStack, cx: &skein::cx::Cx) -> JobResult {
    loop {
        let result = stack
            .await_worker_result(cx, Duration::from_secs(20))
            .await
            .expect("successful replacement result");
        if result.status == ResultStatus::Success {
            return result;
        }
    }
}

fn assert_one_retryable_publication(stack: &HermeticRealStack) {
    let publications = stack.published_results();
    assert_eq!(publications.len(), 1, "only one attempt may publish so far");
    assert_eq!(
        publications[0]
            .failure
            .as_ref()
            .map(|failure| failure.class),
        Some(FailureClass::Transient)
    );
}

async fn wait_for_outbox_count(stack: &HermeticRealStack, cx: &skein::cx::Cx, expected: usize) {
    // Acknowledgement is synchronized explicitly above; this shorter bound now
    // verifies only the asynchronous filesystem compaction that follows it.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let count = stack.pending_result_count().expect("read result outbox");
        if count == expected {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {expected} outbox entries, saw {count}"
        );
        temper_engine_io::runtime::sleep_for(cx, Duration::from_millis(1)).await;
    }
}

async fn wait_for_checkpoint_count(
    stack: &HermeticRealStack,
    cx: &skein::cx::Cx,
    point: PausePoint,
    expected: usize,
) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let count = stack.pause_hooks().reached_count(point);
        if count >= expected {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {point:?} count {expected}, saw {count}"
        );
        temper_engine_io::runtime::sleep_for(cx, Duration::from_millis(1)).await;
    }
}
