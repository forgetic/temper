// SPDX-License-Identifier: MPL-2.0

pub(crate) use std::{
    sync::{Arc, Mutex as StdMutex},
    time::Duration,
};

pub(crate) use serde_json::json;
pub(crate) use temper_protocol_worker::{
    Artifact, Branch, Capability, Capacity, ErrorCode, Failure, FailureClass, JobResult, Poll,
    Register, ReleaseDisposition, RepoOutcome, ResultStatus, WORKER_PROTOCOL_VERSION,
    WorkerProtocolMessage,
};
pub(crate) use temper_worker_registry::InFlightJob;

pub(crate) struct RecordingApplier {
    tx: temper_engine_io::CqSender<(InFlightJob, JobResult)>,
}

pub(crate) struct GatedApplier {
    tx: temper_engine_io::CqSender<(InFlightJob, JobResult)>,
    releases: StdMutex<Vec<temper_engine_io::OneshotReceiver<()>>>,
}

impl GatedApplier {
    pub(crate) fn new(
        tx: temper_engine_io::CqSender<(InFlightJob, JobResult)>,
        releases: Vec<temper_engine_io::OneshotReceiver<()>>,
    ) -> Self {
        Self {
            tx,
            releases: StdMutex::new(releases),
        }
    }
}

#[async_trait::async_trait]
impl temper_engine::ResultApplier for GatedApplier {
    async fn apply(&self, job: InFlightJob, result: JobResult) -> temper_engine::ApplyOutcome {
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
        temper_engine::ApplyOutcome::Applied
    }
}

#[async_trait::async_trait]
impl temper_engine::ResultApplier for RecordingApplier {
    async fn apply(&self, job: InFlightJob, result: JobResult) -> temper_engine::ApplyOutcome {
        let _ = self.tx.send((job, result));
        temper_engine::ApplyOutcome::Applied
    }
}

pub(crate) async fn spawn_with_applier(
    handle: &skein::runtime::RuntimeHandle,
    applier: Arc<dyn temper_engine::ResultApplier>,
) -> (temper_engine::Daemon, String) {
    spawn_daemon(
        handle,
        temper_engine::Daemon::with_applier(Arc::new(handle.clone()), applier),
    )
    .await
}

pub(crate) async fn spawn_with_applier_and_apply_grace(
    handle: &skein::runtime::RuntimeHandle,
    applier: Arc<dyn temper_engine::ResultApplier>,
    apply_grace: Duration,
) -> (temper_engine::Daemon, String) {
    spawn_daemon(
        handle,
        temper_engine::Daemon::with_applier(Arc::new(handle.clone()), applier)
            .with_apply_grace(apply_grace),
    )
    .await
}

pub(crate) async fn spawn_daemon(
    handle: &skein::runtime::RuntimeHandle,
    daemon: temper_engine::Daemon,
) -> (temper_engine::Daemon, String) {
    let server = temper_engine::serve(
        handle,
        &daemon,
        "127.0.0.1:0".parse().expect("loopback addr"),
    )
    .await
    .expect("bind test server");
    let addr = server.local_addr();
    (daemon, format!("http://{addr}/v1/message"))
}

pub(crate) async fn spawn_recording(
    handle: &skein::runtime::RuntimeHandle,
) -> (
    temper_engine::Daemon,
    String,
    temper_engine_io::CqReceiver<(InFlightJob, JobResult)>,
) {
    let (tx, rx) = temper_engine_io::channel();
    let (daemon, url) = spawn_with_applier(handle, Arc::new(RecordingApplier { tx })).await;
    (daemon, url, rx)
}

pub(crate) async fn spawn_recording_with_apply_grace(
    handle: &skein::runtime::RuntimeHandle,
    apply_grace: Duration,
) -> (
    temper_engine::Daemon,
    String,
    temper_engine_io::CqReceiver<(InFlightJob, JobResult)>,
) {
    let (tx, rx) = temper_engine_io::channel();
    let (daemon, url) =
        spawn_with_applier_and_apply_grace(handle, Arc::new(RecordingApplier { tx }), apply_grace)
            .await;
    (daemon, url, rx)
}

pub(crate) fn register(worker_id: &str) -> WorkerProtocolMessage {
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
        worker_pool: None,
        labels: None,
    })
}

pub(crate) fn poll_with_wait(worker_id: &str, max_wait_ms: u64) -> WorkerProtocolMessage {
    WorkerProtocolMessage::Poll(Poll {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: worker_id.to_string(),
        free_capacity: 1,
        max_wait_ms: Some(max_wait_ms),
    })
}

pub(crate) fn poll(worker_id: &str) -> WorkerProtocolMessage {
    poll_with_wait(worker_id, 30_000)
}

pub(crate) fn job_result(worker_id: &str, job_id: &str, repos: Vec<RepoOutcome>) -> JobResult {
    JobResult {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: worker_id.to_string(),
        job_id: job_id.to_string(),
        status: ResultStatus::Success,
        repos,
        verdict: None,
        title: None,
        body: None,
        children: Vec::new(),
        failure: None,
        summary: Some("done".to_string()),
        details: None,
    }
}

pub(crate) fn transient_failure_result(worker_id: &str, job_id: &str) -> JobResult {
    failure_result(
        worker_id,
        job_id,
        FailureClass::Transient,
        "temporary worker failure",
        "transient failure",
    )
}

pub(crate) fn permanent_failure_result(worker_id: &str, job_id: &str) -> JobResult {
    failure_result(
        worker_id,
        job_id,
        FailureClass::Permanent,
        "worker could not complete the job",
        "permanent failure",
    )
}

pub(crate) fn failure_result(
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
        title: None,
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

pub(crate) fn success_result(worker_id: &str, job_id: &str) -> JobResult {
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

pub(crate) fn artifact() -> Artifact {
    Artifact {
        item: json!(114),
        kind: "issue".to_string(),
    }
}

pub(crate) async fn post(
    client: &temper_engine_io::http::JsonClient,
    url: &str,
    msg: &WorkerProtocolMessage,
) -> temper_engine_io::http::HttpResponseData {
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

pub(crate) async fn post_json(
    client: &temper_engine_io::http::JsonClient,
    url: &str,
    msg: &WorkerProtocolMessage,
) -> WorkerProtocolMessage {
    let response = post(client, url, msg).await;
    assert_eq!(response.status, 200);
    serde_json::from_slice(&response.body).expect("protocol response json")
}

pub(crate) fn assert_release(msg: WorkerProtocolMessage, worker_id: &str, job_id: &str) {
    match msg {
        WorkerProtocolMessage::Release(release) => {
            assert_eq!(release.worker_id, worker_id);
            assert_eq!(release.job_id, job_id);
            assert_eq!(release.disposition, ReleaseDisposition::Accepted);
        }
        other => panic!("expected release, got {other:?}"),
    }
}

pub(crate) fn assert_assigned(msg: WorkerProtocolMessage, job_id: &str) {
    match msg {
        WorkerProtocolMessage::Assign(assign) => assert_eq!(assign.job_id, job_id),
        other => panic!("expected assign, got {other:?}"),
    }
}

pub(crate) fn assignment_job_id(msg: WorkerProtocolMessage) -> String {
    match msg {
        WorkerProtocolMessage::Assign(assign) => assign.job_id,
        other => panic!("expected assign, got {other:?}"),
    }
}

pub(crate) fn assert_poll_timeout(msg: WorkerProtocolMessage) {
    match msg {
        WorkerProtocolMessage::Error(error) => assert_eq!(error.code, ErrorCode::PollTimeout),
        other => panic!("expected poll timeout, got {other:?}"),
    }
}

pub(crate) async fn enqueue_standard_job(daemon: &temper_engine::Daemon, job_id: &str) {
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

pub(crate) async fn eventually_enqueue_and_assign(
    cx: &temper_engine_io::Cx,
    daemon: &temper_engine::Daemon,
    client: &temper_engine_io::http::JsonClient,
    url: &str,
    worker_id: &str,
    job_id: &str,
) {
    for _ in 0..20 {
        enqueue_standard_job(daemon, job_id).await;
        match post_json(client, url, &poll_with_wait(worker_id, 25)).await {
            WorkerProtocolMessage::Assign(assign) if assign.job_id == job_id => return,
            WorkerProtocolMessage::Error(error) if error.code == ErrorCode::PollTimeout => {
                temper_engine_io::runtime::sleep_for(cx, Duration::from_millis(10)).await;
            }
            other => panic!("expected assign for {job_id} or poll timeout, got {other:?}"),
        }
    }

    panic!("job {job_id} did not become dispatchable after apply finished");
}
