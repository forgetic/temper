//! Offline contract tests for portable body-substring list filters.

mod support;

use support::{MockHttpClient, OWNER, REPO, block_on, forge, repo_id};
use temper_forge_forgejo::HttpMethod;
use temper_forge_model::{
    IssueQuery, IssueState, ItemListDetails, ItemNumber, PullRequestQuery, PullRequestState,
};

fn issue_json(number: u64, body: &str, labels: &str) -> String {
    format!(
        r#"{{
            "number": {number},
            "title": "Issue {number}",
            "body": "{body}",
            "state": "open",
            "user": {{"login": "author"}},
            "labels": {labels},
            "created_at": "2024-03-01T00:00:00Z",
            "updated_at": "2024-03-02T00:00:00Z"
        }}"#
    )
}

fn pr_issue_json(number: u64, body: &str) -> String {
    format!(
        r#"{{
            "number": {number},
            "title": "PR {number}",
            "body": "{body}",
            "state": "open",
            "user": {{"login": "author"}},
            "labels": [{{"id":1,"name":"ready"}}],
            "created_at": "2024-03-01T00:00:00Z",
            "updated_at": "2024-03-02T00:00:00Z",
            "pull_request": {{"url": "http://x/pulls/{number}"}}
        }}"#
    )
}

#[test]
fn issue_body_contains_filters_after_state_and_labels() {
    let client = MockHttpClient::new();
    let marker = "temper:correlation:issue-1";
    client.push_response(
        200,
        format!(
            "[{},{}]",
            issue_json(
                1,
                &format!("prefix {marker} suffix"),
                r#"[{"id":1,"name":"ready"}]"#
            ),
            issue_json(2, "different", r#"[{"id":1,"name":"ready"}]"#),
        ),
    );
    let forge = forge(client.clone());

    let issues = block_on(forge.list_issues(
        &repo_id(),
        IssueQuery {
            state: Some(IssueState::Open),
            labels: vec!["ready".into()],
            body_contains: Some(marker.into()),
            details: ItemListDetails::summary(),
            ..IssueQuery::default()
        },
    ))
    .unwrap();

    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].number, ItemNumber::new(1));
    let request = client.last_request().unwrap();
    assert_eq!(request.method, HttpMethod::Get);
    assert_eq!(request.path, format!("/api/v1/repos/{OWNER}/{REPO}/issues"));
    assert!(request.query.contains(&("state".into(), "open".into())));
    assert!(request.query.contains(&("type".into(), "issues".into())));
    assert!(request.query.contains(&("labels".into(), "ready".into())));
    assert!(
        !request
            .query
            .iter()
            .any(|(key, _)| key == "q" || key == "body")
    );
}

#[test]
fn labelled_pull_request_body_filter_keeps_label_index_query_bounded() {
    let client = MockHttpClient::new();
    let marker = "temper:correlation:pr-1";
    client.push_response(
        200,
        format!(
            "[{},{}]",
            pr_issue_json(1, &format!("prefix {marker} suffix")),
            pr_issue_json(2, "different"),
        ),
    );
    let forge = forge(client.clone());

    let pulls = block_on(forge.list_pull_requests(
        &repo_id(),
        PullRequestQuery {
            state: Some(PullRequestState::Open),
            labels: vec!["ready".into()],
            body_contains: Some(marker.into()),
            details: ItemListDetails::summary(),
            ..PullRequestQuery::default()
        },
    ))
    .unwrap();

    assert_eq!(pulls.len(), 1);
    assert_eq!(pulls[0].number, ItemNumber::new(1));
    let requests = client.recorded();
    let discovery = &requests[0];
    assert_eq!(discovery.method, HttpMethod::Get);
    assert_eq!(
        discovery.path,
        format!("/api/v1/repos/{OWNER}/{REPO}/issues")
    );
    assert!(discovery.query.contains(&("state".into(), "open".into())));
    assert!(discovery.query.contains(&("type".into(), "pulls".into())));
    assert!(discovery.query.contains(&("labels".into(), "ready".into())));
    assert!(!requests.iter().any(|request| {
        request.path.ends_with("/pulls") && request.query.contains(&("state".into(), "all".into()))
    }));
    assert!(
        !requests
            .iter()
            .any(|request| request.path.contains("/pulls/"))
    );
}
