//! Unit tests for backend-agnostic multi-repo worker wrappers.

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Wake, Waker};
use temper_forge::{
    BranchRef, ChangeHint, ChangeKind, CreateIssue, CreatePullRequest, CreateRepository, Forge,
    IssueState, ItemNumber, PullRequestQuery, RepositoryId, RepositoryPath, UpdateIssue, UserId,
};
use temper_forge_memory::{FaultOp, MemoryForge};
use temper_runner::{
    Agent, AgentError, MultiRepoMechanicalWorker, MultiRepoRoleWorker, Progress, RepositoryJournal,
    RepositorySet, RepositoryTarget, RoleTools, WorkItem,
};
use temper_workflow::{
    ArtifactKindId, ExecutionContext, InMemoryJournal, LeasePolicy, QueueId, RawWorkflowSpec,
    RoleId, TransitionId,
};

const REFERENCE: &str = include_str!("../../temper-workflow/fixtures/reference-delivery.json");

const COMMENT_PR_WORKFLOW: &str = r#"
{
  "name": "comment-pr-worker",
  "roles": [{"id": "engineer", "queues": ["code_ready"]}],
  "labels": [
    {"id": "code"},
    {"id": "ready"},
    {"id": "in-progress"},
    {"id": "implementation"}
  ],
  "artifact_kinds": [
    {"id": "code", "target": "issue", "identifying_labels": ["code"]},
    {"id": "implementation_pr", "target": "pull_request", "identifying_labels": ["implementation"]}
  ],
  "queues": [{"id": "code_ready", "artifact": "code", "labels": ["ready"]}],
  "transitions": [
    {"id": "claim_with_note", "artifact": "code", "roles": ["engineer"], "effects": [
      {"kind": "remove_label", "label": "ready"},
      {"kind": "add_label", "label": "in-progress"},
      {"kind": "create_comment", "body": "Claimed for implementation."}
    ]}
  ]
}
"#;

struct NoopWake;

impl Wake for NoopWake {
    fn wake(self: Arc<Self>) {}
}

fn block_on<F: Future>(future: F) -> F::Output {
    let waker = Waker::from(Arc::new(NoopWake));
    let mut context = Context::from_waker(&waker);
    let mut future = Box::pin(future);
    match Future::poll(future.as_mut(), &mut context) {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("in-memory forge futures should not park in tests"),
    }
}

fn ts(value: &str) -> DateTime<Utc> {
    value.parse().expect("valid RFC 3339 timestamp")
}

fn workflow() -> temper_workflow::ValidatedWorkflow {
    let spec: RawWorkflowSpec = serde_json::from_str(REFERENCE).expect("fixture parses");
    spec.validate().expect("reference fixture validates")
}

fn comment_pr_workflow() -> temper_workflow::ValidatedWorkflow {
    let spec: RawWorkflowSpec = serde_json::from_str(COMMENT_PR_WORKFLOW).expect("fixture parses");
    spec.validate().expect("comment fixture validates")
}

fn repo_input(name: &str) -> CreateRepository {
    CreateRepository {
        owner: "acme".into(),
        name: name.into(),
        default_branch: "main".into(),
        description: None,
    }
}

fn create_repo(forge: &MemoryForge, name: &str) -> RepositoryTarget {
    let repo = block_on(forge.create_repository(repo_input(name))).expect("repository created");
    RepositoryTarget::new(repo.id, RepositoryPath::new(repo.owner, repo.name))
}

fn create_issue(forge: &MemoryForge, repo: &RepositoryId, labels: &[&str]) -> ItemNumber {
    block_on(forge.create_issue(
        repo,
        CreateIssue {
            title: "work".into(),
            body: String::new(),
            labels: labels.iter().map(|label| (*label).to_string()).collect(),
            assignees: Vec::new(),
        },
    ))
    .expect("issue created")
    .number
}

fn close_issue(forge: &MemoryForge, repo: &RepositoryId, number: ItemNumber) {
    let issue = block_on(forge.get_issue_by_number(repo, number))
        .expect("lookup succeeds")
        .expect("issue exists");
    block_on(forge.update_issue(
        &issue.id,
        UpdateIssue {
            state: Some(IssueState::Closed),
            ..UpdateIssue::default()
        },
    ))
    .expect("issue closes");
}

fn add_issue_dependency(
    forge: &MemoryForge,
    repo: &RepositoryId,
    source: ItemNumber,
    target: ItemNumber,
) {
    let issue = block_on(forge.get_issue_by_number(repo, source))
        .expect("lookup succeeds")
        .expect("issue exists");
    block_on(forge.add_issue_dependency(&issue.id, target)).expect("dependency link added");
}

fn issue_labels(forge: &MemoryForge, repo: &RepositoryId, number: ItemNumber) -> Vec<String> {
    let mut labels = block_on(forge.get_issue_by_number(repo, number))
        .expect("lookup succeeds")
        .expect("issue exists")
        .labels;
    labels.sort();
    labels
}

fn lease_policy() -> LeasePolicy {
    LeasePolicy::new(Duration::minutes(30))
}

struct TriageToCode;

#[async_trait]
impl Agent<MemoryForge> for TriageToCode {
    async fn service(
        &self,
        item: &WorkItem,
        tools: &RoleTools<'_, MemoryForge>,
    ) -> Result<bool, AgentError> {
        if item.queue == QueueId::new("design_triage") && item.kind == ArtifactKindId::new("intake")
        {
            tools
                .run(item.target, &TransitionId::new("triage_to_code"))
                .await?;
            return Ok(true);
        }
        Ok(false)
    }
}

struct ClaimReady;

#[async_trait]
impl Agent<MemoryForge> for ClaimReady {
    async fn service(
        &self,
        item: &WorkItem,
        tools: &RoleTools<'_, MemoryForge>,
    ) -> Result<bool, AgentError> {
        if item.queue == QueueId::new("code_ready") && item.kind == ArtifactKindId::new("code") {
            tools
                .run(item.target, &TransitionId::new("claim_code"))
                .await?;
            return Ok(true);
        }
        Ok(false)
    }
}

struct ClaimCommentAndPr;

#[async_trait]
impl Agent<MemoryForge> for ClaimCommentAndPr {
    async fn service(
        &self,
        item: &WorkItem,
        tools: &RoleTools<'_, MemoryForge>,
    ) -> Result<bool, AgentError> {
        if item.queue != QueueId::new("code_ready") || item.kind != ArtifactKindId::new("code") {
            return Ok(false);
        }
        tools
            .run(item.target, &TransitionId::new("claim_with_note"))
            .await?;
        let branch = format!("feature-{}", tools.repo());
        tools
            .open_pull_request(
                &format!("pr:{}:{:?}", tools.repo(), item.target),
                CreatePullRequest {
                    title: "implementation".into(),
                    body: "opened by test".into(),
                    source: BranchRef {
                        repository_id: tools.repo().clone(),
                        branch,
                    },
                    target: BranchRef {
                        repository_id: tools.repo().clone(),
                        branch: "main".into(),
                    },
                    labels: vec!["implementation".into()],
                    assignees: Vec::new(),
                },
            )
            .await?;
        Ok(true)
    }
}

struct RecordingAgent {
    seen: Arc<Mutex<Vec<RepositoryId>>>,
}

#[async_trait]
impl Agent<MemoryForge> for RecordingAgent {
    async fn service(
        &self,
        _item: &WorkItem,
        tools: &RoleTools<'_, MemoryForge>,
    ) -> Result<bool, AgentError> {
        self.seen
            .lock()
            .expect("recording mutex")
            .push(tools.repo().clone());
        Ok(false)
    }
}

#[test]
fn role_worker_triages_intake_to_code_in_two_repos() {
    let forge = MemoryForge::new();
    let repo_a = create_repo(&forge, "a");
    let repo_b = create_repo(&forge, "b");
    let issue_a = create_issue(&forge, &repo_a.id, &["untriaged"]);
    let issue_b = create_issue(&forge, &repo_b.id, &["untriaged"]);
    let workflow = workflow();
    let compiled = workflow.compile();
    let worker = MultiRepoRoleWorker::new(
        &workflow,
        &compiled,
        &forge,
        RepositorySet::new(vec![repo_b.clone(), repo_a.clone()]),
        RoleId::new("architect"),
        Arc::new(TriageToCode),
        ExecutionContext::new(),
    );

    let report = block_on(worker.tick_report(ts("2026-05-29T00:00:00Z")));

    assert!(report.failures.is_empty());
    assert_eq!(
        report.progress,
        Progress {
            changed: true,
            actions: 2
        }
    );
    assert_eq!(
        issue_labels(&forge, &repo_a.id, issue_a),
        vec!["code".to_string(), "ready".to_string()]
    );
    assert_eq!(
        issue_labels(&forge, &repo_b.id, issue_b),
        vec!["code".to_string(), "ready".to_string()]
    );
}

#[test]
fn mechanical_worker_unblocks_independently_in_two_repos() {
    let forge = MemoryForge::new();
    let repo_a = create_repo(&forge, "a");
    let repo_b = create_repo(&forge, "b");
    let dep_a = create_issue(&forge, &repo_a.id, &["code", "ready"]);
    let dep_b = create_issue(&forge, &repo_b.id, &["code", "ready"]);
    close_issue(&forge, &repo_a.id, dep_a);
    close_issue(&forge, &repo_b.id, dep_b);
    let blocked_a = create_issue(&forge, &repo_a.id, &["code", "blocked"]);
    let blocked_b = create_issue(&forge, &repo_b.id, &["code", "blocked"]);
    add_issue_dependency(&forge, &repo_a.id, blocked_a, dep_a);
    add_issue_dependency(&forge, &repo_b.id, blocked_b, dep_b);
    let workflow = workflow();
    let journal_a = InMemoryJournal::new();
    let journal_b = InMemoryJournal::new();
    let worker = MultiRepoMechanicalWorker::new(
        &workflow,
        &forge,
        RepositorySet::new(vec![repo_b.clone(), repo_a.clone()]),
        vec![
            RepositoryJournal {
                repository: &repo_b.id,
                journal: &journal_b,
            },
            RepositoryJournal {
                repository: &repo_a.id,
                journal: &journal_a,
            },
        ],
        lease_policy(),
    )
    .expect("worker builds");

    let report = block_on(worker.tick_report(ts("2026-05-29T00:00:00Z")));

    assert!(report.failures.is_empty());
    assert_eq!(
        report.progress,
        Progress {
            changed: true,
            actions: 2
        }
    );
    assert_eq!(
        issue_labels(&forge, &repo_a.id, blocked_a),
        vec!["code".to_string(), "ready".to_string()]
    );
    assert_eq!(
        issue_labels(&forge, &repo_b.id, blocked_b),
        vec!["code".to_string(), "ready".to_string()]
    );
}

#[test]
fn labels_comments_and_prs_do_not_leak_between_repos() {
    let forge = MemoryForge::new();
    let repo_a = create_repo(&forge, "a");
    let repo_b = create_repo(&forge, "b");
    let issue_a = create_issue(&forge, &repo_a.id, &["code", "ready"]);
    let issue_b = create_issue(&forge, &repo_b.id, &["code"]);
    let workflow = comment_pr_workflow();
    let compiled = workflow.compile();
    let worker = MultiRepoRoleWorker::new(
        &workflow,
        &compiled,
        &forge,
        RepositorySet::new(vec![repo_a.clone(), repo_b.clone()]),
        RoleId::new("engineer"),
        Arc::new(ClaimCommentAndPr),
        ExecutionContext::new(),
    );

    let report = block_on(worker.tick_report(ts("2026-05-29T00:00:00Z")));

    assert!(report.failures.is_empty());
    assert_eq!(
        issue_labels(&forge, &repo_a.id, issue_a),
        vec!["code", "in-progress"]
    );
    assert_eq!(issue_labels(&forge, &repo_b.id, issue_b), vec!["code"]);

    let issue_a = block_on(forge.get_issue_by_number(&repo_a.id, issue_a))
        .unwrap()
        .unwrap();
    let issue_b = block_on(forge.get_issue_by_number(&repo_b.id, issue_b))
        .unwrap()
        .unwrap();
    assert_eq!(
        block_on(forge.list_issue_comments(&issue_a.id))
            .unwrap()
            .len(),
        1
    );
    assert!(
        block_on(forge.list_issue_comments(&issue_b.id))
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        block_on(forge.list_pull_requests(&repo_a.id, PullRequestQuery::default()))
            .unwrap()
            .len(),
        1
    );
    assert!(
        block_on(forge.list_pull_requests(&repo_b.id, PullRequestQuery::default()))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn backend_error_in_one_repo_does_not_stop_remaining_repos() {
    let forge = MemoryForge::new();
    let repo_a = create_repo(&forge, "a");
    let repo_b = create_repo(&forge, "b");
    create_issue(&forge, &repo_a.id, &["code", "ready"]);
    let issue_b = create_issue(&forge, &repo_b.id, &["code", "ready"]);
    let workflow = workflow();
    let compiled = workflow.compile();
    let worker = MultiRepoRoleWorker::new(
        &workflow,
        &compiled,
        &forge,
        RepositorySet::new(vec![repo_a.clone(), repo_b.clone()]),
        RoleId::new("engineer"),
        Arc::new(ClaimReady),
        ExecutionContext::new().with_assignee(RoleId::new("engineer"), UserId::new("user-1")),
    );
    forge.fail_next(FaultOp::ListIssues, "repo a temporarily unavailable");

    let report = block_on(worker.tick_report(ts("2026-05-29T00:00:00Z")));

    assert_eq!(report.failures.len(), 1);
    assert_eq!(report.failures[0].repository, repo_a);
    assert_eq!(
        report.progress,
        Progress {
            changed: true,
            actions: 1
        }
    );
    assert_eq!(
        issue_labels(&forge, &repo_b.id, issue_b),
        vec!["code".to_string(), "in-progress".to_string()]
    );
}

#[test]
fn role_worker_hint_matching_ticks_only_the_hinted_repository() {
    let forge = MemoryForge::new();
    let repo_a = create_repo(&forge, "a");
    let repo_b = create_repo(&forge, "b");
    let repo_c = create_repo(&forge, "c");
    for repo in [&repo_a, &repo_b, &repo_c] {
        create_issue(&forge, &repo.id, &["code", "ready"]);
    }
    let workflow = workflow();
    let compiled = workflow.compile();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let worker = MultiRepoRoleWorker::new(
        &workflow,
        &compiled,
        &forge,
        RepositorySet::new(vec![repo_a.clone(), repo_b.clone(), repo_c.clone()]),
        RoleId::new("engineer"),
        Arc::new(RecordingAgent {
            seen: Arc::clone(&seen),
        }),
        ExecutionContext::new(),
    );

    let hint = ChangeHint::repo(RepositoryPath::new("acme", "b"), ChangeKind::Issue);
    let report = block_on(worker.tick_matching_hints(ts("2026-05-29T00:00:00Z"), &[hint]));

    assert!(report.failures.is_empty());
    assert_eq!(report.scanned_repository_count(), 1);
    assert_eq!(
        report.scanned_repository_paths(),
        vec!["acme/b".to_string()]
    );
    assert_eq!(*seen.lock().unwrap(), vec![repo_b.id.clone()]);
}

#[test]
fn mechanical_worker_hint_matching_ticks_only_the_hinted_repository() {
    let forge = MemoryForge::new();
    let repo_a = create_repo(&forge, "a");
    let repo_b = create_repo(&forge, "b");
    let dep_a = create_issue(&forge, &repo_a.id, &["code", "ready"]);
    let dep_b = create_issue(&forge, &repo_b.id, &["code", "ready"]);
    close_issue(&forge, &repo_a.id, dep_a);
    close_issue(&forge, &repo_b.id, dep_b);
    let blocked_a = create_issue(&forge, &repo_a.id, &["code", "blocked"]);
    let blocked_b = create_issue(&forge, &repo_b.id, &["code", "blocked"]);
    add_issue_dependency(&forge, &repo_a.id, blocked_a, dep_a);
    add_issue_dependency(&forge, &repo_b.id, blocked_b, dep_b);
    let workflow = workflow();
    let journal_a = InMemoryJournal::new();
    let journal_b = InMemoryJournal::new();
    let worker = MultiRepoMechanicalWorker::new(
        &workflow,
        &forge,
        RepositorySet::new(vec![repo_a.clone(), repo_b.clone()]),
        vec![
            RepositoryJournal {
                repository: &repo_a.id,
                journal: &journal_a,
            },
            RepositoryJournal {
                repository: &repo_b.id,
                journal: &journal_b,
            },
        ],
        lease_policy(),
    )
    .expect("worker builds");

    let hint = ChangeHint::repo(RepositoryPath::new("acme", "b"), ChangeKind::Issue);
    let report = block_on(worker.tick_matching_hints(ts("2026-05-29T00:00:00Z"), &[hint]));

    assert!(report.failures.is_empty());
    assert_eq!(report.scanned_repository_count(), 1);
    assert_eq!(
        report.scanned_repository_paths(),
        vec!["acme/b".to_string()]
    );
    assert_eq!(
        issue_labels(&forge, &repo_a.id, blocked_a),
        vec!["blocked".to_string(), "code".to_string()]
    );
    assert_eq!(
        issue_labels(&forge, &repo_b.id, blocked_b),
        vec!["code".to_string(), "ready".to_string()]
    );
}

#[test]
fn repository_scan_order_is_deterministic_and_hints_prioritize() {
    let forge = MemoryForge::new();
    let repo_c = create_repo(&forge, "c");
    let repo_a = create_repo(&forge, "a");
    let repo_b = create_repo(&forge, "b");
    for repo in [&repo_a, &repo_b, &repo_c] {
        create_issue(&forge, &repo.id, &["code", "ready"]);
    }
    let workflow = workflow();
    let compiled = workflow.compile();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let worker = MultiRepoRoleWorker::new(
        &workflow,
        &compiled,
        &forge,
        RepositorySet::new(vec![repo_c.clone(), repo_b.clone(), repo_a.clone()]),
        RoleId::new("engineer"),
        Arc::new(RecordingAgent {
            seen: Arc::clone(&seen),
        }),
        ExecutionContext::new(),
    );

    let report = block_on(worker.tick_report(ts("2026-05-29T00:00:00Z")));
    assert!(report.failures.is_empty());
    assert_eq!(
        *seen.lock().unwrap(),
        vec![repo_a.id.clone(), repo_b.id.clone(), repo_c.id.clone()]
    );

    seen.lock().unwrap().clear();
    let hint = ChangeHint::repo(RepositoryPath::new("acme", "c"), ChangeKind::Issue);
    let report = block_on(worker.tick_hinted(ts("2026-05-29T00:00:01Z"), &[hint]));
    assert!(report.failures.is_empty());
    assert_eq!(
        *seen.lock().unwrap(),
        vec![repo_c.id.clone(), repo_a.id.clone(), repo_b.id.clone()]
    );
}

#[test]
fn repository_set_can_be_resolved_from_forge_ids() {
    let forge = MemoryForge::new();
    let repo_b = create_repo(&forge, "b");
    let repo_a = create_repo(&forge, "a");

    let set = block_on(RepositorySet::resolve(
        &forge,
        vec![repo_b.id.clone(), repo_a.id.clone()],
    ))
    .expect("repositories resolve");

    let paths: Vec<String> = set
        .repositories()
        .iter()
        .map(RepositoryTarget::display_path)
        .collect();
    assert_eq!(paths, vec!["acme/a".to_string(), "acme/b".to_string()]);
}
