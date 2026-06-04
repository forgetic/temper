//! Targeted correlation lookup tests for idempotent issue and PR creation.

mod support;

use support::crash::{CrashForge, ForgeOp};
use support::{block_on, create_issue, new_repo, workflow, TestRoot};
use temper_forge::{
    BranchRef, CreateIssue, CreatePullRequest, Forge, IssueQuery, IssueState, ItemListDetails,
    MergeMethod, MergePullRequest, PullRequestQuery, PullRequestState, RepositoryId, UpdateIssue,
    UserId,
};
use temper_workflow::{
    parse_metadata_block, render_metadata_block, ArtifactRef, EnsureOutcome, Executor,
    WorkflowMetadata,
};

fn code_issue_input() -> CreateIssue {
    CreateIssue {
        title: "Add login flow".into(),
        body: "Implements login.".into(),
        labels: vec!["code".into(), "ready".into()],
        assignees: Vec::<UserId>::new(),
    }
}

fn implementation_pr_input(repo: &RepositoryId) -> CreatePullRequest {
    CreatePullRequest {
        title: "Implement login".into(),
        body: "Implements login.".into(),
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
fn normal_ensure_paths_use_targeted_summary_correlation_queries() {
    let root = TestRoot::new();
    let forge = root.forge();
    let repo = new_repo(&forge);
    let crash = CrashForge::new(forge, vec![]);
    let workflow = workflow();
    let executor = Executor::new(&workflow, &crash);

    block_on(executor.ensure_issue(&repo, "code-issue-42", code_issue_input()))
        .expect("issue ensure succeeds");
    block_on(executor.ensure_pull_request(
        &repo,
        "implementation-pr-42",
        implementation_pr_input(&repo),
    ))
    .expect("pull-request ensure succeeds");

    assert_eq!(crash.count(ForgeOp::ListIssuesDefault), 0);
    assert_eq!(crash.count(ForgeOp::ListPullRequestsDefault), 0);

    let issue_queries = crash.issue_queries();
    assert_eq!(
        issue_queries,
        vec![
            IssueQuery {
                state: Some(IssueState::Open),
                labels: vec!["code".into(), "ready".into()],
                body_contains: Some("\"correlation_key\": \"code-issue-42\"".into()),
                author_id: None,
                assignee_id: None,
                sort: None,
                details: ItemListDetails::summary(),
            },
            IssueQuery {
                state: Some(IssueState::Closed),
                labels: vec!["code".into(), "ready".into()],
                body_contains: Some("\"correlation_key\": \"code-issue-42\"".into()),
                author_id: None,
                assignee_id: None,
                sort: None,
                details: ItemListDetails::summary(),
            },
        ]
    );

    let pull_request_queries = crash.pull_request_queries();
    assert_eq!(
        pull_request_queries,
        vec![
            PullRequestQuery {
                state: Some(PullRequestState::Open),
                labels: vec!["implementation".into()],
                body_contains: Some("\"correlation_key\": \"implementation-pr-42\"".into()),
                author_id: None,
                assignee_id: None,
                sort: None,
                details: ItemListDetails::summary(),
            },
            PullRequestQuery {
                state: Some(PullRequestState::Closed),
                labels: vec!["implementation".into()],
                body_contains: Some("\"correlation_key\": \"implementation-pr-42\"".into()),
                author_id: None,
                assignee_id: None,
                sort: None,
                details: ItemListDetails::summary(),
            },
            PullRequestQuery {
                state: Some(PullRequestState::Merged),
                labels: vec!["implementation".into()],
                body_contains: Some("\"correlation_key\": \"implementation-pr-42\"".into()),
                author_id: None,
                assignee_id: None,
                sort: None,
                details: ItemListDetails::summary(),
            },
        ]
    );
}

#[test]
fn ensure_lookup_confirms_metadata_instead_of_trusting_body_filter() {
    let root = TestRoot::new();
    let forge = root.forge();
    let repo = new_repo(&forge);
    block_on(forge.create_issue(
        &repo,
        CreateIssue {
            title: "False positive".into(),
            body: "Prose mentions \"correlation_key\": \"code-issue-42\" only.".into(),
            labels: vec!["code".into(), "ready".into()],
            assignees: Vec::<UserId>::new(),
        },
    ))
    .expect("false-positive issue created");
    let workflow = workflow();
    let executor = Executor::new(&workflow, &forge);

    let outcome = block_on(executor.ensure_issue(&repo, "code-issue-42", code_issue_input()))
        .expect("ensure succeeds");

    assert!(matches!(outcome, EnsureOutcome::Created(_)));
    let issues =
        block_on(forge.list_issues(&repo, IssueQuery::default())).expect("issues list succeeds");
    assert_eq!(issues.len(), 2, "false-positive body text was not accepted");
}

#[test]
fn ensure_finds_closed_issue_and_merged_pr_by_correlation_key() {
    let root = TestRoot::new();
    let forge = root.forge();
    let repo = new_repo(&forge);
    let workflow = workflow();
    let executor = Executor::new(&workflow, &forge);

    let issue = block_on(executor.ensure_issue(&repo, "code-issue-42", code_issue_input()))
        .expect("issue created")
        .into_artifact();
    block_on(forge.update_issue(
        &issue.id,
        UpdateIssue {
            state: Some(IssueState::Closed),
            ..UpdateIssue::default()
        },
    ))
    .expect("issue closed");

    let existing_issue =
        block_on(executor.ensure_issue(&repo, "code-issue-42", code_issue_input()))
            .expect("closed issue is found");
    assert!(matches!(existing_issue, EnsureOutcome::Existing(_)));
    assert_eq!(existing_issue.artifact().state, IssueState::Closed);

    let pull_request = block_on(executor.ensure_pull_request(
        &repo,
        "implementation-pr-42",
        implementation_pr_input(&repo),
    ))
    .expect("pull request created")
    .into_artifact();
    block_on(forge.merge_pull_request(
        &pull_request.id,
        MergePullRequest {
            method: MergeMethod::MergeCommit,
            commit_title: None,
            commit_body: None,
        },
    ))
    .expect("pull request merged");

    let existing_pr = block_on(executor.ensure_pull_request(
        &repo,
        "implementation-pr-42",
        implementation_pr_input(&repo),
    ))
    .expect("merged pull request is found");
    assert!(matches!(existing_pr, EnsureOutcome::Existing(_)));
    assert_eq!(existing_pr.artifact().state, PullRequestState::Merged);
}

#[test]
fn ensure_issue_with_parent_repairs_child_found_through_targeted_query() {
    let root = TestRoot::new();
    let forge = root.forge();
    let repo = new_repo(&forge);
    let parent_number = create_issue(&forge, &repo, &["code"], "Parent work");
    let parent = ArtifactRef::in_repo(repo.clone(), parent_number);
    let key = "child-code-issue-42";
    let existing = block_on(forge.create_issue(
        &repo,
        CreateIssue {
            title: "Existing child".into(),
            body: format!(
                "Child body\n\n{}",
                render_metadata_block(&WorkflowMetadata {
                    correlation_key: Some(key.into()),
                    ..WorkflowMetadata::default()
                })
            ),
            labels: vec!["code".into(), "ready".into()],
            assignees: Vec::<UserId>::new(),
        },
    ))
    .expect("existing child created");
    let crash = CrashForge::new(forge.clone(), vec![]);
    let workflow = workflow();
    let executor = Executor::new(&workflow, &crash);

    let outcome = block_on(executor.ensure_issue_with_parent(
        &repo,
        key,
        Some(parent.clone()),
        code_issue_input(),
    ))
    .expect("ensure repairs parent metadata");

    assert!(matches!(outcome, EnsureOutcome::Existing(_)));
    assert_eq!(outcome.artifact().number, existing.number);
    assert_eq!(crash.count(ForgeOp::CreateIssue), 0);
    assert_eq!(crash.count(ForgeOp::UpdateIssue), 1);
    assert_eq!(crash.count(ForgeOp::ListIssuesDefault), 0);
    assert!(crash
        .issue_queries()
        .iter()
        .all(|query| query.details == ItemListDetails::summary()
            && query.body_contains.as_deref()
                == Some("\"correlation_key\": \"child-code-issue-42\"")
            && query.state.is_some()
            && query.labels == vec!["code".to_string(), "ready".to_string()]));

    let repaired = block_on(forge.get_issue_by_number(&repo, existing.number))
        .expect("lookup succeeds")
        .expect("child exists");
    let metadata = parse_metadata_block(&repaired.body)
        .expect("metadata parses")
        .expect("metadata exists");
    assert!(metadata.parents.contains(&parent));
}
