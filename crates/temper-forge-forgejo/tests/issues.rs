//! Offline contract tests for Forgejo issue list/get/create/update, issue and
//! pull-request comments routed through the shared item helpers, and the
//! best-effort conditional-update (CAS) path. Every request is served by a
//! recording mock client; no test touches the network.

mod support;

use support::{
    MockHttpClient, OWNER, REPO, block_on, body_json, forge, forge_with, issue_id, pull_id, repo_id,
};
use temper_forge::{
    CreateComment, CreateIssue, ForgeError, IssueQuery, IssueState, ItemListDetails, ItemNumber,
    ItemSort, ItemSortField, SortDirection, UpdateIssue, UserId, Version,
};
use temper_forge_forgejo::{CasMode, HttpMethod};

/// Renders an issue DTO JSON body with overridable labels and trailing fields.
fn issue_json(number: u64, state: &str, labels: &str, extra: &str) -> String {
    format!(
        r#"{{
            "number": {number},
            "title": "Issue {number}",
            "body": "body {number}",
            "state": "{state}",
            "user": {{"login": "author"}},
            "labels": {labels},
            "created_at": "2024-03-01T00:00:00Z",
            "updated_at": "2024-03-02T00:00:00Z"
            {extra}
        }}"#
    )
}

#[test]
fn list_issues_constructs_request_excludes_pulls_and_maps() {
    let client = MockHttpClient::new();
    // The provider may still return a PR-as-issue row; the backend drops it.
    let body = format!(
        "[{},{}]",
        issue_json(1, "open", r#"[{"id":1,"name":"ready"}]"#, ""),
        issue_json(
            2,
            "open",
            "[]",
            r#", "pull_request": {"merged": false, "url": "http://x/pulls/2"}"#
        ),
    );
    client.push_response(200, body);
    // Only the single genuine issue is enriched with its dependency links.
    client.push_response(200, "[]");
    let forge = forge(client.clone());

    let query = IssueQuery {
        state: Some(IssueState::Open),
        labels: vec!["ready".to_string()],
        ..IssueQuery::default()
    };
    let issues = block_on(forge.list_issues(&repo_id(), query)).unwrap();

    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].number, ItemNumber::new(1));
    assert_eq!(issues[0].state, IssueState::Open);
    assert_eq!(issues[0].author_id, UserId::new("author"));
    assert_eq!(issues[0].labels, vec!["ready".to_string()]);
    assert!(issues[0].dependencies.is_empty());

    let request = &client.recorded()[0];
    assert_eq!(request.method, HttpMethod::Get);
    assert_eq!(request.path, format!("/api/v1/repos/{OWNER}/{REPO}/issues"));
    assert!(
        request
            .query
            .contains(&("state".to_string(), "open".to_string()))
    );
    assert!(
        request
            .query
            .contains(&("type".to_string(), "issues".to_string()))
    );
    assert!(
        request
            .query
            .contains(&("labels".to_string(), "ready".to_string()))
    );
    // The dependency lookup targets the surviving issue only.
    assert_eq!(
        client.last_request().unwrap().path,
        format!("/api/v1/repos/{OWNER}/{REPO}/issues/1/dependencies")
    );
}

#[test]
fn list_issues_dependency_detail_is_demand_driven() {
    let client = MockHttpClient::new();
    client.push_response(200, format!("[{}]", issue_json(1, "open", "[]", "")));
    let forge = forge(client.clone());

    let summary = block_on(forge.list_issues(
        &repo_id(),
        IssueQuery {
            state: Some(IssueState::Open),
            details: ItemListDetails::summary(),
            ..IssueQuery::default()
        },
    ))
    .unwrap();
    assert_eq!(summary.len(), 1);
    assert!(summary[0].dependencies.is_empty());
    assert_eq!(client.call_count(), 1);

    client.push_response(200, format!("[{}]", issue_json(1, "open", "[]", "")));
    client.push_response(200, r#"[{"number": 3}]"#);
    let detailed = block_on(forge.list_issues(
        &repo_id(),
        IssueQuery {
            state: Some(IssueState::Open),
            ..IssueQuery::default()
        },
    ))
    .unwrap();
    assert_eq!(detailed[0].dependencies, vec![ItemNumber::new(3)]);
    assert_eq!(client.call_count(), 3);
    assert_eq!(
        client.last_request().unwrap().path,
        format!("/api/v1/repos/{OWNER}/{REPO}/issues/1/dependencies")
    );
}

#[test]
fn list_issues_filters_author_and_assignee_client_side() {
    let client = MockHttpClient::new();
    let row = |number: u64, author: &str| {
        format!(
            r#"{{
                "number": {number},
                "title": "Issue {number}",
                "body": "b",
                "state": "open",
                "user": {{"login": "{author}"}},
                "labels": [],
                "assignees": [{{"login": "bob"}}],
                "created_at": "2024-03-01T00:00:00Z",
                "updated_at": "2024-03-02T00:00:00Z"
            }}"#
        )
    };
    let body = format!("[{},{}]", row(1, "alice"), row(2, "carol"));
    client.push_response(200, body);
    client.push_response(200, "[]"); // dependency enrichment for the one match
    let forge = forge(client.clone());

    let query = IssueQuery {
        author_id: Some(UserId::new("alice")),
        assignee_id: Some(UserId::new("bob")),
        ..IssueQuery::default()
    };
    let issues = block_on(forge.list_issues(&repo_id(), query)).unwrap();
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].number, ItemNumber::new(1));
}

#[test]
fn list_issues_sorts_by_number_descending() {
    let client = MockHttpClient::new();
    let body = format!(
        "[{},{}]",
        issue_json(1, "open", "[]", ""),
        issue_json(2, "open", "[]", ""),
    );
    client.push_response(200, body);
    client.push_response(200, "[]");
    client.push_response(200, "[]");
    let forge = forge(client);

    let query = IssueQuery {
        sort: Some(ItemSort {
            field: ItemSortField::Number,
            direction: SortDirection::Desc,
        }),
        ..IssueQuery::default()
    };
    let issues = block_on(forge.list_issues(&repo_id(), query)).unwrap();
    let numbers: Vec<u64> = issues.iter().map(|issue| issue.number.get()).collect();
    assert_eq!(numbers, vec![2, 1]);
}

#[test]
fn get_issue_by_number_maps_fields_and_dependencies() {
    let client = MockHttpClient::new();
    client.push_response(200, issue_json(7, "open", r#"[{"id":1,"name":"bug"}]"#, ""));
    client.push_response(200, r#"[{"number": 4}, {"number": 2}, {"number": 4}]"#);
    let forge = forge(client.clone());

    let issue = block_on(forge.get_issue_by_number(&repo_id(), ItemNumber::new(7)))
        .unwrap()
        .expect("issue present");
    assert_eq!(issue.number, ItemNumber::new(7));
    assert_eq!(issue.labels, vec!["bug".to_string()]);
    assert_eq!(
        issue.dependencies,
        vec![ItemNumber::new(2), ItemNumber::new(4)]
    );

    let requests = client.recorded();
    assert_eq!(
        requests[0].path,
        format!("/api/v1/repos/{OWNER}/{REPO}/issues/7")
    );
    assert_eq!(
        requests[1].path,
        format!("/api/v1/repos/{OWNER}/{REPO}/issues/7/dependencies")
    );
}

#[test]
fn get_issue_returns_none_for_pull_request_row() {
    let client = MockHttpClient::new();
    client.push_response(
        200,
        issue_json(
            9,
            "open",
            "[]",
            r#", "pull_request": {"merged": false, "url": "http://x/pulls/9"}"#,
        ),
    );
    let forge = forge(client.clone());
    let issue = block_on(forge.get_issue(&issue_id(9))).unwrap();
    assert!(issue.is_none());
    // A PR row is detected from the single read; no dependency enrichment runs.
    assert_eq!(client.call_count(), 1);
}

#[test]
fn get_issue_missing_is_none() {
    let client = MockHttpClient::new();
    client.push_response(404, r#"{"message":"not found"}"#);
    let forge = forge(client);
    assert!(block_on(forge.get_issue(&issue_id(99))).unwrap().is_none());
}

#[test]
fn create_issue_applies_labels_atomically_and_refetches() {
    let client = MockHttpClient::new();
    // Label ids are resolved BEFORE the create so the POST payload carries them;
    // the issue is therefore never visible label-less (no separate PUT follows).
    // A create-then-label sequence would leave a window in which a concurrent
    // daemon scan classifies the label-less issue as the catch-all `intake` kind
    // and stamps it `untriaged` — derailing a freshly materialised `code` child.
    client.push_response(200, r#"[{"id":1,"name":"bug"}]"#); // GET labels (resolve ids)
    client.push_response(
        201,
        issue_json(
            7,
            "open",
            r#"[{"id":1,"name":"bug"}]"#,
            r#", "assignees": [{"login": "bob"}]"#,
        ),
    ); // POST create (labels in the payload)
    client.push_response(
        200,
        issue_json(
            7,
            "open",
            r#"[{"id":1,"name":"bug"}]"#,
            r#", "assignees": [{"login": "bob"}]"#,
        ),
    ); // GET refetch
    client.push_response(200, "[]"); // dependency enrichment
    let forge = forge(client.clone());

    let input = CreateIssue {
        title: "Fix bug".to_string(),
        body: "details".to_string(),
        labels: vec!["bug".to_string()],
        assignees: vec![UserId::new("bob")],
    };
    let issue = block_on(forge.create_issue(&repo_id(), input)).unwrap();
    assert_eq!(issue.labels, vec!["bug".to_string()]);
    assert_eq!(issue.assignees, vec![UserId::new("bob")]);

    let requests = client.recorded();
    assert_eq!(requests.len(), 4);

    assert_eq!(requests[0].method, HttpMethod::Get);
    assert_eq!(
        requests[0].path,
        format!("/api/v1/repos/{OWNER}/{REPO}/labels")
    );

    assert_eq!(requests[1].method, HttpMethod::Post);
    assert_eq!(
        requests[1].path,
        format!("/api/v1/repos/{OWNER}/{REPO}/issues")
    );
    let create_body = body_json(&requests[1]);
    assert_eq!(create_body["title"], "Fix bug");
    assert_eq!(create_body["body"], "details");
    assert_eq!(create_body["assignees"], serde_json::json!(["bob"]));
    // The create payload carries the resolved numeric label ids atomically.
    assert_eq!(create_body["labels"], serde_json::json!([1]));

    // No separate label PUT: all labels were applied on create.
    let label_put = format!("/api/v1/repos/{OWNER}/{REPO}/issues/7/labels");
    assert!(
        !requests
            .iter()
            .any(|r| r.method == HttpMethod::Put && r.path == label_put),
        "create must not issue a separate label PUT when labels apply atomically"
    );

    assert_eq!(requests[2].method, HttpMethod::Get);
    assert_eq!(
        requests[2].path,
        format!("/api/v1/repos/{OWNER}/{REPO}/issues/7")
    );
}

#[test]
fn create_issue_without_labels_skips_label_call() {
    let client = MockHttpClient::new();
    client.push_response(201, issue_json(8, "open", "[]", "")); // POST create
    client.push_response(200, issue_json(8, "open", "[]", "")); // GET refetch
    client.push_response(200, "[]"); // dependency enrichment
    let forge = forge(client.clone());

    let input = CreateIssue {
        title: "No labels".to_string(),
        body: String::new(),
        labels: Vec::new(),
        assignees: Vec::new(),
    };
    let _ = block_on(forge.create_issue(&repo_id(), input)).unwrap();

    let requests = client.recorded();
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[0].method, HttpMethod::Post);
    assert_eq!(requests[1].method, HttpMethod::Get);
    assert_eq!(requests[2].method, HttpMethod::Get); // dependencies
}

#[test]
fn update_issue_patches_then_sequences_labels_and_assignees() {
    let client = MockHttpClient::new();
    // GET current carries an existing assignee so the replacement set is derived.
    client.push_response(
        200,
        issue_json(7, "open", "[]", r#", "assignees": [{"login": "old"}]"#),
    );
    client.push_response(200, "{}"); // PATCH issue (title/state)
    // One label-id read resolves names for set, remove, and add.
    client.push_response(
        200,
        r#"[{"id":3,"name":"base"},{"id":9,"name":"stale"},{"id":1,"name":"ready"}]"#,
    ); // GET labels (resolve ids)
    client.push_response(200, "[]"); // PUT set labels (by id)
    client.push_response(200, "{}"); // DELETE label by id
    client.push_response(200, "[]"); // POST add labels (by id)
    client.push_response(200, "{}"); // PATCH issue (assignees)
    client.push_response(
        200,
        issue_json(
            7,
            "closed",
            r#"[{"id":1,"name":"ready"}]"#,
            r#", "assignees": [{"login": "bob"}]"#,
        ),
    ); // GET refetch
    client.push_response(200, "[]"); // dependency enrichment
    let forge = forge(client.clone());

    let input = UpdateIssue {
        title: Some("Renamed".to_string()),
        state: Some(IssueState::Closed),
        set_labels: Some(vec!["base".to_string()]),
        add_labels: vec!["ready".to_string()],
        remove_labels: vec!["stale".to_string()],
        add_assignees: vec![UserId::new("bob")],
        remove_assignees: vec![UserId::new("old")],
        ..UpdateIssue::default()
    };
    let issue = block_on(forge.update_issue(&issue_id(7), input)).unwrap();
    assert_eq!(issue.state, IssueState::Closed);

    let requests = client.recorded();
    // GET(current), PATCH(edit), GET(label ids), PUT(set), DELETE(remove),
    // POST(add), PATCH(assignees), GET(refetch), GET(deps).
    assert_eq!(requests.len(), 9);

    assert_eq!(requests[0].method, HttpMethod::Get);

    assert_eq!(requests[1].method, HttpMethod::Patch);
    assert_eq!(
        requests[1].path,
        format!("/api/v1/repos/{OWNER}/{REPO}/issues/7")
    );
    let edit = body_json(&requests[1]);
    assert_eq!(edit["title"], "Renamed");
    assert_eq!(edit["state"], "closed");

    // A single label-id read precedes the label writes, which send numeric ids.
    assert_eq!(requests[2].method, HttpMethod::Get);
    assert_eq!(
        requests[2].path,
        format!("/api/v1/repos/{OWNER}/{REPO}/labels")
    );

    // Label sequencing: set (PUT, by id) → remove-by-id (DELETE) → add (POST, by id).
    assert_eq!(requests[3].method, HttpMethod::Put);
    assert_eq!(
        requests[3].path,
        format!("/api/v1/repos/{OWNER}/{REPO}/issues/7/labels")
    );
    assert_eq!(body_json(&requests[3])["labels"], serde_json::json!([3]));

    assert_eq!(requests[4].method, HttpMethod::Delete);
    assert_eq!(
        requests[4].path,
        format!("/api/v1/repos/{OWNER}/{REPO}/issues/7/labels/9")
    );
    assert_eq!(requests[5].method, HttpMethod::Post);
    assert_eq!(body_json(&requests[5])["labels"], serde_json::json!([1]));

    // Assignee replacement is `current − remove + add` = {bob}.
    assert_eq!(requests[6].method, HttpMethod::Patch);
    assert_eq!(
        requests[6].path,
        format!("/api/v1/repos/{OWNER}/{REPO}/issues/7")
    );
    assert_eq!(
        body_json(&requests[6])["assignees"],
        serde_json::json!(["bob"])
    );

    assert_eq!(requests[7].method, HttpMethod::Get); // refetch
}

#[test]
fn update_issue_missing_is_not_found() {
    let client = MockHttpClient::new();
    client.push_response(404, r#"{"message":"not found"}"#);
    let forge = forge(client);
    let result = block_on(forge.update_issue(&issue_id(5), UpdateIssue::default()));
    assert!(matches!(result, Err(ForgeError::NotFound(_))));
}

#[test]
fn update_issue_rejects_pull_request_row() {
    let client = MockHttpClient::new();
    client.push_response(
        200,
        issue_json(
            5,
            "open",
            "[]",
            r#", "pull_request": {"merged": false, "url": "http://x/pulls/5"}"#,
        ),
    );
    let forge = forge(client.clone());
    let result = block_on(forge.update_issue(&issue_id(5), UpdateIssue::default()));
    assert!(matches!(result, Err(ForgeError::NotFound(_))));
    // Only the read happened; no mutation request was emitted.
    assert_eq!(client.call_count(), 1);
}

#[test]
fn conditional_update_conflicts_without_mutating_when_validator_changed() {
    // First read observes etag-a → Version::INITIAL, the token the caller holds.
    let client = MockHttpClient::new();
    client.push_response_with_etag(issue_json(7, "open", "[]", ""), "etag-a");
    client.push_response(200, "[]"); // dependency enrichment for the initial read
    let forge = forge(client.clone());
    let observed = block_on(forge.get_issue(&issue_id(7)))
        .unwrap()
        .expect("issue present")
        .version;
    assert_eq!(observed, Version::INITIAL);
    // get_issue issues the read plus a dependency enrichment call.
    assert_eq!(client.call_count(), 2);

    // The conditional update re-reads and sees a changed validator (etag-b), so
    // it must conflict and emit no mutation request.
    client.push_response_with_etag(issue_json(7, "open", "[]", ""), "etag-b");
    let input = UpdateIssue {
        title: Some("Renamed".to_string()),
        expected_version: Some(observed),
        ..UpdateIssue::default()
    };
    let result = block_on(forge.update_issue(&issue_id(7), input));
    assert!(matches!(result, Err(ForgeError::Conflict(_))));
    // Exactly one extra request (the fresh read); nothing was mutated.
    assert_eq!(client.call_count(), 3);
    assert_eq!(client.last_request().unwrap().method, HttpMethod::Get);
}

#[test]
fn conditional_update_succeeds_when_validator_matches() {
    let client = MockHttpClient::new();
    client.push_response_with_etag(issue_json(7, "open", "[]", ""), "etag-a");
    client.push_response(200, "[]"); // dependency enrichment for the initial read
    let forge = forge(client.clone());
    let observed = block_on(forge.get_issue(&issue_id(7)))
        .unwrap()
        .expect("issue present")
        .version;

    // Re-read returns the same validator, so the precondition holds and the
    // title patch is applied.
    client.push_response_with_etag(issue_json(7, "open", "[]", ""), "etag-a"); // GET current
    client.push_response(200, "{}"); // PATCH title
    client.push_response_with_etag(issue_json(7, "open", "[]", ""), "etag-a"); // GET refetch
    client.push_response(200, "[]"); // dependency enrichment
    let input = UpdateIssue {
        title: Some("Renamed".to_string()),
        expected_version: Some(observed),
        ..UpdateIssue::default()
    };
    let issue = block_on(forge.update_issue(&issue_id(7), input)).unwrap();
    // The validator is unchanged, so the version is stable across the write.
    assert_eq!(issue.version, observed);

    let methods: Vec<HttpMethod> = client
        .recorded()
        .iter()
        .skip(2) // the initial get_issue read + its dependency call
        .map(|request| request.method)
        .collect();
    assert_eq!(
        methods,
        vec![
            HttpMethod::Get,
            HttpMethod::Patch,
            HttpMethod::Get,
            HttpMethod::Get
        ]
    );
}

#[test]
fn strict_cas_rejects_conditional_update_without_validator() {
    // No ETag header, so the only validator is the weak `updated_at`. Under
    // strict mode an absent provider validator must reject the conditional write.
    let client = MockHttpClient::new();
    // Plain 200 with no ETag → response_validator is None.
    client.push_response(200, issue_json(7, "open", "[]", ""));
    let forge = forge_with(client.clone(), CasMode::Strict);

    let input = UpdateIssue {
        title: Some("Renamed".to_string()),
        expected_version: Some(Version::INITIAL),
        ..UpdateIssue::default()
    };
    let result = block_on(forge.update_issue(&issue_id(7), input));
    assert!(matches!(result, Err(ForgeError::InvalidRequest(_))));
    // Only the read happened; strict mode emitted no mutation.
    assert_eq!(client.call_count(), 1);
}

#[test]
fn issue_comments_list_in_order_and_add() {
    let client = MockHttpClient::new();
    client.push_response(
        200,
        r#"[
            {"id": 2, "user": {"login": "bob"}, "body": "second", "created_at": "2024-03-02T00:00:00Z", "updated_at": "2024-03-02T00:00:00Z"},
            {"id": 1, "user": {"login": "carol"}, "body": "first", "created_at": "2024-03-01T00:00:00Z", "updated_at": "2024-03-01T00:00:00Z"}
        ]"#,
    );
    let backend = forge(client.clone());
    let comments = block_on(backend.list_issue_comments(&issue_id(7))).unwrap();
    assert_eq!(comments.len(), 2);
    assert_eq!(comments[0].body, "first");
    assert_eq!(comments[1].body, "second");
    assert_eq!(
        client.last_request().unwrap().path,
        format!("/api/v1/repos/{OWNER}/{REPO}/issues/7/comments")
    );

    let client = MockHttpClient::new();
    client.push_response(
        201,
        r#"{"id": 3, "user": {"login": "bob"}, "body": "noted", "created_at": "2024-03-03T00:00:00Z", "updated_at": "2024-03-03T00:00:00Z"}"#,
    );
    let backend = forge(client.clone());
    let comment = block_on(backend.add_issue_comment(
        &issue_id(7),
        CreateComment {
            body: "noted".to_string(),
        },
    ))
    .unwrap();
    assert_eq!(comment.body, "noted");
    let request = client.last_request().unwrap();
    assert_eq!(request.method, HttpMethod::Post);
    assert_eq!(
        request.path,
        format!("/api/v1/repos/{OWNER}/{REPO}/issues/7/comments")
    );
    assert_eq!(body_json(&request)["body"], "noted");
}

#[test]
fn pull_request_comments_route_through_shared_helpers() {
    // The PR comment trait methods reuse the same issue-comment endpoints, keyed
    // by the parsed pull-request number.
    let client = MockHttpClient::new();
    client.push_response(
        200,
        r#"[{"id": 1, "user": {"login": "carol"}, "body": "hi", "created_at": "2024-03-01T00:00:00Z", "updated_at": "2024-03-01T00:00:00Z"}]"#,
    );
    let backend = forge(client.clone());
    let comments = block_on(backend.list_pull_request_comments(&pull_id(42))).unwrap();
    assert_eq!(comments.len(), 1);
    assert_eq!(
        client.last_request().unwrap().path,
        format!("/api/v1/repos/{OWNER}/{REPO}/issues/42/comments")
    );
}
