//! Offline contract tests for issue operations.

mod support;

use support::{MockHttpClient, block_on, body_json, forge, forge_with, issue_id, repo_id};
use temper_forge_github::{CasMode, HttpMethod};
use temper_forge_model::{
    CreateComment, CreateIssue, ForgeError, IssueQuery, IssueState, ItemListDetails, ItemNumber,
    UpdateIssue, UserId,
};

fn issue_json(number: u64, title: &str, state: &str) -> String {
    format!(
        r#"{{
            "number": {number},
            "title": "{title}",
            "body": "the body",
            "state": "{state}",
            "user": {{"login": "author"}},
            "labels": [{{"id": 1, "name": "task"}}],
            "assignees": [{{"login": "bob"}}],
            "created_at": "2024-03-01T00:00:00Z",
            "updated_at": "2024-03-02T00:00:00Z",
            "pull_request": null
        }}"#
    )
}

fn pull_as_issue_json(number: u64) -> String {
    format!(
        r#"{{
            "number": {number},
            "title": "a pull request",
            "state": "open",
            "user": {{"login": "author"}},
            "created_at": "2024-03-01T00:00:00Z",
            "updated_at": "2024-03-02T00:00:00Z",
            "pull_request": {{"url": "https://api.github.com/repos/acme/widgets/pulls/8"}}
        }}"#
    )
}

#[test]
fn issue_limit_bounds_page_size_and_stops_when_satisfied() {
    let client = MockHttpClient::new();
    client.push_response(200, format!("[{}]", issue_json(7, "limited", "open")));
    let forge = forge(client.clone());

    let issues = block_on(forge.list_issues(
        &repo_id(),
        IssueQuery {
            limit: Some(1),
            details: ItemListDetails::summary(),
            ..IssueQuery::default()
        },
    ))
    .unwrap();

    assert_eq!(issues.len(), 1);
    assert_eq!(client.call_count(), 1);
    assert!(
        client.recorded()[0]
            .query
            .contains(&("per_page".to_string(), "1".to_string()))
    );
}

#[test]
fn list_issues_filters_pull_request_rows_and_sends_provider_filters() {
    let client = MockHttpClient::new();
    client.push_response(
        200,
        format!(
            "[{}, {}]",
            issue_json(7, "real issue", "open"),
            pull_as_issue_json(8)
        ),
    );
    let forge = forge(client.clone());

    let issues = block_on(forge.list_issues(
        &repo_id(),
        IssueQuery {
            state: Some(IssueState::Open),
            labels: vec!["task".to_string()],
            ..IssueQuery::default()
        },
    ))
    .unwrap();
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].number, ItemNumber::new(7));
    assert_eq!(issues[0].id, issue_id(7));
    assert_eq!(issues[0].labels, vec!["task".to_string()]);
    assert!(issues[0].dependencies.is_empty());

    let request = client.last_request().unwrap();
    assert_eq!(request.path, "/repos/acme/widgets/issues");
    assert!(
        request
            .query
            .iter()
            .any(|(key, value)| key == "state" && value == "open")
    );
    assert!(
        request
            .query
            .iter()
            .any(|(key, value)| key == "labels" && value == "task")
    );
}

#[test]
fn list_issues_applies_client_side_filters() {
    let client = MockHttpClient::new();
    client.push_response(
        200,
        format!(
            "[{}, {}]",
            issue_json(7, "by author", "open"),
            r#"{
                "number": 9,
                "title": "by someone else",
                "body": "other",
                "state": "open",
                "user": {"login": "stranger"},
                "created_at": "2024-03-01T00:00:00Z",
                "updated_at": "2024-03-02T00:00:00Z"
            }"#
        ),
    );
    let forge = forge(client);

    let issues = block_on(forge.list_issues(
        &repo_id(),
        IssueQuery {
            author_id: Some(UserId::new("author")),
            body_contains: Some("the body".to_string()),
            ..IssueQuery::default()
        },
    ))
    .unwrap();
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].number, ItemNumber::new(7));
}

#[test]
fn get_issue_by_number_maps_absence_and_pull_rows_to_none() {
    let client = MockHttpClient::new();
    client.push_response(404, r#"{"message": "Not Found"}"#);
    client.push_response(200, pull_as_issue_json(8));
    client.push_response(200, issue_json(7, "found", "open"));
    let forge = forge(client.clone());

    let missing = block_on(forge.get_issue_by_number(&repo_id(), ItemNumber::new(99))).unwrap();
    assert!(missing.is_none());

    let pull_row = block_on(forge.get_issue_by_number(&repo_id(), ItemNumber::new(8))).unwrap();
    assert!(pull_row.is_none());

    let found = block_on(forge.get_issue_by_number(&repo_id(), ItemNumber::new(7))).unwrap();
    assert_eq!(found.unwrap().title, "found");
    assert_eq!(client.recorded()[2].path, "/repos/acme/widgets/issues/7");
}

#[test]
fn get_issue_parses_backend_id() {
    let client = MockHttpClient::new();
    client.push_response(200, issue_json(7, "found", "open"));
    let forge = forge(client.clone());

    let issue = block_on(forge.get_issue(&issue_id(7))).unwrap().unwrap();
    assert_eq!(issue.number, ItemNumber::new(7));

    // A foreign id shape is rejected without any HTTP call.
    let error = block_on(forge.get_issue(&temper_forge_model::IssueId::new("forgejo:a/b:issue:1")))
        .unwrap_err();
    assert!(matches!(error, ForgeError::InvalidRequest(_)));
    assert_eq!(client.call_count(), 1);
}

#[test]
fn create_issue_sends_labels_and_assignees_in_one_call() {
    let client = MockHttpClient::new();
    client.push_response(201, issue_json(7, "new issue", "open")); // create
    client.push_response(200, issue_json(7, "new issue", "open")); // re-read
    let forge = forge(client.clone());

    let issue = block_on(forge.create_issue(
        &repo_id(),
        CreateIssue {
            title: "new issue".to_string(),
            body: "the body".to_string(),
            labels: vec!["task".to_string()],
            assignees: vec![UserId::new("bob")],
        },
    ))
    .unwrap();
    assert_eq!(issue.id, issue_id(7));

    let recorded = client.recorded();
    assert_eq!(recorded.len(), 2);
    assert_eq!(recorded[0].method, HttpMethod::Post);
    assert_eq!(recorded[0].path, "/repos/acme/widgets/issues");
    let payload = body_json(&recorded[0]);
    assert_eq!(payload["title"], "new issue");
    assert_eq!(payload["labels"], serde_json::json!(["task"]));
    assert_eq!(payload["assignees"], serde_json::json!(["bob"]));
}

#[test]
fn update_issue_sequences_edit_labels_and_assignees() {
    let client = MockHttpClient::new();
    client.push_response(200, issue_json(7, "old", "open")); // current read
    client.push_response(200, issue_json(7, "new", "closed")); // PATCH echo
    client.push_response(200, r#"[{"id": 1, "name": "done"}]"#); // PUT set labels
    client.push_response(200, issue_json(7, "new", "closed")); // PATCH assignees
    client.push_response(200, issue_json(7, "new", "closed")); // final re-read
    let forge = forge(client.clone());

    let issue = block_on(forge.update_issue(
        &issue_id(7),
        UpdateIssue {
            title: Some("new".to_string()),
            state: Some(IssueState::Closed),
            set_labels: Some(vec!["done".to_string()]),
            add_assignees: vec![UserId::new("carol")],
            ..UpdateIssue::default()
        },
    ))
    .unwrap();
    assert_eq!(issue.state, IssueState::Closed);

    let recorded = client.recorded();
    assert_eq!(recorded.len(), 5);

    assert_eq!(recorded[0].method, HttpMethod::Get);
    assert_eq!(recorded[0].path, "/repos/acme/widgets/issues/7");

    assert_eq!(recorded[1].method, HttpMethod::Patch);
    let edit = body_json(&recorded[1]);
    assert_eq!(edit["title"], "new");
    assert_eq!(edit["state"], "closed");

    assert_eq!(recorded[2].method, HttpMethod::Put);
    assert_eq!(recorded[2].path, "/repos/acme/widgets/issues/7/labels");
    assert_eq!(
        body_json(&recorded[2])["labels"],
        serde_json::json!(["done"])
    );

    assert_eq!(recorded[3].method, HttpMethod::Patch);
    // Assignee set is current (bob) plus carol, sorted.
    assert_eq!(
        body_json(&recorded[3])["assignees"],
        serde_json::json!(["bob", "carol"])
    );

    assert_eq!(recorded[4].method, HttpMethod::Get);
}

#[test]
fn update_issue_removes_labels_individually() {
    let client = MockHttpClient::new();
    client.push_response(200, issue_json(7, "t", "open")); // current read
    client.push_response(200, "{}"); // DELETE label (attached)
    client.push_response(200, issue_json(7, "t", "open")); // final re-read
    let forge = forge(client.clone());

    block_on(forge.update_issue(
        &issue_id(7),
        UpdateIssue {
            remove_labels: vec!["task".to_string()],
            ..UpdateIssue::default()
        },
    ))
    .unwrap();

    let recorded = client.recorded();
    assert_eq!(recorded.len(), 3);
    assert_eq!(recorded[1].method, HttpMethod::Delete);
    assert_eq!(recorded[1].path, "/repos/acme/widgets/issues/7/labels/task");
}

#[test]
fn update_issue_refuses_pull_request_rows() {
    let client = MockHttpClient::new();
    client.push_response(200, pull_as_issue_json(8));
    let forge = forge(client);

    let error = block_on(forge.update_issue(
        &issue_id(8),
        UpdateIssue {
            title: Some("nope".to_string()),
            ..UpdateIssue::default()
        },
    ))
    .unwrap_err();
    assert!(matches!(error, ForgeError::NotFound(_)));
}

#[test]
fn conditional_update_detects_stale_version() {
    let client = MockHttpClient::new();
    // First read captures validator etag-a at version 1.
    client.push_response_with_etag(issue_json(7, "t", "open"), "etag-a");
    // The conditional update re-reads and sees a changed validator.
    client.push_response_with_etag(issue_json(7, "t", "open"), "etag-b");
    let forge = forge_with(client.clone(), CasMode::BestEffort);

    let issue = block_on(forge.get_issue(&issue_id(7))).unwrap().unwrap();

    let error = block_on(forge.update_issue(
        &issue_id(7),
        UpdateIssue {
            title: Some("new".to_string()),
            expected_version: Some(issue.version),
            ..UpdateIssue::default()
        },
    ))
    .unwrap_err();
    assert!(matches!(error, ForgeError::Conflict(_)));
    // Nothing was mutated: only the two reads happened.
    assert_eq!(client.call_count(), 2);
}

#[test]
fn conditional_update_proceeds_when_validator_is_unchanged() {
    let client = MockHttpClient::new();
    client.push_response_with_etag(issue_json(7, "t", "open"), "etag-a");
    client.push_response_with_etag(issue_json(7, "t", "open"), "etag-a");
    client.push_response(200, issue_json(7, "new", "open")); // PATCH echo
    client.push_response(200, issue_json(7, "new", "open")); // final re-read
    let forge = forge_with(client.clone(), CasMode::BestEffort);

    let issue = block_on(forge.get_issue(&issue_id(7))).unwrap().unwrap();
    let updated = block_on(forge.update_issue(
        &issue_id(7),
        UpdateIssue {
            title: Some("new".to_string()),
            expected_version: Some(issue.version),
            ..UpdateIssue::default()
        },
    ))
    .unwrap();
    assert_eq!(updated.title, "new");
    assert_eq!(client.call_count(), 4);
}

#[test]
fn issue_comments_round_trip() {
    let client = MockHttpClient::new();
    client.push_response(
        200,
        r#"[
            {"id": 2, "user": {"login": "bob"}, "body": "second",
             "created_at": "2024-03-02T00:00:00Z", "updated_at": "2024-03-02T00:00:00Z"},
            {"id": 1, "user": {"login": "alice"}, "body": "first",
             "created_at": "2024-03-01T00:00:00Z", "updated_at": "2024-03-01T00:00:00Z"}
        ]"#,
    );
    client.push_response(
        201,
        r#"{"id": 3, "user": {"login": "carol"}, "body": "third",
            "created_at": "2024-03-03T00:00:00Z", "updated_at": "2024-03-03T00:00:00Z"}"#,
    );
    let forge = forge(client.clone());

    let comments = block_on(forge.list_issue_comments(&issue_id(7))).unwrap();
    assert_eq!(comments.len(), 2);
    // Chronological order despite the provider returning newest first.
    assert_eq!(comments[0].body, "first");
    assert_eq!(comments[1].body, "second");

    let comment = block_on(forge.add_issue_comment(
        &issue_id(7),
        CreateComment {
            body: "third".to_string(),
        },
    ))
    .unwrap();
    assert_eq!(comment.author_id, UserId::new("carol"));
    assert_eq!(comment.id.as_str(), "github:acme/widgets:comment:3");

    let request = client.last_request().unwrap();
    assert_eq!(request.method, HttpMethod::Post);
    assert_eq!(request.path, "/repos/acme/widgets/issues/7/comments");
    assert_eq!(body_json(&request)["body"], "third");
}
