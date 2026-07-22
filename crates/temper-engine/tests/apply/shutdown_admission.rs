// SPDX-License-Identifier: MPL-2.0

use crate::support::*;

use std::collections::BTreeSet;

use temper_protocol_worker::{
    ContextOutcome, FetchContext, ForgeContextErrorCode, ForgeContextOperation,
    ForgeGetItemOperation,
};

#[test]
fn shutdown_fences_new_results_and_reports_an_already_admitted_application() {
    temper_engine_io::block_on_with(move |_cx, handle| async move {
        let (record_tx, mut record_rx) = temper_engine_io::channel();
        let (release_tx, release_rx) = temper_engine_io::oneshot();
        let (daemon, url) = spawn_with_applier(
            &handle,
            Arc::new(GatedApplier::new(record_tx, vec![release_rx])),
        )
        .await;
        let client = temper_engine_io::http::JsonClient::new();
        assert_eq!(post(&client, &url, &register("worker-a")).await.status, 204);
        enqueue_standard_job(&daemon, "job-shutdown-apply").await;
        let assignment = match post_json(&client, &url, &poll("worker-a")).await {
            WorkerProtocolMessage::Assign(assign) => assign,
            other => panic!("expected assignment, got {other:?}"),
        };
        let mut result = success_result("worker-a", "job-shutdown-apply");
        result.attempt_id = assignment.attempt_id.clone();
        let result_message = WorkerProtocolMessage::Result(result.clone());
        let acknowledgement = post_json_background(&handle, &url, result_message.clone());
        record_rx.recv().await.expect("application starts");

        let shutdown = daemon.begin_shutdown().await;
        let identity = temper_engine::AssignmentAttemptIdentity::new(
            "worker-a",
            "job-shutdown-apply",
            assignment.attempt_id,
        );
        assert_eq!(
            shutdown.report().pending_results,
            BTreeSet::from([identity.clone()])
        );
        assert_eq!(
            shutdown.report().pending_applications,
            BTreeSet::from([identity])
        );
        assert_eq!(
            post(&client, &url, &result_message).await.status,
            503,
            "a result racing the worker outbox after the fence remains retryable"
        );
        let context = WorkerProtocolMessage::FetchContext(FetchContext {
            protocol_version: WORKER_PROTOCOL_VERSION,
            worker_id: "worker-a".to_string(),
            job_id: "job-shutdown-apply".to_string(),
            attempt_id: result.attempt_id.clone(),
            operation: ForgeContextOperation::ForgeGetItem(ForgeGetItemOperation {
                repo: "ai/temper".to_string(),
                number: 660,
                artifact_type: None,
                include_comments: false,
            }),
        });
        let WorkerProtocolMessage::ContextResponse(context) =
            post_json(&client, &url, &context).await
        else {
            panic!("expected context denial")
        };
        assert_eq!(
            context.outcome,
            ContextOutcome::Error {
                code: ForgeContextErrorCode::NotAuthorized,
            }
        );
        assert!(record_rx.try_recv().is_none(), "no second apply starts");

        release_tx.send(());
        assert_release(
            acknowledgement
                .recv()
                .await
                .expect("admitted result receives its release"),
            "worker-a",
            "job-shutdown-apply",
        );
        assert!(shutdown.wait_for_join().await);
    })
}

struct GatedClaimApplier {
    entered: temper_engine_io::CqSender<()>,
    claim_release: StdMutex<Option<temper_engine_io::OneshotReceiver<()>>>,
    rolled_back: temper_engine_io::CqSender<(InFlightJob, temper_engine::ClaimContext)>,
}

#[async_trait::async_trait]
impl temper_engine::ResultApplier for GatedClaimApplier {
    async fn claim(
        &self,
        _job: InFlightJob,
        _context: temper_engine::ClaimContext,
    ) -> temper_engine::ClaimOutcome {
        let release = self
            .claim_release
            .lock()
            .expect("claim gate")
            .take()
            .expect("one claim");
        let _ = self.entered.send(());
        release.recv().await.expect("claim released");
        temper_engine::ClaimOutcome::Claimed
    }

    async fn release_claim(&self, job: InFlightJob, context: temper_engine::ClaimContext) {
        let _ = self.rolled_back.send((job, context));
    }

    async fn apply(&self, _job: InFlightJob, _result: JobResult) -> temper_engine::ApplyOutcome {
        panic!("claim rollback test never applies a result")
    }
}

#[test]
fn claim_that_completes_after_the_fence_rolls_back_before_join() {
    temper_engine_io::block_on_with(move |_cx, handle| async move {
        let (entered_tx, mut entered_rx) = temper_engine_io::channel();
        let (claim_release_tx, claim_release_rx) = temper_engine_io::oneshot();
        let (rolled_back_tx, mut rolled_back_rx) = temper_engine_io::channel();
        let applier = Arc::new(GatedClaimApplier {
            entered: entered_tx,
            claim_release: StdMutex::new(Some(claim_release_rx)),
            rolled_back: rolled_back_tx,
        });
        let (daemon, url) = spawn_with_applier(&handle, applier).await;
        let client = temper_engine_io::http::JsonClient::new();
        assert_eq!(post(&client, &url, &register("worker-a")).await.status, 204);
        enqueue_standard_job(&daemon, "job-shutdown-claim").await;

        let (poll_tx, poll_rx) = temper_engine_io::oneshot();
        let poll_url = url.clone();
        handle.spawn_with_cx(move |_cx| async move {
            let response = post(
                &temper_engine_io::http::JsonClient::new(),
                &poll_url,
                &poll("worker-a"),
            )
            .await;
            poll_tx.send(response);
        });
        entered_rx.recv().await.expect("claim starts");

        let shutdown = daemon.begin_shutdown().await;
        assert_eq!(shutdown.report().pending_claims.len(), 1);
        claim_release_tx.send(());
        assert_eq!(
            poll_rx.recv().await.expect("shutdown poll response").status,
            204
        );
        let (job, context) = rolled_back_rx.recv().await.expect("claim rolls back");
        assert_eq!(job.job_id, "job-shutdown-claim");
        assert_eq!(context.worker_id, "worker-a");
        assert!(shutdown.wait_for_join().await);
    })
}

struct ReleaseRecordingApplier {
    released: temper_engine_io::CqSender<(InFlightJob, temper_engine::ClaimContext)>,
}

#[async_trait::async_trait]
impl temper_engine::ResultApplier for ReleaseRecordingApplier {
    async fn release_claim(&self, job: InFlightJob, context: temper_engine::ClaimContext) {
        let _ = self.released.send((job, context));
    }

    async fn apply(&self, _job: InFlightJob, _result: JobResult) -> temper_engine::ApplyOutcome {
        temper_engine::ApplyOutcome::Applied
    }
}

#[test]
fn shutdown_release_matches_only_worker_joined_exact_attempts() {
    temper_engine_io::block_on_with(move |_cx, handle| async move {
        let (released_tx, mut released_rx) = temper_engine_io::channel();
        let (daemon, url) = spawn_with_applier(
            &handle,
            Arc::new(ReleaseRecordingApplier {
                released: released_tx,
            }),
        )
        .await;
        let client = temper_engine_io::http::JsonClient::new();
        for worker in ["worker-a", "worker-b"] {
            assert_eq!(post(&client, &url, &register(worker)).await.status, 204);
        }
        for job_id in ["job-joined", "job-unresolved"] {
            enqueue_standard_job(&daemon, job_id).await;
        }
        let joined = match post_json(&client, &url, &poll("worker-a")).await {
            WorkerProtocolMessage::Assign(assign) => assign,
            other => panic!("expected joined assignment, got {other:?}"),
        };
        let unresolved = match post_json(&client, &url, &poll("worker-b")).await {
            WorkerProtocolMessage::Assign(assign) => assign,
            other => panic!("expected unresolved assignment, got {other:?}"),
        };

        let shutdown = daemon.begin_shutdown().await;
        assert!(shutdown.wait_for_join().await);
        let joined_identity = temper_engine::AssignmentAttemptIdentity::new(
            "worker-a",
            joined.job_id.clone(),
            joined.attempt_id.clone(),
        );
        let wrong_attempt = temper_engine::AssignmentAttemptIdentity::new(
            "worker-b",
            unresolved.job_id.clone(),
            Some("not-the-current-attempt".to_string()),
        );
        daemon
            .release_joined_assignments_for_shutdown(&BTreeSet::from([
                joined_identity,
                wrong_attempt,
            ]))
            .await;

        let (released, context) = released_rx.recv().await.expect("one exact release");
        assert_eq!(released.job_id, joined.job_id);
        assert_eq!(context.worker_id, "worker-a");
        assert!(released_rx.try_recv().is_none());
    })
}

#[test]
fn replacement_assignment_rejects_old_attempt_result_and_context() {
    temper_engine_io::block_on_with(move |_cx, handle| async move {
        let daemon = temper_engine::Daemon::new(Arc::new(handle.clone())).begin_startup_recovery();
        daemon
            .stage_recovered_job(
                temper_worker_registry::RecoveredJob {
                    job_id: "job-replaced".to_string(),
                    attempt_id: Some("attempt-old".to_string()),
                    worker_id: "worker-a".to_string(),
                    role: "engineer".to_string(),
                    repo: "ai/temper".to_string(),
                    artifact: artifact(),
                    job_payload: json!({"prompt":"recover"}),
                },
                "daemon-boot-old",
            )
            .await
            .expect("old assignment stages");
        let (_daemon, url) = spawn_daemon(&handle, daemon.clone()).await;
        let client = temper_engine_io::http::JsonClient::new();
        assert_eq!(post(&client, &url, &register("worker-a")).await.status, 204);
        assert_eq!(daemon.collect_startup_orphans().await.len(), 1);
        daemon.complete_startup_recovery().await;
        enqueue_standard_job(&daemon, "job-replaced").await;
        let replacement = match post_json(&client, &url, &poll("worker-a")).await {
            WorkerProtocolMessage::Assign(assign) => assign,
            other => panic!("expected replacement assignment, got {other:?}"),
        };
        assert_ne!(replacement.attempt_id.as_deref(), Some("attempt-old"));

        let mut old_result = success_result("worker-a", "job-replaced");
        old_result.attempt_id = Some("attempt-old".to_string());
        let WorkerProtocolMessage::Release(release) =
            post_json(&client, &url, &WorkerProtocolMessage::Result(old_result)).await
        else {
            panic!("expected old-attempt release")
        };
        assert_eq!(release.disposition, ReleaseDisposition::Superseded);

        let old_context = WorkerProtocolMessage::FetchContext(FetchContext {
            protocol_version: WORKER_PROTOCOL_VERSION,
            worker_id: "worker-a".to_string(),
            job_id: "job-replaced".to_string(),
            attempt_id: Some("attempt-old".to_string()),
            operation: ForgeContextOperation::ForgeGetItem(ForgeGetItemOperation {
                repo: "ai/temper".to_string(),
                number: 660,
                artifact_type: None,
                include_comments: false,
            }),
        });
        let WorkerProtocolMessage::ContextResponse(response) =
            post_json(&client, &url, &old_context).await
        else {
            panic!("expected context denial")
        };
        assert_eq!(
            response.outcome,
            ContextOutcome::Error {
                code: ForgeContextErrorCode::NotAuthorized,
            }
        );
    })
}
