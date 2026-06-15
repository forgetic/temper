mod support;

use support::{TestRoot, block_on, issue_with, pull_request_with, repository};
use temper_forge_model::{
    Forge, IssueQuery, IssueState, ItemNumber, PullRequestQuery, PullRequestState,
    PullRequestUpdateState, UpdateIssue, UpdatePullRequest,
};

#[test]
fn body_contains_filters_issues_with_state_and_labels() {
    let root = TestRoot::new("body-query-issues");
    let forge = root.forge();
    let repository = block_on(forge.create_repository(repository("alice", "project"))).unwrap();
    let marker = "temper:correlation:issue-1";
    let matching = block_on(forge.create_issue(
        &repository.id,
        issue_with(
            "matching",
            &format!("prefix {marker} suffix"),
            &["workflow"],
            &[],
        ),
    ))
    .unwrap();
    block_on(forge.create_issue(
        &repository.id,
        issue_with("other label", marker, &["other"], &[]),
    ))
    .unwrap();
    block_on(forge.create_issue(
        &repository.id,
        issue_with("no marker", "different", &["workflow"], &[]),
    ))
    .unwrap();
    let closed = block_on(forge.create_issue(
        &repository.id,
        issue_with("closed", marker, &["workflow"], &[]),
    ))
    .unwrap();
    block_on(forge.update_issue(
        &closed.id,
        UpdateIssue {
            state: Some(IssueState::Closed),
            ..UpdateIssue::default()
        },
    ))
    .unwrap();

    let issues = block_on(forge.list_issues(
        &repository.id,
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
        &repository.id,
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
    let root = TestRoot::new("body-query-pulls");
    let forge = root.forge();
    let repository = block_on(forge.create_repository(repository("alice", "project"))).unwrap();
    let marker = "temper:correlation:pr-1";
    let matching = block_on(forge.create_pull_request(
        &repository.id,
        pull_request_with(
            &repository.id,
            "matching",
            &format!("prefix {marker} suffix"),
            &["workflow"],
            &[],
        ),
    ))
    .unwrap();
    block_on(forge.create_pull_request(
        &repository.id,
        pull_request_with(&repository.id, "other label", marker, &["other"], &[]),
    ))
    .unwrap();
    block_on(forge.create_pull_request(
        &repository.id,
        pull_request_with(&repository.id, "no marker", "different", &["workflow"], &[]),
    ))
    .unwrap();
    let closed = block_on(forge.create_pull_request(
        &repository.id,
        pull_request_with(&repository.id, "closed", marker, &["workflow"], &[]),
    ))
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
        &repository.id,
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
