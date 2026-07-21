// SPDX-License-Identifier: MPL-2.0

use crate::support::*;

use std::sync::atomic::{AtomicUsize, Ordering};

use temper_protocol_worker::{
    Capacity, ContextOutcome, FetchContext, ForgeContextErrorCode, ForgeContextOperation,
    ForgeGetItemOperation, Heartbeat, HeartbeatState, JobHeartbeat, Register,
};

struct OwnershipLostApplier {
    application_calls: AtomicUsize,
}

#[async_trait::async_trait]
impl temper_engine::ResultApplier for OwnershipLostApplier {
    async fn heartbeat(
        &self,
        _job: InFlightJob,
        _context: temper_engine::ClaimContext,
    ) -> temper_engine::RecoveredHeartbeatOutcome {
        temper_engine::RecoveredHeartbeatOutcome::OwnershipLost {
            reason: temper_engine::RecoveredOwnershipLossReason::AssignmentAbsent,
        }
    }

    async fn apply(&self, _job: InFlightJob, _result: JobResult) -> temper_engine::ApplyOutcome {
        self.application_calls.fetch_add(1, Ordering::SeqCst);
        temper_engine::ApplyOutcome::Applied
    }

    async fn apply_recovered(
        &self,
        _job: InFlightJob,
        _result: JobResult,
        _context: temper_engine::ClaimContext,
    ) -> temper_engine::ApplyOutcome {
        self.application_calls.fetch_add(1, Ordering::SeqCst);
        temper_engine::ApplyOutcome::Applied
    }
}

struct TransientHeartbeatApplier {
    heartbeat_calls: AtomicUsize,
    application_calls: AtomicUsize,
}

#[async_trait::async_trait]
impl temper_engine::ResultApplier for TransientHeartbeatApplier {
    async fn heartbeat(
        &self,
        _job: InFlightJob,
        _context: temper_engine::ClaimContext,
    ) -> temper_engine::RecoveredHeartbeatOutcome {
        self.heartbeat_calls.fetch_add(1, Ordering::SeqCst);
        temper_engine::RecoveredHeartbeatOutcome::TransientlyUnavailable {
            reason: "Forge temporarily unavailable".to_string(),
        }
    }

    async fn apply(&self, _job: InFlightJob, _result: JobResult) -> temper_engine::ApplyOutcome {
        panic!("a recovered assignment must use recovered result application")
    }

    async fn apply_recovered(
        &self,
        _job: InFlightJob,
        _result: JobResult,
        _context: temper_engine::ClaimContext,
    ) -> temper_engine::ApplyOutcome {
        self.application_calls.fetch_add(1, Ordering::SeqCst);
        temper_engine::ApplyOutcome::Applied
    }
}

struct StaleResultApplier;

#[async_trait::async_trait]
impl temper_engine::ResultApplier for StaleResultApplier {
    async fn apply(&self, _job: InFlightJob, _result: JobResult) -> temper_engine::ApplyOutcome {
        temper_engine::ApplyOutcome::Stale
    }
}

struct GatedOwnershipApplier {
    entered: temper_engine_io::CqSender<()>,
    outcome: StdMutex<
        Option<temper_engine_io::OneshotReceiver<temper_engine::RecoveredHeartbeatOutcome>>,
    >,
    application_calls: AtomicUsize,
}

#[async_trait::async_trait]
impl temper_engine::ResultApplier for GatedOwnershipApplier {
    async fn heartbeat(
        &self,
        _job: InFlightJob,
        _context: temper_engine::ClaimContext,
    ) -> temper_engine::RecoveredHeartbeatOutcome {
        let outcome = self
            .outcome
            .lock()
            .expect("ownership outcome gate lock")
            .take()
            .expect("one ownership check is expected");
        let _ = self.entered.send(());
        outcome
            .recv()
            .await
            .expect("test supplies ownership outcome")
    }

    async fn apply(&self, _job: InFlightJob, _result: JobResult) -> temper_engine::ApplyOutcome {
        self.application_calls.fetch_add(1, Ordering::SeqCst);
        temper_engine::ApplyOutcome::Applied
    }

    async fn apply_recovered(
        &self,
        _job: InFlightJob,
        _result: JobResult,
        _context: temper_engine::ClaimContext,
    ) -> temper_engine::ApplyOutcome {
        self.application_calls.fetch_add(1, Ordering::SeqCst);
        temper_engine::ApplyOutcome::Applied
    }
}

fn register_with_capacity(worker_id: &str, capacity: u32) -> WorkerProtocolMessage {
    WorkerProtocolMessage::Register(Register {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: worker_id.to_string(),
        capabilities: vec![Capability {
            role: "engineer".to_string(),
            repo: "ai/temper".to_string(),
        }],
        capacity: Capacity {
            max_concurrent_jobs: capacity,
        },
        worker_pool: None,
        labels: None,
    })
}

fn recovered_job(job_id: &str, attempt_id: &str) -> temper_worker_registry::RecoveredJob {
    temper_worker_registry::RecoveredJob {
        job_id: job_id.to_string(),
        attempt_id: Some(attempt_id.to_string()),
        worker_id: "worker-fenced".to_string(),
        role: "engineer".to_string(),
        repo: "ai/temper".to_string(),
        artifact: artifact(),
        job_payload: json!({"prompt":"resume"}),
    }
}

fn recovered_heartbeat(
    jobs: impl IntoIterator<Item = (&'static str, &'static str)>,
) -> WorkerProtocolMessage {
    WorkerProtocolMessage::Heartbeat(Heartbeat {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: "worker-fenced".to_string(),
        jobs: jobs
            .into_iter()
            .map(|(job_id, attempt_id)| JobHeartbeat {
                job_id: job_id.to_string(),
                attempt_id: Some(attempt_id.to_string()),
                state: HeartbeatState::Running,
                message: "running".to_string(),
                liveness: None,
            })
            .collect(),
        free_capacity: Some(0),
        worker_pool: None,
        max_concurrent_jobs: None,
        capabilities: Vec::new(),
    })
}

fn result_for_attempt(job_id: &str, attempt_id: &str, summary: &str) -> WorkerProtocolMessage {
    let mut result = success_result("worker-fenced", job_id);
    result.attempt_id = Some(attempt_id.to_string());
    result.summary = Some(summary.to_string());
    WorkerProtocolMessage::Result(result)
}

async fn spawn_with_role_limit(
    handle: &skein::runtime::RuntimeHandle,
    applier: Arc<dyn temper_engine::ResultApplier>,
    limit: u32,
) -> (temper_engine::Daemon, String) {
    spawn_daemon(
        handle,
        temper_engine::Daemon::with_applier_and_role_limits(
            Arc::new(handle.clone()),
            applier,
            BTreeMap::from([("engineer".to_string(), limit)]),
        ),
    )
    .await
}

#[test]
fn ownership_loss_fences_context_and_holds_capacity_until_terminal_result() {
    temper_engine_io::block_on_with(move |_cx, handle| async move {
        let applier = Arc::new(OwnershipLostApplier {
            application_calls: AtomicUsize::new(0),
        });
        let (daemon, url) = spawn_with_role_limit(&handle, applier.clone(), 1).await;
        daemon
            .stage_recovered_job(
                recovered_job("job-fenced", "attempt-fenced"),
                "daemon-boot-original",
            )
            .await
            .expect("recovered assignment stages");

        let client = temper_engine_io::http::JsonClient::new();
        assert_eq!(
            post(&client, &url, &register_with_capacity("worker-fenced", 1))
                .await
                .status,
            204
        );
        let heartbeat = recovered_heartbeat([("job-fenced", "attempt-fenced")]);
        let first_cancellation = post_json(&client, &url, &heartbeat).await;
        let repeated_cancellation = post_json(&client, &url, &heartbeat).await;
        assert_eq!(first_cancellation, repeated_cancellation);
        let WorkerProtocolMessage::CancelAttempts(cancellation) = first_cancellation else {
            panic!("expected ownership-loss cancellation")
        };
        assert!(cancellation.cancellations()[0].matches_exact(
            "worker-fenced",
            "job-fenced",
            Some("attempt-fenced")
        ));

        let context = WorkerProtocolMessage::FetchContext(FetchContext {
            protocol_version: WORKER_PROTOCOL_VERSION,
            worker_id: "worker-fenced".to_string(),
            job_id: "job-fenced".to_string(),
            attempt_id: Some("attempt-fenced".to_string()),
            operation: ForgeContextOperation::ForgeGetItem(ForgeGetItemOperation {
                repo: "ai/temper".to_string(),
                number: 603,
                artifact_type: None,
                include_comments: false,
            }),
        });
        let WorkerProtocolMessage::ContextResponse(context) =
            post_json(&client, &url, &context).await
        else {
            panic!("expected fenced context response")
        };
        assert_eq!(
            context.outcome,
            ContextOutcome::Error {
                code: ForgeContextErrorCode::NotAuthorized
            }
        );

        enqueue_standard_job(&daemon, "job-after-fence").await;
        assert_poll_timeout(post_json(&client, &url, &poll_with_wait("worker-fenced", 1)).await);

        let first_release = post_json(
            &client,
            &url,
            &result_for_attempt("job-fenced", "attempt-fenced", "cleanup durable"),
        )
        .await;
        let conflicting_release = post_json(
            &client,
            &url,
            &result_for_attempt("job-fenced", "attempt-fenced", "different stale payload"),
        )
        .await;
        assert_eq!(first_release, conflicting_release);
        let WorkerProtocolMessage::Release(release) = first_release else {
            panic!("expected reclaimed stale release")
        };
        assert_eq!(release.disposition, ReleaseDisposition::Reclaimed);
        assert_eq!(applier.application_calls.load(Ordering::SeqCst), 0);

        assert_assigned(
            post_json(&client, &url, &poll("worker-fenced")).await,
            "job-after-fence",
        );
    })
}

#[test]
fn unresolved_ownership_check_makes_result_retryable_before_fencing_response() {
    temper_engine_io::block_on_with(move |_cx, handle| async move {
        let (entered_tx, mut entered_rx) = temper_engine_io::channel();
        let (outcome_tx, outcome_rx) = temper_engine_io::oneshot();
        let applier = Arc::new(GatedOwnershipApplier {
            entered: entered_tx,
            outcome: StdMutex::new(Some(outcome_rx)),
            application_calls: AtomicUsize::new(0),
        });
        let (daemon, url) = spawn_with_applier(&handle, applier.clone()).await;
        daemon
            .stage_recovered_job(
                recovered_job("job-checking", "attempt-checking"),
                "daemon-boot-original",
            )
            .await
            .expect("recovered assignment stages");

        let client = temper_engine_io::http::JsonClient::new();
        assert_eq!(
            post(&client, &url, &register_with_capacity("worker-fenced", 1))
                .await
                .status,
            204
        );
        let heartbeat_reply = post_json_background(
            &handle,
            &url,
            recovered_heartbeat([("job-checking", "attempt-checking")]),
        );
        entered_rx.recv().await.expect("ownership check starts");

        let result = result_for_attempt("job-checking", "attempt-checking", "cleanup durable");
        assert_eq!(post(&client, &url, &result).await.status, 503);
        assert_eq!(applier.application_calls.load(Ordering::SeqCst), 0);

        outcome_tx.send(temper_engine::RecoveredHeartbeatOutcome::OwnershipLost {
            reason: temper_engine::RecoveredOwnershipLossReason::LeaseAbsent,
        });
        let cancellation = heartbeat_reply
            .recv()
            .await
            .expect("heartbeat receives machine-applied outcome");
        assert!(matches!(
            cancellation,
            WorkerProtocolMessage::CancelAttempts(_)
        ));
        let WorkerProtocolMessage::Release(release) = post_json(&client, &url, &result).await
        else {
            panic!("expected stale release after ownership check")
        };
        assert_eq!(release.disposition, ReleaseDisposition::Reclaimed);
        assert_eq!(applier.application_calls.load(Ordering::SeqCst), 0);
    })
}

#[test]
fn transient_ownership_outcome_retains_assignment_without_cancellation() {
    temper_engine_io::block_on_with(move |_cx, handle| async move {
        let applier = Arc::new(TransientHeartbeatApplier {
            heartbeat_calls: AtomicUsize::new(0),
            application_calls: AtomicUsize::new(0),
        });
        let (daemon, url) = spawn_with_applier(&handle, applier.clone()).await;
        daemon
            .stage_recovered_job(
                recovered_job("job-transient", "attempt-transient"),
                "daemon-boot-original",
            )
            .await
            .expect("recovered assignment stages");

        let client = temper_engine_io::http::JsonClient::new();
        assert_eq!(
            post(&client, &url, &register_with_capacity("worker-fenced", 1))
                .await
                .status,
            204
        );
        assert_eq!(
            post(
                &client,
                &url,
                &recovered_heartbeat([("job-transient", "attempt-transient")]),
            )
            .await
            .status,
            204,
            "transient ownership lookup must not cancel the attempt"
        );
        assert_eq!(applier.heartbeat_calls.load(Ordering::SeqCst), 1);

        assert_release(
            post_json(
                &client,
                &url,
                &result_for_attempt("job-transient", "attempt-transient", "terminal result"),
            )
            .await,
            "worker-fenced",
            "job-transient",
        );
        assert_eq!(applier.application_calls.load(Ordering::SeqCst), 1);
    })
}

#[test]
fn stale_application_is_reclaimed_and_conflicting_replay_remains_compactable() {
    temper_engine_io::block_on_with(move |_cx, handle| async move {
        let (daemon, url) = spawn_with_applier(&handle, Arc::new(StaleResultApplier)).await;
        let client = temper_engine_io::http::JsonClient::new();
        assert_eq!(post(&client, &url, &register("worker-a")).await.status, 204);
        enqueue_standard_job(&daemon, "job-apply-stale").await;
        assert_assigned(
            post_json(&client, &url, &poll("worker-a")).await,
            "job-apply-stale",
        );

        let first = WorkerProtocolMessage::Result(success_result("worker-a", "job-apply-stale"));
        let WorkerProtocolMessage::Release(release) = post_json(&client, &url, &first).await else {
            panic!("expected reclaimed stale result")
        };
        assert_eq!(release.disposition, ReleaseDisposition::Reclaimed);

        let mut conflicting = first;
        let WorkerProtocolMessage::Result(result) = &mut conflicting else {
            unreachable!()
        };
        result.summary = Some("conflicting stale payload".to_string());
        let WorkerProtocolMessage::Release(replay) = post_json(&client, &url, &conflicting).await
        else {
            panic!("expected compactable stale replay")
        };
        assert_eq!(replay, release);
    })
}

#[test]
fn one_heartbeat_returns_all_exact_attempt_cancellations_in_stable_order() {
    temper_engine_io::block_on_with(move |_cx, handle| async move {
        let applier = Arc::new(OwnershipLostApplier {
            application_calls: AtomicUsize::new(0),
        });
        let (daemon, url) = spawn_with_role_limit(&handle, applier, 2).await;
        for (job_id, attempt_id) in [
            ("job-multi-b", "attempt-multi-b"),
            ("job-multi-a", "attempt-multi-a"),
        ] {
            daemon
                .stage_recovered_job(recovered_job(job_id, attempt_id), "daemon-boot-original")
                .await
                .expect("recovered assignment stages");
        }

        let client = temper_engine_io::http::JsonClient::new();
        assert_eq!(
            post(&client, &url, &register_with_capacity("worker-fenced", 2))
                .await
                .status,
            204
        );
        let WorkerProtocolMessage::CancelAttempts(cancellation) = post_json(
            &client,
            &url,
            &recovered_heartbeat([
                ("job-multi-b", "attempt-multi-b"),
                ("job-multi-a", "attempt-multi-a"),
            ]),
        )
        .await
        else {
            panic!("expected multi-attempt cancellation")
        };
        let identities = cancellation
            .cancellations()
            .iter()
            .map(|entry| (entry.job_id(), entry.attempt_id()))
            .collect::<Vec<_>>();
        assert_eq!(
            identities,
            [
                ("job-multi-a", Some("attempt-multi-a")),
                ("job-multi-b", Some("attempt-multi-b")),
            ]
        );
    })
}
