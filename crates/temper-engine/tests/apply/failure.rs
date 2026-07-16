// SPDX-License-Identifier: MPL-2.0

use crate::support::*;

#[test]
fn transient_failure_applies_retry_bookkeeping_then_rescans() {
    temper_engine_io::block_on_with(move |_cx, handle| async move {
        let (daemon, url, mut rx) = spawn_recording_with_apply_grace(&handle, Duration::ZERO).await;
        let client = temper_engine_io::http::JsonClient::new();
        assert_eq!(post(&client, &url, &register("worker-a")).await.status, 204);

        enqueue_standard_job(&daemon, "job-retry-1").await;

        let first_job_id = assignment_job_id(post_json(&client, &url, &poll("worker-a")).await);
        assert_eq!(first_job_id, "job-retry-1");

        let transient = transient_failure_result("worker-a", &first_job_id);
        assert_release(
            post_json(
                &client,
                &url,
                &WorkerProtocolMessage::Result(transient.clone()),
            )
            .await,
            "worker-a",
            &first_job_id,
        );

        let (retry_job, retry_result) = rx
            .recv()
            .await
            .expect("applier records transient retry bookkeeping");
        assert_eq!(retry_job.job_id, first_job_id);
        assert_eq!(retry_result.job_id, transient.job_id);
        assert_eq!(retry_result.status, ResultStatus::Failure);
        assert_eq!(
            retry_result.failure.as_ref().map(|failure| failure.class),
            Some(FailureClass::Transient)
        );

        enqueue_standard_job(&daemon, &first_job_id).await;
        let retry_job_id = assignment_job_id(post_json(&client, &url, &poll("worker-a")).await);
        assert_eq!(retry_job_id, first_job_id);

        let final_result = success_result("worker-a", &retry_job_id);
        assert_release(
            post_json(
                &client,
                &url,
                &WorkerProtocolMessage::Result(final_result.clone()),
            )
            .await,
            "worker-a",
            &retry_job_id,
        );

        let (job, recorded_result) = rx
            .recv()
            .await
            .expect("applier records final success result");
        assert_eq!(job.job_id, retry_job_id);
        assert_eq!(recorded_result.job_id, final_result.job_id);
        assert_eq!(recorded_result.status, ResultStatus::Success);
        assert_eq!(recorded_result.repos, final_result.repos);
        assert!(rx.try_recv().is_none());
    })
}

#[test]
fn permanent_failure_apply_window_unblocks_after_apply_completes() {
    temper_engine_io::block_on_with(move |cx, handle| async move {
        let (record_tx, mut rx) = temper_engine_io::channel();
        let (release_tx, release_rx) = temper_engine_io::oneshot();
        let (daemon, url) = spawn_with_applier_and_apply_grace(
            &handle,
            Arc::new(GatedApplier::new(record_tx, vec![release_rx])),
            Duration::ZERO,
        )
        .await;
        let client = temper_engine_io::http::JsonClient::new();
        assert_eq!(post(&client, &url, &register("worker-a")).await.status, 204);

        enqueue_standard_job(&daemon, "job-permanent-failure-1").await;
        assert_assigned(
            post_json(&client, &url, &poll("worker-a")).await,
            "job-permanent-failure-1",
        );

        let failure = permanent_failure_result("worker-a", "job-permanent-failure-1");
        let acknowledgement = post_json_background(
            &handle,
            &url,
            WorkerProtocolMessage::Result(failure.clone()),
        );
        let (job, recorded_result) = rx.recv().await.expect("applier starts and parks");
        assert_eq!(job.job_id, "job-permanent-failure-1");
        assert_eq!(recorded_result.job_id, failure.job_id);
        assert_eq!(recorded_result.status, ResultStatus::Failure);
        assert_eq!(
            recorded_result
                .failure
                .as_ref()
                .map(|failure| failure.class),
            Some(FailureClass::Permanent)
        );

        enqueue_standard_job(&daemon, "job-permanent-failure-1").await;
        assert_poll_timeout(post_json(&client, &url, &poll_with_wait("worker-a", 25)).await);

        release_tx.send(());
        assert_release(
            acknowledgement
                .recv()
                .await
                .expect("result acknowledgement"),
            "worker-a",
            "job-permanent-failure-1",
        );
        eventually_enqueue_and_assign(
            &cx,
            &daemon,
            &client,
            &url,
            "worker-a",
            "job-permanent-failure-1",
        )
        .await;
    })
}
