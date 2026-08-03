// SPDX-License-Identifier: MPL-2.0

use super::*;
use std::sync::Arc;
use temper_forge::{CiJob, CiJobConclusion, CiJobId, CiJobStatus, MergeMethod, MergePullRequest};
use temper_forge_memory::FaultOp;
use temper_protocol_worker::{
    Capability, Capacity, ErrorCode, JobResult, Poll, Register, ResultStatus,
    WORKER_PROTOCOL_VERSION, WorkerProtocolMessage,
};

fn basic_workflow() -> (ValidatedWorkflow, CompiledWorkflow) {
    let workflow: RawWorkflowSpec =
        serde_json::from_str(BASIC_DELIVERY_FIXTURE).expect("basic-delivery workflow parses");
    let workflow = workflow
        .validate()
        .expect("basic-delivery workflow validates");
    let compiled = workflow.compile();
    (workflow, compiled)
}

async fn create_ai_temper_repo(forge: &MemoryForge) -> RepositoryId {
    forge
        .create_repository(CreateRepository {
            owner: "ai".to_string(),
            name: "temper".to_string(),
            default_branch: "main".to_string(),
            description: None,
        })
        .await
        .expect("repository is created")
        .id
}

async fn create_ready_issue(forge: &MemoryForge, repo: &RepositoryId) -> ItemNumber {
    forge
        .create_issue(
            repo,
            temper_forge::CreateIssue {
                title: "ready".to_string(),
                body: "needs implementation".to_string(),
                labels: vec!["code".to_string(), "ready".to_string()],
                assignees: Vec::new(),
            },
        )
        .await
        .expect("issue is created")
        .number
}

async fn create_implementation_pr(
    forge: &MemoryForge,
    repo: &RepositoryId,
) -> temper_forge::PullRequest {
    forge
        .create_pull_request(
            repo,
            CreatePullRequest {
                title: "Implement change".to_string(),
                body: "Implementation PR.".to_string(),
                source: BranchRef {
                    repository_id: repo.clone(),
                    branch: "agent/pr-for-code-1".to_string(),
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
        .expect("pull request is created")
}

fn seed_pr_ci(
    forge: &MemoryForge,
    repo: &RepositoryId,
    pull_request: &temper_forge::PullRequest,
    status: CiJobStatus,
    conclusion: Option<CiJobConclusion>,
) {
    forge.seed_ci_jobs(
        repo,
        vec![CiJob {
            id: CiJobId::new(format!("ci-{}", pull_request.number.get())),
            repo_id: repo.clone(),
            pull_request_id: Some(pull_request.id.clone()),
            commit_sha: pull_request.head_sha.clone().unwrap_or_default(),
            name: "validate".to_string(),
            status,
            conclusion,
            provider_conclusion: None,
            provider_reason: None,
            run_id: None,
            attempt: None,
            verified_failure: None,
            url: None,
            created_at: chrono::DateTime::<chrono::Utc>::from_timestamp(1, 0).unwrap(),
            started_at: None,
            completed_at: None,
            updated_at: chrono::DateTime::<chrono::Utc>::from_timestamp(1, 0).unwrap(),
        }],
    );
}

fn register_engineer(worker_id: &str) -> WorkerProtocolMessage {
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

fn poll_worker(worker_id: &str) -> WorkerProtocolMessage {
    WorkerProtocolMessage::Poll(Poll {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: worker_id.to_string(),
        free_capacity: 1,
        max_wait_ms: Some(0),
    })
}

thread_local! {
    static ASSIGNMENT_ATTEMPTS: std::cell::RefCell<std::collections::BTreeMap<String, Option<String>>> =
        const { std::cell::RefCell::new(std::collections::BTreeMap::new()) };
}

fn success_result(worker_id: &str, job_id: &str) -> WorkerProtocolMessage {
    let attempt_id =
        ASSIGNMENT_ATTEMPTS.with(|attempts| attempts.borrow().get(job_id).cloned().flatten());
    WorkerProtocolMessage::Result(JobResult {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: worker_id.to_string(),
        job_id: job_id.to_string(),
        attempt_id,
        status: ResultStatus::Success,
        repos: Vec::new(),
        verdict: None,
        title: None,
        body: None,
        children: Vec::new(),
        failure: None,
        summary: None,
        details: None,
    })
}

fn assigned_job(reply: Option<WorkerProtocolMessage>) -> temper_protocol_worker::Assign {
    match reply {
        Some(WorkerProtocolMessage::Assign(assign)) => assign,
        other => panic!("expected assignment, got {other:?}"),
    }
}

fn assigned_job_id(reply: Option<WorkerProtocolMessage>) -> String {
    let assign = assigned_job(reply);
    ASSIGNMENT_ATTEMPTS.with(|attempts| {
        attempts
            .borrow_mut()
            .insert(assign.job_id.clone(), assign.attempt_id.clone());
    });
    assign.job_id
}

fn assert_poll_timeout(reply: Option<WorkerProtocolMessage>) {
    match reply {
        Some(WorkerProtocolMessage::Error(error)) => {
            assert_eq!(error.code, ErrorCode::PollTimeout);
        }
        other => panic!("expected poll timeout, got {other:?}"),
    }
}

fn scan_now() -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::<chrono::Utc>::from_timestamp(10, 0).unwrap()
}

#[test]
fn enqueue_scanned_role_work_prunes_stale_pr_and_preserves_current_work() {
    temper_engine_io::block_on_with(move |_cx, handle| async move {
        let daemon = Daemon::new(Arc::new(handle));
        let forge = MemoryForge::new();
        let repo = create_ai_temper_repo(&forge).await;
        let (workflow, compiled) = basic_workflow();
        let role = RoleId::new("engineer");

        assert_eq!(
            daemon
                .deliver_protocol_message(register_engineer("engineer-1"))
                .await
                .expect("register succeeds"),
            None
        );
        daemon
            .enqueue_job(
                "busy-job",
                "engineer",
                "ai/temper",
                Artifact {
                    item: json!(900),
                    kind: "issue".to_string(),
                },
                json!({"busy": true}),
            )
            .await;
        assert_eq!(
            assigned_job_id(
                daemon
                    .deliver_protocol_message(poll_worker("engineer-1"))
                    .await
                    .expect("poll succeeds")
            ),
            "busy-job"
        );

        let pull_request = create_implementation_pr(&forge, &repo).await;
        seed_pr_ci(
            &forge,
            &repo,
            &pull_request,
            CiJobStatus::Completed,
            Some(CiJobConclusion::Failure),
        );

        assert_eq!(
            daemon
                .enqueue_scanned_role_work(
                    &forge,
                    &repo,
                    &workflow,
                    &compiled,
                    scan_now(),
                    &role,
                    RoleFeedMode::Normal,
                )
                .await
                .expect("first scan succeeds"),
            1
        );
        let stale_pr_job = format!(
            "ai/temper/pull_request-{}/engineer/pr_ci_failed",
            pull_request.number.get()
        );
        assert_eq!(
            daemon
                .queued_jobs()
                .await
                .iter()
                .map(|job| job.job_id.as_str())
                .collect::<Vec<_>>(),
            vec![stale_pr_job.as_str()]
        );

        let issue_number = create_ready_issue(&forge, &repo).await;
        seed_pr_ci(
            &forge,
            &repo,
            &pull_request,
            CiJobStatus::Completed,
            Some(CiJobConclusion::Success),
        );

        assert_eq!(
            daemon
                .enqueue_scanned_role_work(
                    &forge,
                    &repo,
                    &workflow,
                    &compiled,
                    scan_now(),
                    &role,
                    RoleFeedMode::Normal,
                )
                .await
                .expect("second scan succeeds"),
            1
        );
        let current_issue_job =
            format!("ai/temper/issue-{}/engineer/code_ready", issue_number.get());
        assert_eq!(
            daemon
                .queued_jobs()
                .await
                .iter()
                .map(|job| job.job_id.as_str())
                .collect::<Vec<_>>(),
            vec![current_issue_job.as_str()]
        );

        daemon
            .deliver_protocol_message(success_result("engineer-1", "busy-job"))
            .await
            .expect("result succeeds");
        assert_eq!(
            assigned_job_id(
                daemon
                    .deliver_protocol_message(poll_worker("engineer-1"))
                    .await
                    .expect("poll succeeds")
            ),
            current_issue_job
        );
    })
}

#[test]
fn enqueue_scanned_role_work_prunes_pending_pr_after_merge_before_assignment() {
    temper_engine_io::block_on_with(move |_cx, handle| async move {
        let daemon = Daemon::new(Arc::new(handle));
        let forge = MemoryForge::new();
        let repo = create_ai_temper_repo(&forge).await;
        let (workflow, compiled) = basic_workflow();
        let role = RoleId::new("engineer");

        daemon
            .deliver_protocol_message(register_engineer("engineer-1"))
            .await
            .expect("register succeeds");
        daemon
            .enqueue_job(
                "busy-job",
                "engineer",
                "ai/temper",
                Artifact {
                    item: json!(901),
                    kind: "issue".to_string(),
                },
                json!({"busy": true}),
            )
            .await;
        assert_eq!(
            assigned_job_id(
                daemon
                    .deliver_protocol_message(poll_worker("engineer-1"))
                    .await
                    .expect("poll succeeds")
            ),
            "busy-job"
        );

        let pull_request = create_implementation_pr(&forge, &repo).await;
        seed_pr_ci(
            &forge,
            &repo,
            &pull_request,
            CiJobStatus::Completed,
            Some(CiJobConclusion::Failure),
        );
        assert_eq!(
            daemon
                .enqueue_scanned_role_work(
                    &forge,
                    &repo,
                    &workflow,
                    &compiled,
                    scan_now(),
                    &role,
                    RoleFeedMode::Normal,
                )
                .await
                .expect("first scan succeeds"),
            1
        );
        assert_eq!(daemon.queued_jobs().await.len(), 1);

        forge
            .merge_pull_request(
                &pull_request.id,
                MergePullRequest {
                    method: MergeMethod::Squash,
                    commit_title: None,
                    commit_body: None,
                    delete_source_branch: false,
                },
            )
            .await
            .expect("pull request merges");
        assert_eq!(
            daemon
                .enqueue_scanned_role_work(
                    &forge,
                    &repo,
                    &workflow,
                    &compiled,
                    scan_now(),
                    &role,
                    RoleFeedMode::Normal,
                )
                .await
                .expect("second scan succeeds"),
            0
        );
        assert!(daemon.queued_jobs().await.is_empty());

        daemon
            .deliver_protocol_message(success_result("engineer-1", "busy-job"))
            .await
            .expect("result succeeds");
        assert_poll_timeout(
            daemon
                .deliver_protocol_message(poll_worker("engineer-1"))
                .await
                .expect("poll succeeds"),
        );
    })
}

#[test]
fn scan_level_error_does_not_reconcile_pending_jobs() {
    temper_engine_io::block_on_with(move |_cx, handle| async move {
        let daemon = Daemon::new(Arc::new(handle));
        let forge = MemoryForge::new();
        let repo = create_ai_temper_repo(&forge).await;
        let (workflow, compiled) = basic_workflow();
        let role = RoleId::new("engineer");

        daemon
            .enqueue_job(
                "ai/temper/pull_request-9/engineer/pr_ci_failed",
                "engineer",
                "ai/temper",
                Artifact {
                    item: json!(9),
                    kind: "pull_request".to_string(),
                },
                json!({"stale_if_scan_succeeds": true}),
            )
            .await;
        assert_eq!(daemon.queued_jobs().await.len(), 1);

        forge.fail_next(FaultOp::ListIssues, "scan is unavailable");
        assert!(
            daemon
                .enqueue_scanned_role_work(
                    &forge,
                    &repo,
                    &workflow,
                    &compiled,
                    scan_now(),
                    &role,
                    RoleFeedMode::Normal,
                )
                .await
                .is_err()
        );

        assert_eq!(
            daemon
                .queued_jobs()
                .await
                .iter()
                .map(|job| job.job_id.as_str())
                .collect::<Vec<_>>(),
            vec!["ai/temper/pull_request-9/engineer/pr_ci_failed"]
        );
    })
}
