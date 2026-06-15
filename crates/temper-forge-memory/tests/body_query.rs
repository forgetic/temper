use std::future::Future;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use temper_forge_model::{
    BranchRef, CreateIssue, CreatePullRequest, CreateRepository, Forge, IssueQuery, IssueState,
    ItemNumber, PullRequestQuery, PullRequestState, PullRequestUpdateState, RepositoryId,
    UpdateIssue, UpdatePullRequest,
};
use temper_forge_memory::MemoryForge;

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
        Poll::Pending => panic!("in-memory forge futures should not park"),
    }
}

fn repo(forge: &MemoryForge) -> RepositoryId {
    block_on(forge.create_repository(CreateRepository {
        owner: "acme".into(),
        name: "service".into(),
        default_branch: "main".into(),
        description: None,
    }))
    .unwrap()
    .id
}

fn issue(body: &str, labels: &[&str]) -> CreateIssue {
    CreateIssue {
        title: "issue".into(),
        body: body.into(),
        labels: labels.iter().map(|label| (*label).into()).collect(),
        assignees: Vec::new(),
    }
}

fn pull_request(repo: &RepositoryId, body: &str, labels: &[&str]) -> CreatePullRequest {
    CreatePullRequest {
        title: "pr".into(),
        body: body.into(),
        source: BranchRef {
            repository_id: repo.clone(),
            branch: "feature".into(),
        },
        target: BranchRef {
            repository_id: repo.clone(),
            branch: "main".into(),
        },
        labels: labels.iter().map(|label| (*label).into()).collect(),
        assignees: Vec::new(),
    }
}

#[test]
fn body_contains_filters_issues_with_state_and_labels() {
    let forge = MemoryForge::new();
    let repo = repo(&forge);
    let marker = "temper:correlation:issue-1";
    let matching = block_on(forge.create_issue(
        &repo,
        issue(&format!("prefix {marker} suffix"), &["workflow"]),
    ))
    .unwrap();
    block_on(forge.create_issue(&repo, issue(marker, &["other"]))).unwrap();
    block_on(forge.create_issue(&repo, issue("different", &["workflow"]))).unwrap();
    let closed = block_on(forge.create_issue(&repo, issue(marker, &["workflow"]))).unwrap();
    block_on(forge.update_issue(
        &closed.id,
        UpdateIssue {
            state: Some(IssueState::Closed),
            ..UpdateIssue::default()
        },
    ))
    .unwrap();

    let issues = block_on(forge.list_issues(
        &repo,
        IssueQuery {
            state: Some(IssueState::Open),
            labels: vec!["workflow".into()],
            body_contains: Some(marker.into()),
            ..IssueQuery::default()
        },
    ))
    .unwrap();
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].number, matching.number);

    let empty_is_no_filter = block_on(forge.list_issues(
        &repo,
        IssueQuery {
            state: Some(IssueState::Open),
            labels: vec!["workflow".into()],
            body_contains: Some(String::new()),
            ..IssueQuery::default()
        },
    ))
    .unwrap();
    let numbers: Vec<ItemNumber> = empty_is_no_filter
        .iter()
        .map(|issue| issue.number)
        .collect();
    assert_eq!(numbers, vec![ItemNumber::new(1), ItemNumber::new(3)]);
}

#[test]
fn body_contains_filters_pull_requests_with_state_and_labels() {
    let forge = MemoryForge::new();
    let repo = repo(&forge);
    let marker = "temper:correlation:pr-1";
    let matching = block_on(forge.create_pull_request(
        &repo,
        pull_request(&repo, &format!("prefix {marker} suffix"), &["workflow"]),
    ))
    .unwrap();
    block_on(forge.create_pull_request(&repo, pull_request(&repo, marker, &["other"]))).unwrap();
    block_on(forge.create_pull_request(&repo, pull_request(&repo, "different", &["workflow"])))
        .unwrap();
    let closed =
        block_on(forge.create_pull_request(&repo, pull_request(&repo, marker, &["workflow"])))
            .unwrap();
    block_on(forge.update_pull_request(
        &closed.id,
        UpdatePullRequest {
            state: Some(PullRequestUpdateState::Closed),
            ..UpdatePullRequest::default()
        },
    ))
    .unwrap();

    let pulls = block_on(forge.list_pull_requests(
        &repo,
        PullRequestQuery {
            state: Some(PullRequestState::Open),
            labels: vec!["workflow".into()],
            body_contains: Some(marker.into()),
            ..PullRequestQuery::default()
        },
    ))
    .unwrap();
    assert_eq!(pulls.len(), 1);
    assert_eq!(pulls[0].number, matching.number);
}
