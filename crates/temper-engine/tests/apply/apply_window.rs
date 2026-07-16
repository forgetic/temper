// SPDX-License-Identifier: MPL-2.0

use crate::support::*;

#[test]
fn apply_window_blocks_duplicate_enqueue_until_apply_finishes() {
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

        enqueue_standard_job(&daemon, "job-apply-window-1").await;
        assert_assigned(
            post_json(&client, &url, &poll("worker-a")).await,
            "job-apply-window-1",
        );

        let result = success_result("worker-a", "job-apply-window-1");
        let acknowledgement =
            post_json_background(&handle, &url, WorkerProtocolMessage::Result(result.clone()));
        let (job, recorded_result) = rx.recv().await.expect("applier starts and parks");
        assert_eq!(job.job_id, "job-apply-window-1");
        assert_eq!(recorded_result.job_id, result.job_id);

        enqueue_standard_job(&daemon, "job-apply-window-1").await;
        assert_poll_timeout(post_json(&client, &url, &poll_with_wait("worker-a", 25)).await);

        release_tx.send(());
        assert_release(
            acknowledgement
                .recv()
                .await
                .expect("result acknowledgement"),
            "worker-a",
            "job-apply-window-1",
        );
        eventually_enqueue_and_assign(
            &cx,
            &daemon,
            &client,
            &url,
            "worker-a",
            "job-apply-window-1",
        )
        .await;
    })
}

#[test]
fn post_apply_grace_blocks_immediate_duplicate_enqueue_then_expires() {
    temper_engine_io::block_on_with(move |cx, handle| async move {
        let (record_tx, mut rx) = temper_engine_io::channel();
        let (release_tx, release_rx) = temper_engine_io::oneshot();
        let (daemon, url) = spawn_with_applier_and_apply_grace(
            &handle,
            Arc::new(GatedApplier::new(record_tx, vec![release_rx])),
            Duration::from_millis(200),
        )
        .await;
        let client = temper_engine_io::http::JsonClient::new();
        assert_eq!(post(&client, &url, &register("worker-a")).await.status, 204);

        enqueue_standard_job(&daemon, "job-apply-grace-1").await;
        assert_assigned(
            post_json(&client, &url, &poll("worker-a")).await,
            "job-apply-grace-1",
        );

        let result = success_result("worker-a", "job-apply-grace-1");
        let acknowledgement =
            post_json_background(&handle, &url, WorkerProtocolMessage::Result(result.clone()));
        let (job, recorded_result) = rx.recv().await.expect("applier starts and parks");
        assert_eq!(job.job_id, "job-apply-grace-1");
        assert_eq!(recorded_result.job_id, result.job_id);
        release_tx.send(());
        assert_release(
            acknowledgement
                .recv()
                .await
                .expect("result acknowledgement"),
            "worker-a",
            "job-apply-grace-1",
        );
        temper_engine_io::runtime::sleep_for(&cx, Duration::from_millis(25)).await;

        enqueue_standard_job(&daemon, "job-apply-grace-1").await;
        assert_poll_timeout(post_json(&client, &url, &poll_with_wait("worker-a", 25)).await);

        temper_engine_io::runtime::sleep_for(&cx, Duration::from_millis(225)).await;
        enqueue_standard_job(&daemon, "job-apply-grace-1").await;
        assert_assigned(
            post_json(&client, &url, &poll("worker-a")).await,
            "job-apply-grace-1",
        );
    })
}

#[test]
fn apply_block_is_global_but_post_apply_grace_is_per_job_id() {
    temper_engine_io::block_on_with(move |cx, handle| async move {
        let (record_tx, mut rx) = temper_engine_io::channel();
        let (release_tx, release_rx) = temper_engine_io::oneshot();
        let (daemon, url) = spawn_with_applier_and_apply_grace(
            &handle,
            Arc::new(GatedApplier::new(record_tx, vec![release_rx])),
            Duration::from_millis(200),
        )
        .await;
        let client = temper_engine_io::http::JsonClient::new();
        assert_eq!(post(&client, &url, &register("worker-a")).await.status, 204);
        assert_eq!(post(&client, &url, &register("worker-b")).await.status, 204);

        enqueue_standard_job(&daemon, "job-blocked").await;
        assert_assigned(
            post_json(&client, &url, &poll("worker-a")).await,
            "job-blocked",
        );

        let result = success_result("worker-a", "job-blocked");
        let acknowledgement =
            post_json_background(&handle, &url, WorkerProtocolMessage::Result(result.clone()));
        let (job, recorded_result) = rx.recv().await.expect("applier starts and parks");
        assert_eq!(job.job_id, "job-blocked");
        assert_eq!(recorded_result.job_id, result.job_id);

        // A distinct job can be a child that the active result apply has only
        // partially created. It must not dispatch until that apply has finished
        // wiring dependency metadata and lifecycle labels.
        enqueue_standard_job(&daemon, "job-blocked").await;
        enqueue_standard_job(&daemon, "job-created-during-apply").await;
        assert_poll_timeout(post_json(&client, &url, &poll_with_wait("worker-b", 25)).await);

        release_tx.send(());
        assert_release(
            acknowledgement
                .recv()
                .await
                .expect("result acknowledgement"),
            "worker-a",
            "job-blocked",
        );
        temper_engine_io::runtime::sleep_for(&cx, Duration::from_millis(25)).await;
        assert!(rx.try_recv().is_none());

        // The originating job remains under its per-id grace period, while an
        // independent job is dispatchable once the global apply window closes.
        enqueue_standard_job(&daemon, "job-blocked").await;
        enqueue_standard_job(&daemon, "job-after-apply").await;
        assert_assigned(
            post_json(&client, &url, &poll("worker-b")).await,
            "job-after-apply",
        );
    })
}
