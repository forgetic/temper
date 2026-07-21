// SPDX-License-Identifier: MPL-2.0

use super::*;
use temper_protocol_worker::{HeartbeatState, JobHeartbeat};

fn heartbeat_jobs(
    worker_id: &str,
    jobs: impl IntoIterator<Item = (&'static str, &'static str)>,
) -> WorkerProtocolMessage {
    WorkerProtocolMessage::Heartbeat(Heartbeat {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: worker_id.to_string(),
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

#[test]
fn repeated_unknown_heartbeat_is_cancelled_and_stale_payloads_compact_idempotently() {
    temper_engine_io::block_on_with(move |_cx, handle| async move {
        let (_, url) = spawn(&handle).await;
        let client = JsonClient::new();
        assert_eq!(
            post(
                &client,
                &url,
                &register("worker-a", "engineer", "ai/temper", 1)
            )
            .await
            .status,
            204
        );
        let heartbeat = heartbeat_jobs(
            "worker-a",
            [("missing-after-restart", "attempt-before-restart")],
        );

        let first = post_json(&client, &url, &heartbeat).await;
        let second = post_json(&client, &url, &heartbeat).await;
        assert_eq!(first, second, "the cancellation directive is stable");
        let WorkerProtocolMessage::CancelAttempts(cancel) = first else {
            panic!("expected cancellation directive")
        };
        assert_eq!(cancel.cancellations().len(), 1);
        assert!(cancel.cancellations()[0].matches_exact(
            "worker-a",
            "missing-after-restart",
            Some("attempt-before-restart")
        ));

        let stale = result(
            "worker-a",
            "missing-after-restart",
            Some("attempt-before-restart"),
        );
        for summary in ["first payload", "conflicting duplicate"] {
            let mut delivery = stale.clone();
            let WorkerProtocolMessage::Result(result) = &mut delivery else {
                unreachable!()
            };
            result.summary = Some(summary.to_string());
            match post_json(&client, &url, &delivery).await {
                WorkerProtocolMessage::Release(release) => {
                    assert_eq!(release.disposition, ReleaseDisposition::Reclaimed);
                    assert_eq!(
                        release.attempt_id.as_deref(),
                        Some("attempt-before-restart")
                    );
                }
                other => panic!("expected reclaimed release, got {other:?}"),
            }
        }
    })
}

#[test]
fn stale_heartbeat_and_result_do_not_release_a_newer_attempt() {
    temper_engine_io::block_on_with(move |_cx, handle| async move {
        let (daemon, url) = spawn(&handle).await;
        let client = JsonClient::new();
        let _ = post(
            &client,
            &url,
            &register("worker-a", "engineer", "ai/temper", 1),
        )
        .await;
        daemon
            .enqueue_job("job-1", "engineer", "ai/temper", artifact(), json!({}))
            .await;
        daemon
            .enqueue_job("job-2", "engineer", "ai/temper", artifact(), json!({}))
            .await;
        let current = match post_json(&client, &url, &poll("worker-a")).await {
            WorkerProtocolMessage::Assign(assign) => assign,
            other => panic!("expected current assignment, got {other:?}"),
        };
        let current_attempt = current.attempt_id.as_deref().expect("fenced assignment");

        let cancellation = post_json(
            &client,
            &url,
            &heartbeat_jobs("worker-a", [("job-1", "attempt-older")]),
        )
        .await;
        let WorkerProtocolMessage::CancelAttempts(cancellation) = cancellation else {
            panic!("expected stale attempt cancellation")
        };
        assert!(cancellation.cancellations()[0].matches_exact(
            "worker-a",
            "job-1",
            Some("attempt-older")
        ));
        match post_json(
            &client,
            &url,
            &result("worker-a", "job-1", Some("attempt-older")),
        )
        .await
        {
            WorkerProtocolMessage::Release(release) => {
                assert_eq!(release.disposition, ReleaseDisposition::Superseded)
            }
            other => panic!("expected superseded release, got {other:?}"),
        }
        assert_error(
            post_json(&client, &url, &poll_with_wait("worker-a", 1)).await,
            ErrorCode::PollTimeout,
        );

        match post_json(
            &client,
            &url,
            &result("worker-a", "job-1", Some(current_attempt)),
        )
        .await
        {
            WorkerProtocolMessage::Release(release) => {
                assert_eq!(release.disposition, ReleaseDisposition::Accepted)
            }
            other => panic!("expected current release, got {other:?}"),
        }
        match post_json(&client, &url, &poll("worker-a")).await {
            WorkerProtocolMessage::Assign(assign) => assert_eq!(assign.job_id, "job-2"),
            other => panic!("expected second assignment, got {other:?}"),
        }
    })
}
