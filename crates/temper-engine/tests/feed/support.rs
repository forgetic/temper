// SPDX-License-Identifier: MPL-2.0

pub(crate) use std::sync::Arc;

pub(crate) use serde_json::json;
pub(crate) use temper_engine::{Daemon, RoleFeedMode};
pub(crate) use temper_forge::{
    BranchRef, CreateIssue, CreatePullRequest, CreateRepository, Forge, IssueState, ItemNumber,
    MergeMethod, MergePullRequest, PullRequest, PullRequestUpdateState, RepositoryId, UpdateIssue,
    UpdatePullRequest,
};
pub(crate) use temper_forge_memory::{FaultOp, MemoryForge};
pub(crate) use temper_protocol_worker::{
    Branch, Capability, Capacity, ErrorCode, JobResult, Poll, Register, ReleaseDisposition,
    RepoOutcome, ResultStatus, WORKER_PROTOCOL_VERSION, WorkerProtocolMessage,
};
pub(crate) use temper_worker_registry::InFlightJob;
pub(crate) use temper_workflow::{
    RawWorkflowSpec, RoleId, ValidatedWorkflow, WorkflowMetadata, render_metadata_block,
};

const FIXTURE: &str = include_str!("../../../temper-workflow/fixtures/basic-delivery.json");

pub(crate) struct RecordingApplier {
    tx: temper_engine_io::CqSender<(InFlightJob, JobResult)>,
}

#[async_trait::async_trait]
impl temper_engine::ResultApplier for RecordingApplier {
    async fn apply(&self, job: InFlightJob, result: JobResult) -> temper_engine::ApplyOutcome {
        let _ = self.tx.send((job, result));
        temper_engine::ApplyOutcome::Applied
    }
}

pub(crate) fn workflow() -> ValidatedWorkflow {
    let spec: RawWorkflowSpec = serde_json::from_str(FIXTURE).expect("workflow parses");
    spec.validate().expect("workflow validates")
}

pub(crate) fn ts(value: &str) -> chrono::DateTime<chrono::Utc> {
    value.parse().expect("valid RFC 3339 timestamp")
}

pub(crate) async fn spawn_recording(
    handle: &skein::runtime::RuntimeHandle,
) -> (
    Daemon,
    String,
    temper_engine_io::CqReceiver<(InFlightJob, JobResult)>,
) {
    let (tx, rx) = temper_engine_io::channel();
    let daemon = Daemon::with_applier(
        std::sync::Arc::new(handle.clone()),
        Arc::new(RecordingApplier { tx }),
    );
    let server = temper_engine::serve(
        handle,
        &daemon,
        "127.0.0.1:0".parse().expect("loopback addr"),
    )
    .await
    .expect("bind test server");
    let addr = server.local_addr();
    (daemon, format!("http://{addr}/v1/message"), rx)
}

pub(crate) async fn new_repo(forge: &MemoryForge) -> RepositoryId {
    forge
        .create_repository(CreateRepository {
            owner: "acme".to_string(),
            name: "service".to_string(),
            default_branch: "main".to_string(),
            description: None,
        })
        .await
        .expect("repository is created")
        .id
}

pub(crate) async fn create_issue(
    forge: &MemoryForge,
    repo: &RepositoryId,
    labels: &[&str],
) -> ItemNumber {
    let issue = forge
        .create_issue(
            repo,
            CreateIssue {
                title: "ready code issue".to_string(),
                body: "Implement the queued daemon work item.".to_string(),
                labels: labels.iter().map(|label| (*label).to_string()).collect(),
                assignees: Vec::new(),
            },
        )
        .await
        .expect("issue is created");
    issue.number
}

pub(crate) async fn create_issue_record(
    forge: &MemoryForge,
    repo: &RepositoryId,
    labels: &[&str],
) -> temper_forge::Issue {
    forge
        .create_issue(
            repo,
            CreateIssue {
                title: "ready code issue".to_string(),
                body: "Implement the queued daemon work item.".to_string(),
                labels: labels.iter().map(|label| (*label).to_string()).collect(),
                assignees: Vec::new(),
            },
        )
        .await
        .expect("issue is created")
}

pub(crate) async fn create_implementation_pull_request(
    forge: &MemoryForge,
    repo: &RepositoryId,
    correlation_key: &str,
) -> PullRequest {
    forge
        .create_pull_request(
            repo,
            CreatePullRequest {
                title: "Implement queued work".to_string(),
                body: format!(
                    "Implementation PR.\n\n{}",
                    render_metadata_block(&WorkflowMetadata {
                        correlation_key: Some(correlation_key.to_string()),
                        ..WorkflowMetadata::default()
                    })
                ),
                source: BranchRef {
                    repository_id: repo.clone(),
                    branch: format!("agent/{correlation_key}"),
                },
                target: BranchRef {
                    repository_id: repo.clone(),
                    branch: "main".to_string(),
                },
                labels: vec!["implementation".to_string()],
                assignees: Vec::new(),
            },
        )
        .await
        .expect("implementation pull request is created")
}

pub(crate) fn register(worker_id: &str, role: &str, repo: &str) -> WorkerProtocolMessage {
    WorkerProtocolMessage::Register(Register {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: worker_id.to_string(),
        capabilities: vec![Capability {
            role: role.to_string(),
            repo: repo.to_string(),
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

pub(crate) fn job_result(worker_id: &str, job_id: &str) -> JobResult {
    JobResult {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: worker_id.to_string(),
        job_id: job_id.to_string(),
        attempt_id: None,
        status: ResultStatus::Success,
        repos: vec![RepoOutcome {
            repo: "acme/service".to_string(),
            branch: Branch {
                name: "agent/pr-for-code-feed".to_string(),
                head_sha: "feed123".to_string(),
            },
        }],
        verdict: None,
        title: None,
        body: None,
        children: Vec::new(),
        failure: None,
        summary: Some("done".to_string()),
        details: Some(json!({"note":"fake worker result"})),
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

pub(crate) fn assert_poll_timeout(msg: WorkerProtocolMessage) {
    match msg {
        WorkerProtocolMessage::Error(error) => assert_eq!(error.code, ErrorCode::PollTimeout),
        other => panic!("expected poll timeout, got {other:?}"),
    }
}
