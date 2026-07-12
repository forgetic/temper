// SPDX-License-Identifier: MPL-2.0

use crate::support::*;

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

fn assert_mismatched_result(message: WorkerProtocolMessage) {
    match message {
        WorkerProtocolMessage::Error(error) => {
            assert_eq!(
                error.code,
                temper_protocol_worker::ErrorCode::MalformedMessage
            );
        }
        other => panic!("expected mismatched-result error, got {other:?}"),
    }
}
