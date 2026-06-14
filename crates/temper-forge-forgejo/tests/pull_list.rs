//! Offline contract tests focused on Forgejo pull-request list request shapes.

mod support;

use support::{MockHttpClient, OWNER, REPO, block_on, forge, repo_id};
use temper_forge::{ItemListDetails, ItemNumber, PullRequestQuery, PullRequestState};
use temper_forge_forgejo::HttpMethod;

fn pr_issue_json(number: u64, state: &str, labels: &str) -> String {
    pr_issue_json_with_merge(number, state, labels, None)
}

fn pr_issue_json_with_merge(
    number: u64,
    state: &str,
    labels: &str,
    merged: Option<bool>,
) -> String {
    let labels: serde_json::Value = serde_json::from_str(labels).expect("labels are json");
    let marker = match merged {
        Some(true) => serde_json::json!({
            "url": format!("http://x/pulls/{number}"),
            "merged": true,
            "merged_at": "2024-03-03T00:00:00Z"
        }),
        Some(false) => serde_json::json!({
            "url": format!("http://x/pulls/{number}"),
            "merged": false,
            "merged_at": null
        }),
        None => serde_json::json!({"url": format!("http://x/pulls/{number}")}),
    };
    serde_json::json!({
        "number": number,
        "title": format!("PR {number}"),
        "body": format!("body {number}"),
        "state": state,
        "user": {"login": "author"},
        "labels": labels,
        "created_at": "2024-03-01T00:00:00Z",
        "updated_at": "2024-03-02T00:00:00Z",
        "pull_request": marker
    })
    .to_string()
}

fn pr_json(number: u64, state: &str, labels: &str) -> String {
    serde_json::json!({
        "number": number,
        "title": format!("PR {number}"),
        "body": format!("body {number}"),
        "state": state,
        "merged": false,
        "user": {"login": "author"},
        "head": {"ref": format!("feature-{number}"), "sha": format!("head{number}")},
        "base": {"ref": "main", "sha": format!("base{number}")},
        "labels": serde_json::from_str::<serde_json::Value>(labels).expect("labels are json"),
        "created_at": "2024-03-01T00:00:00Z",
        "updated_at": "2024-03-02T00:00:00Z"
    })
    .to_string()
}

#[test]
fn labelled_open_pull_request_summary_uses_issue_index_without_detail() {
    let client = MockHttpClient::new();
    client.push_response(
        200,
        format!(
            "[{}]",
            pr_issue_json(1, "open", r#"[{"id":1,"name":"ready"}]"#)
        ),
    );
    let forge = forge(client.clone());

    let pulls = block_on(forge.list_pull_requests(
        &repo_id(),
        PullRequestQuery {
            state: Some(PullRequestState::Open),
            labels: vec!["ready".to_string()],
            details: ItemListDetails::summary(),
            ..PullRequestQuery::default()
        },
    ))
    .unwrap();

    assert_eq!(pulls.len(), 1);
    assert_eq!(pulls[0].number, ItemNumber::new(1));
    assert_eq!(pulls[0].state, PullRequestState::Open);
    assert!(pulls[0].source.branch.is_empty());
    assert!(pulls[0].requested_reviewers.is_empty());
    assert!(pulls[0].dependencies.is_empty());

    let requests = client.recorded();
    assert_eq!(requests.len(), 1);
    let discovery = &requests[0];
    assert_eq!(discovery.method, HttpMethod::Get);
    assert_eq!(
        discovery.path,
        format!("/api/v1/repos/{OWNER}/{REPO}/issues")
    );
    assert!(discovery.query.contains(&("state".into(), "open".into())));
    assert!(discovery.query.contains(&("type".into(), "pulls".into())));
    assert!(discovery.query.contains(&("labels".into(), "ready".into())));
    assert!(
        !requests
            .iter()
            .any(|request| request.path.contains("/pulls/"))
    );
}

#[test]
fn labelled_closed_and_merged_summary_filters_with_marker_without_detail() {
    let client = MockHttpClient::new();
    let rows = format!(
        "[{},{}]",
        pr_issue_json_with_merge(1, "closed", r#"[{"id":1,"name":"ready"}]"#, Some(false)),
        pr_issue_json_with_merge(2, "closed", r#"[{"id":1,"name":"ready"}]"#, Some(true)),
    );
    client.push_response(200, rows.clone());
    client.push_response(200, rows);
    let forge = forge(client.clone());

    let closed = block_on(forge.list_pull_requests(
        &repo_id(),
        PullRequestQuery {
            state: Some(PullRequestState::Closed),
            labels: vec!["ready".to_string()],
            details: ItemListDetails::summary(),
            ..PullRequestQuery::default()
        },
    ))
    .unwrap();
    assert_eq!(closed.len(), 1);
    assert_eq!(closed[0].number, ItemNumber::new(1));
    assert_eq!(closed[0].state, PullRequestState::Closed);

    let merged = block_on(forge.list_pull_requests(
        &repo_id(),
        PullRequestQuery {
            state: Some(PullRequestState::Merged),
            labels: vec!["ready".to_string()],
            details: ItemListDetails::summary(),
            ..PullRequestQuery::default()
        },
    ))
    .unwrap();
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].number, ItemNumber::new(2));
    assert_eq!(merged[0].state, PullRequestState::Merged);

    let requests = client.recorded();
    assert_eq!(requests.len(), 2);
    assert!(requests.iter().all(|request| {
        request.path == format!("/api/v1/repos/{OWNER}/{REPO}/issues")
            && !request.path.contains("/pulls/")
    }));
}

#[test]
fn labelled_summary_falls_back_to_detail_when_merge_marker_is_missing() {
    let client = MockHttpClient::new();
    client.push_response(
        200,
        format!(
            "[{}]",
            pr_issue_json(3, "closed", r#"[{"id":1,"name":"ready"}]"#)
        ),
    );
    client.push_response(200, pr_json(3, "closed", r#"[{"id":1,"name":"ready"}]"#));
    let forge = forge(client.clone());

    let pulls = block_on(forge.list_pull_requests(
        &repo_id(),
        PullRequestQuery {
            state: Some(PullRequestState::Closed),
            labels: vec!["ready".to_string()],
            details: ItemListDetails::summary(),
            ..PullRequestQuery::default()
        },
    ))
    .unwrap();

    assert_eq!(pulls.len(), 1);
    let requests = client.recorded();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[1].path,
        format!("/api/v1/repos/{OWNER}/{REPO}/pulls/3")
    );
}

#[test]
fn labelled_full_detail_query_fetches_exact_pull_details() {
    let client = MockHttpClient::new();
    client.push_response(
        200,
        format!(
            "[{}]",
            pr_issue_json(4, "open", r#"[{"id":1,"name":"ready"}]"#)
        ),
    );
    client.push_response(200, pr_json(4, "open", r#"[{"id":1,"name":"ready"}]"#));
    client.push_response(200, r#"[{"number": 9}]"#);
    let forge = forge(client.clone());

    let pulls = block_on(forge.list_pull_requests(
        &repo_id(),
        PullRequestQuery {
            state: Some(PullRequestState::Open),
            labels: vec!["ready".to_string()],
            ..PullRequestQuery::default()
        },
    ))
    .unwrap();

    assert_eq!(pulls[0].source.branch, "feature-4");
    assert_eq!(pulls[0].dependencies, vec![ItemNumber::new(9)]);
    let requests = client.recorded();
    assert_eq!(
        requests[1].path,
        format!("/api/v1/repos/{OWNER}/{REPO}/pulls/4")
    );
    assert_eq!(
        requests[2].path,
        format!("/api/v1/repos/{OWNER}/{REPO}/issues/4/dependencies")
    );
}
