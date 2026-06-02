//! Pull-request idempotent create tests (Phase 10).

mod support;

use support::{block_on, create_issue, new_repo, workflow, TestRoot};
use temper_forge::{BranchRef, CreatePullRequest, Forge, PullRequestQuery, RepositoryId, UserId};
use temper_workflow::{
    parse_metadata_block, ArtifactSource, ExecutionContext, Executor, RawWorkflowSpec, RoleId,
    TransitionId, ValidatedWorkflow, WorkflowEffect,
};

const PR_CREATE_WORKFLOW: &str = r#"{
    "name": "pr-create",
    "roles": [{"id": "engineer"}],
    "labels": [{"id": "code"}],
    "artifact_kinds": [
        {"id": "code", "target": "issue", "identifying_labels": ["code"]}
    ],
    "transitions": [{"id": "open_pr", "artifact": "code", "roles": ["engineer"], "effects": [
        {"kind": "create_pull_request", "correlation_key": "pr-code-42"}
    ]}]
}"#;

const DYNAMIC_PR_CREATE_WORKFLOW: &str = r#"{
    "name": "dynamic-pr-create",
    "roles": [{"id": "engineer"}],
    "labels": [{"id": "code"}],
    "artifact_kinds": [
        {"id": "code", "target": "issue", "identifying_labels": ["code"]}
    ],
    "transitions": [{"id": "open_pr", "artifact": "code", "roles": ["engineer"], "effects": [
        {"kind": "create_pull_request"}
    ]}]
}"#;
fn pr_create_workflow() -> ValidatedWorkflow {
    let spec: RawWorkflowSpec = serde_json::from_str(PR_CREATE_WORKFLOW).expect("json parses");
    spec.validate().expect("workflow validates")
}

fn dynamic_pr_create_workflow() -> ValidatedWorkflow {
    let spec: RawWorkflowSpec =
        serde_json::from_str(DYNAMIC_PR_CREATE_WORKFLOW).expect("json parses");
    spec.validate().expect("workflow validates")
}

fn pr_input(repo: &RepositoryId) -> CreatePullRequest {
    CreatePullRequest {
        title: "Implement login".into(),
        body: "Implements the login flow.".into(),
        source: BranchRef {
            repository_id: repo.clone(),
            branch: "feature/login".into(),
        },
        target: BranchRef {
            repository_id: repo.clone(),
            branch: "main".into(),
        },
        labels: vec!["implementation".into()],
        assignees: Vec::<UserId>::new(),
    }
}

#[test]
fn ensure_pull_request_is_idempotent_across_retries() {
    let root = TestRoot::new();
    let forge = root.forge();
    let workflow = workflow();
    let repo = new_repo(&forge);
    let executor = Executor::new(&workflow, &forge);

    let first = block_on(executor.ensure_pull_request(&repo, "pr-code-42", pr_input(&repo)))
        .expect("first ensure creates the pull request");
    assert!(first.was_created());
    let created = first.artifact().clone();
    let metadata = parse_metadata_block(&created.body)
        .expect("body metadata parses")
        .expect("body has a metadata block");
    assert_eq!(metadata.correlation_key.as_deref(), Some("pr-code-42"));

    let second = block_on(executor.ensure_pull_request(&repo, "pr-code-42", pr_input(&repo)))
        .expect("second ensure finds the existing pull request");
    assert!(!second.was_created());
    assert_eq!(second.artifact().number, created.number);
    assert_eq!(second.artifact().id, created.id);

    let pull_requests = block_on(forge.list_pull_requests(&repo, PullRequestQuery::default()))
        .expect("pull requests list");
    assert_eq!(pull_requests.len(), 1);
}

#[test]
fn create_pull_request_effect_uses_idempotent_ensure_path() {
    let root = TestRoot::new();
    let forge = root.forge();
    let workflow = pr_create_workflow();
    let repo = new_repo(&forge);
    let issue = create_issue(&forge, &repo, &["code"], "Implement login.");
    let context = ExecutionContext::new()
        .with_pull_request_create(TransitionId::new("open_pr"), pr_input(&repo));
    let executor = workflow.executor_with_context(&forge, context);

    let report = block_on(executor.execute(
        &repo,
        ArtifactSource::Issue { number: issue },
        &TransitionId::new("open_pr"),
        &RoleId::new("engineer"),
    ))
    .expect("create_pull_request executes");
    assert_eq!(
        report.applied,
        vec![WorkflowEffect::CreatePullRequest {
            correlation_key: Some("pr-code-42".into()),
        }]
    );

    block_on(executor.execute(
        &repo,
        ArtifactSource::Issue { number: issue },
        &TransitionId::new("open_pr"),
        &RoleId::new("engineer"),
    ))
    .expect("retry finds the existing PR by correlation key");

    let pull_requests = block_on(forge.list_pull_requests(&repo, PullRequestQuery::default()))
        .expect("pull requests list");
    assert_eq!(pull_requests.len(), 1);
    let metadata = parse_metadata_block(&pull_requests[0].body)
        .expect("metadata parses")
        .expect("metadata exists");
    assert_eq!(metadata.correlation_key.as_deref(), Some("pr-code-42"));
}

#[test]
fn create_pull_request_effect_accepts_runtime_correlation_key() {
    let root = TestRoot::new();
    let forge = root.forge();
    let workflow = dynamic_pr_create_workflow();
    let repo = new_repo(&forge);
    let issue = create_issue(&forge, &repo, &["code"], "Implement login.");
    let context = ExecutionContext::new()
        .with_pull_request_create(TransitionId::new("open_pr"), pr_input(&repo))
        .with_pull_request_correlation_key_at(TransitionId::new("open_pr"), 0, "pr-code-99");
    let executor = workflow.executor_with_context(&forge, context);

    let report = block_on(executor.execute(
        &repo,
        ArtifactSource::Issue { number: issue },
        &TransitionId::new("open_pr"),
        &RoleId::new("engineer"),
    ))
    .expect("create_pull_request executes with runtime key");
    assert_eq!(
        report.applied,
        vec![WorkflowEffect::CreatePullRequest {
            correlation_key: None,
        }]
    );

    let pull_requests = block_on(forge.list_pull_requests(&repo, PullRequestQuery::default()))
        .expect("pull requests list");
    assert_eq!(pull_requests.len(), 1);
    let metadata = parse_metadata_block(&pull_requests[0].body)
        .expect("metadata parses")
        .expect("metadata exists");
    assert_eq!(metadata.correlation_key.as_deref(), Some("pr-code-99"));
}
