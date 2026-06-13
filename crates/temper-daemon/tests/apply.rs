// SPDX-License-Identifier: MPL-2.0

use std::{
    sync::{Arc, Mutex as StdMutex},
    time::Duration,
};

use serde_json::json;
use temper_worker_protocol::{
    Artifact, Branch, Capability, Capacity, ErrorCode, Failure, FailureClass, JobResult, Poll,
    Register, ReleaseDisposition, RepoOutcome, ResultStatus, WorkerProtocolMessage,
    WORKER_PROTOCOL_VERSION,
};
use temper_worker_registry::InFlightJob;

struct RecordingApplier {
    tx: temper_io_engine::CqSender<(InFlightJob, JobResult)>,
}

struct GatedApplier {
    tx: temper_io_engine::CqSender<(InFlightJob, JobResult)>,
    releases: StdMutex<Vec<temper_io_engine::OneshotReceiver<()>>>,
}

impl GatedApplier {
    fn new(
        tx: temper_io_engine::CqSender<(InFlightJob, JobResult)>,
        releases: Vec<temper_io_engine::OneshotReceiver<()>>,
    ) -> Self {
        Self {
            tx,
            releases: StdMutex::new(releases),
        }
    }
}

#[async_trait::async_trait]
impl temper_daemon::ResultApplier for GatedApplier {
    async fn apply(&self, job: InFlightJob, result: JobResult) {
        let release = {
            let mut releases = self
                .releases
                .lock()
                .expect("release gate lock is not poisoned");
            assert!(
                !releases.is_empty(),
                "test supplies one release gate per apply call"
            );
            releases.remove(0)
        };
        let _ = self.tx.send((job, result));
        let _ = release.recv().await;
    }
}

#[async_trait::async_trait]
impl temper_daemon::ResultApplier for RecordingApplier {
    async fn apply(&self, job: InFlightJob, result: JobResult) {
        let _ = self.tx.send((job, result));
    }
}

async fn spawn_with_applier(
    handle: &skein::runtime::RuntimeHandle,
    applier: Arc<dyn temper_daemon::ResultApplier>,
) -> (temper_daemon::Daemon, String) {
    spawn_daemon(
        handle,
        temper_daemon::Daemon::with_applier(Arc::new(handle.clone()), applier),
    )
    .await
}

async fn spawn_with_applier_and_apply_grace(
    handle: &skein::runtime::RuntimeHandle,
    applier: Arc<dyn temper_daemon::ResultApplier>,
    apply_grace: Duration,
) -> (temper_daemon::Daemon, String) {
    spawn_daemon(
        handle,
        temper_daemon::Daemon::with_applier(Arc::new(handle.clone()), applier)
            .with_apply_grace(apply_grace),
    )
    .await
}

async fn spawn_daemon(
    handle: &skein::runtime::RuntimeHandle,
    daemon: temper_daemon::Daemon,
) -> (temper_daemon::Daemon, String) {
    let server = temper_daemon::serve(
        handle,
        &daemon,
        "127.0.0.1:0".parse().expect("loopback addr"),
    )
    .await
    .expect("bind test server");
    let addr = server.local_addr();
    (daemon, format!("http://{addr}/v1/message"))
}

async fn spawn_recording(
    handle: &skein::runtime::RuntimeHandle,
) -> (
    temper_daemon::Daemon,
    String,
    temper_io_engine::CqReceiver<(InFlightJob, JobResult)>,
) {
    let (tx, rx) = temper_io_engine::channel();
    let (daemon, url) = spawn_with_applier(handle, Arc::new(RecordingApplier { tx })).await;
    (daemon, url, rx)
}

fn register(worker_id: &str) -> WorkerProtocolMessage {
    WorkerProtocolMessage::Register(Register {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: worker_id.to_string(),
        capabilities: vec![Capability {
            role: "engineer".to_string(),
            repo: "ai/temper".to_string(),
        }],
        capacity: Capacity {
            max_concurrent_jobs: 1,
        },
        labels: None,
    })
}

fn poll_with_wait(worker_id: &str, max_wait_ms: u64) -> WorkerProtocolMessage {
    WorkerProtocolMessage::Poll(Poll {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: worker_id.to_string(),
        free_capacity: 1,
        max_wait_ms: Some(max_wait_ms),
    })
}

fn poll(worker_id: &str) -> WorkerProtocolMessage {
    poll_with_wait(worker_id, 30_000)
}

fn job_result(worker_id: &str, job_id: &str, repos: Vec<RepoOutcome>) -> JobResult {
    JobResult {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: worker_id.to_string(),
        job_id: job_id.to_string(),
        status: ResultStatus::Success,
        repos,
        verdict: None,
        body: None,
        children: Vec::new(),
        failure: None,
        summary: Some("done".to_string()),
        details: None,
    }
}

fn transient_failure_result(worker_id: &str, job_id: &str) -> JobResult {
    failure_result(
        worker_id,
        job_id,
        FailureClass::Transient,
        "temporary worker failure",
        "transient failure",
    )
}

fn permanent_failure_result(worker_id: &str, job_id: &str) -> JobResult {
    failure_result(
        worker_id,
        job_id,
        FailureClass::Permanent,
        "worker could not complete the job",
        "permanent failure",
    )
}

fn failure_result(
    worker_id: &str,
    job_id: &str,
    class: FailureClass,
    message: &str,
    summary: &str,
) -> JobResult {
    JobResult {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: worker_id.to_string(),
        job_id: job_id.to_string(),
        status: ResultStatus::Failure,
        repos: Vec::new(),
        verdict: None,
        body: None,
        children: Vec::new(),
        failure: Some(Failure {
            class,
            message: message.to_string(),
        }),
        summary: Some(summary.to_string()),
        details: None,
    }
}

fn success_result(worker_id: &str, job_id: &str) -> JobResult {
    job_result(
        worker_id,
        job_id,
        vec![RepoOutcome {
            repo: "ai/temper".to_string(),
            branch: Branch {
                name: "agent/pr-for-code-114".to_string(),
                head_sha: "abc123".to_string(),
            },
        }],
    )
}

fn artifact() -> Artifact {
    Artifact {
        item: json!(114),
        kind: "issue".to_string(),
    }
}

async fn post(
    client: &temper_io_engine::http::JsonClient,
    url: &str,
    msg: &WorkerProtocolMessage,
) -> temper_io_engine::http::HttpResponseData {
    client
        .send(
            "POST",
            url,
            None,
            Some(&serde_json::to_value(msg).expect("message serializes")),
        )
        .await
        .expect("post message")
}

async fn post_json(
    client: &temper_io_engine::http::JsonClient,
    url: &str,
    msg: &WorkerProtocolMessage,
) -> WorkerProtocolMessage {
    let response = post(client, url, msg).await;
    assert_eq!(response.status, 200);
    serde_json::from_slice(&response.body).expect("protocol response json")
}

fn assert_release(msg: WorkerProtocolMessage, worker_id: &str, job_id: &str) {
    match msg {
        WorkerProtocolMessage::Release(release) => {
            assert_eq!(release.worker_id, worker_id);
            assert_eq!(release.job_id, job_id);
            assert_eq!(release.disposition, ReleaseDisposition::Accepted);
        }
        other => panic!("expected release, got {other:?}"),
    }
}

fn assert_assigned(msg: WorkerProtocolMessage, job_id: &str) {
    match msg {
        WorkerProtocolMessage::Assign(assign) => assert_eq!(assign.job_id, job_id),
        other => panic!("expected assign, got {other:?}"),
    }
}

fn assignment_job_id(msg: WorkerProtocolMessage) -> String {
    match msg {
        WorkerProtocolMessage::Assign(assign) => assign.job_id,
        other => panic!("expected assign, got {other:?}"),
    }
}

fn assert_poll_timeout(msg: WorkerProtocolMessage) {
    match msg {
        WorkerProtocolMessage::Error(error) => assert_eq!(error.code, ErrorCode::PollTimeout),
        other => panic!("expected poll timeout, got {other:?}"),
    }
}

async fn enqueue_standard_job(daemon: &temper_daemon::Daemon, job_id: &str) {
    daemon
        .enqueue_job(
            job_id,
            "engineer",
            "ai/temper",
            artifact(),
            json!({"prompt":"implement", "issue":114}),
        )
        .await;
}

async fn eventually_enqueue_and_assign(
    cx: &temper_io_engine::Cx,
    daemon: &temper_daemon::Daemon,
    client: &temper_io_engine::http::JsonClient,
    url: &str,
    worker_id: &str,
    job_id: &str,
) {
    for _ in 0..20 {
        enqueue_standard_job(daemon, job_id).await;
        match post_json(client, url, &poll_with_wait(worker_id, 25)).await {
            WorkerProtocolMessage::Assign(assign) if assign.job_id == job_id => return,
            WorkerProtocolMessage::Error(error) if error.code == ErrorCode::PollTimeout => {
                temper_io_engine::runtime::sleep_for(cx, Duration::from_millis(10)).await;
            }
            other => panic!("expected assign for {job_id} or poll timeout, got {other:?}"),
        }
    }

    panic!("job {job_id} did not become dispatchable after apply finished");
}

#[test]
fn accepted_result_invokes_applier_with_in_flight_context() {
    temper_io_engine::block_on_with(move |_cx, handle| async move {
        let (daemon, url, mut rx) = spawn_recording(&handle).await;
        let client = temper_io_engine::http::JsonClient::new();
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
    temper_io_engine::block_on_with(move |_cx, handle| async move {
        let (daemon, url, mut rx) = spawn_recording(&handle).await;
        let client = temper_io_engine::http::JsonClient::new();
        assert_eq!(post(&client, &url, &register("worker-a")).await.status, 204);

        assert_release(
            post_json(
                &client,
                &url,
                &WorkerProtocolMessage::Result(job_result("worker-a", "phantom-job", Vec::new())),
            )
            .await,
            "worker-a",
            "phantom-job",
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
        assert_release(
            post_json(
                &client,
                &url,
                &WorkerProtocolMessage::Result(job_result("worker-a", "pending-job", Vec::new())),
            )
            .await,
            "worker-a",
            "pending-job",
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
fn apply_window_blocks_duplicate_enqueue_until_apply_finishes() {
    temper_io_engine::block_on_with(move |cx, handle| async move {
        let (record_tx, mut rx) = temper_io_engine::channel();
        let (release_tx, release_rx) = temper_io_engine::oneshot();
        let (daemon, url) = spawn_with_applier_and_apply_grace(
            &handle,
            Arc::new(GatedApplier::new(record_tx, vec![release_rx])),
            Duration::ZERO,
        )
        .await;
        let client = temper_io_engine::http::JsonClient::new();
        assert_eq!(post(&client, &url, &register("worker-a")).await.status, 204);

        enqueue_standard_job(&daemon, "job-apply-window-1").await;
        assert_assigned(
            post_json(&client, &url, &poll("worker-a")).await,
            "job-apply-window-1",
        );

        let result = success_result("worker-a", "job-apply-window-1");
        assert_release(
            post_json(
                &client,
                &url,
                &WorkerProtocolMessage::Result(result.clone()),
            )
            .await,
            "worker-a",
            "job-apply-window-1",
        );
        let (job, recorded_result) = rx.recv().await.expect("applier starts and parks");
        assert_eq!(job.job_id, "job-apply-window-1");
        assert_eq!(recorded_result.job_id, result.job_id);

        enqueue_standard_job(&daemon, "job-apply-window-1").await;
        assert_poll_timeout(post_json(&client, &url, &poll_with_wait("worker-a", 25)).await);

        release_tx.send(());
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
    temper_io_engine::block_on_with(move |cx, handle| async move {
        let (record_tx, mut rx) = temper_io_engine::channel();
        let (release_tx, release_rx) = temper_io_engine::oneshot();
        let (daemon, url) = spawn_with_applier_and_apply_grace(
            &handle,
            Arc::new(GatedApplier::new(record_tx, vec![release_rx])),
            Duration::from_millis(200),
        )
        .await;
        let client = temper_io_engine::http::JsonClient::new();
        assert_eq!(post(&client, &url, &register("worker-a")).await.status, 204);

        enqueue_standard_job(&daemon, "job-apply-grace-1").await;
        assert_assigned(
            post_json(&client, &url, &poll("worker-a")).await,
            "job-apply-grace-1",
        );

        let result = success_result("worker-a", "job-apply-grace-1");
        assert_release(
            post_json(
                &client,
                &url,
                &WorkerProtocolMessage::Result(result.clone()),
            )
            .await,
            "worker-a",
            "job-apply-grace-1",
        );
        let (job, recorded_result) = rx.recv().await.expect("applier starts and parks");
        assert_eq!(job.job_id, "job-apply-grace-1");
        assert_eq!(recorded_result.job_id, result.job_id);
        release_tx.send(());
        temper_io_engine::runtime::sleep_for(&cx, Duration::from_millis(25)).await;

        enqueue_standard_job(&daemon, "job-apply-grace-1").await;
        assert_poll_timeout(post_json(&client, &url, &poll_with_wait("worker-a", 25)).await);

        temper_io_engine::runtime::sleep_for(&cx, Duration::from_millis(225)).await;
        enqueue_standard_job(&daemon, "job-apply-grace-1").await;
        assert_assigned(
            post_json(&client, &url, &poll("worker-a")).await,
            "job-apply-grace-1",
        );
    })
}

#[test]
fn apply_block_and_grace_are_per_job_id() {
    temper_io_engine::block_on_with(move |cx, handle| async move {
        let (record_tx, mut rx) = temper_io_engine::channel();
        let (release_tx, release_rx) = temper_io_engine::oneshot();
        let (daemon, url) = spawn_with_applier_and_apply_grace(
            &handle,
            Arc::new(GatedApplier::new(record_tx, vec![release_rx])),
            Duration::from_millis(200),
        )
        .await;
        let client = temper_io_engine::http::JsonClient::new();
        assert_eq!(post(&client, &url, &register("worker-a")).await.status, 204);
        assert_eq!(post(&client, &url, &register("worker-b")).await.status, 204);
        assert_eq!(post(&client, &url, &register("worker-c")).await.status, 204);

        enqueue_standard_job(&daemon, "job-blocked").await;
        assert_assigned(
            post_json(&client, &url, &poll("worker-a")).await,
            "job-blocked",
        );

        let result = success_result("worker-a", "job-blocked");
        assert_release(
            post_json(
                &client,
                &url,
                &WorkerProtocolMessage::Result(result.clone()),
            )
            .await,
            "worker-a",
            "job-blocked",
        );
        let (job, recorded_result) = rx.recv().await.expect("applier starts and parks");
        assert_eq!(job.job_id, "job-blocked");
        assert_eq!(recorded_result.job_id, result.job_id);

        enqueue_standard_job(&daemon, "job-blocked").await;
        enqueue_standard_job(&daemon, "job-independent-apply").await;
        assert_assigned(
            post_json(&client, &url, &poll("worker-b")).await,
            "job-independent-apply",
        );

        release_tx.send(());
        temper_io_engine::runtime::sleep_for(&cx, Duration::from_millis(25)).await;
        assert!(rx.try_recv().is_none());

        enqueue_standard_job(&daemon, "job-blocked").await;
        enqueue_standard_job(&daemon, "job-independent-grace").await;
        assert_assigned(
            post_json(&client, &url, &poll("worker-c")).await,
            "job-independent-grace",
        );
    })
}

#[test]
fn transient_failure_drops_job_for_rescan() {
    temper_io_engine::block_on_with(move |_cx, handle| async move {
        let (daemon, url, mut rx) = spawn_recording(&handle).await;
        let client = temper_io_engine::http::JsonClient::new();
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

        assert_poll_timeout(post_json(&client, &url, &poll_with_wait("worker-a", 25)).await);
        assert!(rx.try_recv().is_none());

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
    temper_io_engine::block_on_with(move |cx, handle| async move {
        let (record_tx, mut rx) = temper_io_engine::channel();
        let (release_tx, release_rx) = temper_io_engine::oneshot();
        let (daemon, url) = spawn_with_applier_and_apply_grace(
            &handle,
            Arc::new(GatedApplier::new(record_tx, vec![release_rx])),
            Duration::ZERO,
        )
        .await;
        let client = temper_io_engine::http::JsonClient::new();
        assert_eq!(post(&client, &url, &register("worker-a")).await.status, 204);

        enqueue_standard_job(&daemon, "job-permanent-failure-1").await;
        assert_assigned(
            post_json(&client, &url, &poll("worker-a")).await,
            "job-permanent-failure-1",
        );

        let failure = permanent_failure_result("worker-a", "job-permanent-failure-1");
        assert_release(
            post_json(
                &client,
                &url,
                &WorkerProtocolMessage::Result(failure.clone()),
            )
            .await,
            "worker-a",
            "job-permanent-failure-1",
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
