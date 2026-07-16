// SPDX-License-Identifier: MPL-2.0

use crate::support::*;

struct RecoveredOnlyApplier {
    tx: temper_engine_io::CqSender<(InFlightJob, JobResult, temper_engine::ClaimContext)>,
}

struct RetryRecoveredApplier {
    attempts: std::sync::atomic::AtomicUsize,
    tx: temper_engine_io::CqSender<temper_engine::ClaimContext>,
}

#[async_trait::async_trait]
impl temper_engine::ResultApplier for RetryRecoveredApplier {
    async fn apply(&self, _job: InFlightJob, _result: JobResult) -> temper_engine::ApplyOutcome {
        panic!("a recovered retry must not fall through to ordinary apply")
    }

    async fn apply_recovered(
        &self,
        _job: InFlightJob,
        _result: JobResult,
        context: temper_engine::ClaimContext,
    ) -> temper_engine::ApplyOutcome {
        let _ = self.tx.send(context);
        if self
            .attempts
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            == 0
        {
            temper_engine::ApplyOutcome::Retryable {
                reason: "recovered claim reattachment unavailable".to_string(),
            }
        } else {
            temper_engine::ApplyOutcome::Applied
        }
    }
}

#[async_trait::async_trait]
impl temper_engine::ResultApplier for RecoveredOnlyApplier {
    async fn apply(&self, _job: InFlightJob, _result: JobResult) -> temper_engine::ApplyOutcome {
        panic!("staged result must use recovered application")
    }

    async fn apply_recovered(
        &self,
        job: InFlightJob,
        result: JobResult,
        context: temper_engine::ClaimContext,
    ) -> temper_engine::ApplyOutcome {
        let _ = self.tx.send((job, result, context));
        temper_engine::ApplyOutcome::Applied
    }
}

#[test]
fn accepted_result_invokes_applier_with_in_flight_context() {
    temper_engine_io::block_on_with(move |_cx, handle| async move {
        let (daemon, url, mut rx) = spawn_recording(&handle).await;
        let client = temper_engine_io::http::JsonClient::new();
        assert_eq!(post(&client, &url, &register("worker-a")).await.status, 204);

        let artifact = artifact();
        let payload = json!({"prompt":"implement", "issue":114});
        daemon
            .enqueue_job(
                "job-apply-1",
                "engineer",
                "ai/temper",
                artifact.clone(),
                payload.clone(),
            )
            .await;

        assert_assigned(
            post_json(&client, &url, &poll("worker-a")).await,
            "job-apply-1",
        );

        let branch = Branch {
            name: "agent/pr-for-code-114".to_string(),
            head_sha: "abc123".to_string(),
        };
        let posted_result = job_result(
            "worker-a",
            "job-apply-1",
            vec![RepoOutcome {
                repo: "ai/temper".to_string(),
                branch,
            }],
        );
        assert_release(
            post_json(
                &client,
                &url,
                &WorkerProtocolMessage::Result(posted_result.clone()),
            )
            .await,
            "worker-a",
            "job-apply-1",
        );

        let (job, recorded_result) = rx.recv().await.expect("applier records accepted result");
        assert_eq!(job.job_id, "job-apply-1");
        assert_eq!(job.role, "engineer");
        assert_eq!(job.repo, "ai/temper");
        assert_eq!(job.artifact, artifact);
        assert_eq!(job.job_payload, payload);
        assert_eq!(recorded_result.job_id, posted_result.job_id);
        assert_eq!(recorded_result.status, posted_result.status);
        assert_eq!(recorded_result.repos, posted_result.repos);
        assert!(rx.try_recv().is_none());
    })
}

#[test]
fn matching_result_completes_staged_startup_assignment_through_recovered_apply() {
    temper_engine_io::block_on_with(move |_cx, handle| async move {
        let (tx, mut rx) = temper_engine_io::channel();
        let (daemon, url) =
            spawn_with_applier(&handle, Arc::new(RecoveredOnlyApplier { tx })).await;
        daemon
            .stage_recovered_job(
                temper_worker_registry::RecoveredJob {
                    job_id: "job-recovered-1".to_string(),
                    attempt_id: Some("attempt-recovered-1".to_string()),
                    worker_id: "worker-a".to_string(),
                    role: "engineer".to_string(),
                    repo: "ai/temper".to_string(),
                    artifact: artifact(),
                    job_payload: json!({"prompt":"resume"}),
                },
                "daemon-boot-original",
            )
            .await
            .expect("recovered assignment stages");

        let client = temper_engine_io::http::JsonClient::new();
        assert_eq!(post(&client, &url, &register("worker-a")).await.status, 204);
        let mut result = success_result("worker-a", "job-recovered-1");
        result.attempt_id = Some("attempt-recovered-1".to_string());
        assert_release(
            post_json(
                &client,
                &url,
                &WorkerProtocolMessage::Result(result.clone()),
            )
            .await,
            "worker-a",
            "job-recovered-1",
        );

        let (job, applied, context) = rx.recv().await.expect("recovered apply runs");
        assert_eq!(job.job_id, "job-recovered-1");
        assert_eq!(job.attempt_id.as_deref(), Some("attempt-recovered-1"));
        assert_eq!(applied, result);
        assert_eq!(context.worker_id, "worker-a");
        assert_eq!(context.daemon_boot_id, "daemon-boot-original");
    })
}

#[test]
fn recovered_result_retry_retains_recovered_application_context() {
    temper_engine_io::block_on_with(move |_cx, handle| async move {
        let (tx, mut rx) = temper_engine_io::channel();
        let (daemon, url) = spawn_with_applier(
            &handle,
            Arc::new(RetryRecoveredApplier {
                attempts: std::sync::atomic::AtomicUsize::new(0),
                tx,
            }),
        )
        .await;
        daemon
            .stage_recovered_job(
                temper_worker_registry::RecoveredJob {
                    job_id: "job-recovered-retry".to_string(),
                    attempt_id: Some("attempt-recovered-retry".to_string()),
                    worker_id: "worker-a".to_string(),
                    role: "engineer".to_string(),
                    repo: "ai/temper".to_string(),
                    artifact: artifact(),
                    job_payload: json!({"prompt":"resume"}),
                },
                "daemon-boot-original",
            )
            .await
            .expect("recovered assignment stages");

        let client = temper_engine_io::http::JsonClient::new();
        assert_eq!(post(&client, &url, &register("worker-a")).await.status, 204);
        let mut result = success_result("worker-a", "job-recovered-retry");
        result.attempt_id = Some("attempt-recovered-retry".to_string());

        let first = post(
            &client,
            &url,
            &WorkerProtocolMessage::Result(result.clone()),
        )
        .await;
        assert_eq!(
            first.status, 503,
            "transient reattachment stays unacknowledged"
        );
        assert_eq!(
            rx.recv()
                .await
                .expect("first recovered context")
                .daemon_boot_id,
            "daemon-boot-original"
        );

        assert_release(
            post_json(&client, &url, &WorkerProtocolMessage::Result(result)).await,
            "worker-a",
            "job-recovered-retry",
        );
        assert_eq!(
            rx.recv()
                .await
                .expect("retry recovered context")
                .daemon_boot_id,
            "daemon-boot-original"
        );
        assert!(rx.try_recv().is_none());
    })
}

#[test]
fn result_without_in_flight_job_does_not_invoke_applier() {
    temper_engine_io::block_on_with(move |_cx, handle| async move {
        let (daemon, url, mut rx) = spawn_recording(&handle).await;
        let client = temper_engine_io::http::JsonClient::new();
        assert_eq!(post(&client, &url, &register("worker-a")).await.status, 204);

        assert_mismatched_result(
            post_json(
                &client,
                &url,
                &WorkerProtocolMessage::Result(job_result("worker-a", "phantom-job", Vec::new())),
            )
            .await,
        );

        daemon
            .enqueue_job(
                "pending-job",
                "architect",
                "ai/temper",
                artifact(),
                json!({"n":"pending"}),
            )
            .await;
        assert_mismatched_result(
            post_json(
                &client,
                &url,
                &WorkerProtocolMessage::Result(job_result("worker-a", "pending-job", Vec::new())),
            )
            .await,
        );

        daemon
            .enqueue_job(
                "real-job",
                "engineer",
                "ai/temper",
                artifact(),
                json!({"n":1}),
            )
            .await;
        assert_assigned(
            post_json(&client, &url, &poll("worker-a")).await,
            "real-job",
        );

        let real_result = job_result("worker-a", "real-job", Vec::new());
        assert_release(
            post_json(
                &client,
                &url,
                &WorkerProtocolMessage::Result(real_result.clone()),
            )
            .await,
            "worker-a",
            "real-job",
        );

        let (job, recorded_result) = rx.recv().await.expect("applier records real result");
        assert_eq!(job.job_id, "real-job");
        assert_eq!(recorded_result.job_id, real_result.job_id);
        assert!(rx.try_recv().is_none());
    })
}

#[test]
fn lost_acknowledgement_replays_exact_result_without_double_apply() {
    temper_engine_io::block_on_with(move |_cx, handle| async move {
        let (daemon, url, mut rx) = spawn_recording(&handle).await;
        let client = temper_engine_io::http::JsonClient::new();
        assert_eq!(post(&client, &url, &register("worker-a")).await.status, 204);
        enqueue_standard_job(&daemon, "job-replay-1").await;
        assert_assigned(
            post_json(&client, &url, &poll("worker-a")).await,
            "job-replay-1",
        );
        let result = success_result("worker-a", "job-replay-1");

        assert_release(
            post_json(
                &client,
                &url,
                &WorkerProtocolMessage::Result(result.clone()),
            )
            .await,
            "worker-a",
            "job-replay-1",
        );
        let _ = rx.recv().await.expect("first delivery applies");

        assert_release(
            post_json(
                &client,
                &url,
                &WorkerProtocolMessage::Result(result.clone()),
            )
            .await,
            "worker-a",
            "job-replay-1",
        );
        assert!(rx.try_recv().is_none(), "duplicate result must not reapply");

        let mut conflicting = result;
        conflicting.summary = Some("different payload".to_string());
        match post_json(&client, &url, &WorkerProtocolMessage::Result(conflicting)).await {
            WorkerProtocolMessage::Error(error) => {
                assert_eq!(
                    error.code,
                    temper_protocol_worker::ErrorCode::MalformedMessage
                )
            }
            other => panic!("expected conflicting duplicate rejection, got {other:?}"),
        }
        assert!(rx.try_recv().is_none());
    })
}

fn assert_mismatched_result(message: WorkerProtocolMessage) {
    match message {
        WorkerProtocolMessage::Release(release) => {
            assert_eq!(
                release.disposition,
                temper_protocol_worker::ReleaseDisposition::Reclaimed
            );
        }
        other => panic!("expected reclaimed release, got {other:?}"),
    }
}
