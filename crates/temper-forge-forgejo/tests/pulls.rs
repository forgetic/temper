//! Offline contract tests for Forgejo pull-request list/get/create/update and
//! pull-request comments. Every request is served by a recording mock client;
//! no test touches the network.

mod support;

use support::{
    MockHttpClient, OWNER, REPO, block_on, body_json, forge, forge_with_web_ui, pull_id, repo_id,
};
use temper_forge::{
    BranchRef, CreateComment, CreatePullRequest, ItemListDetails, ItemNumber, ItemSort,
    ItemSortField, PullRequestQuery, PullRequestState, PullRequestUpdateState, SortDirection,
    UpdatePullRequest, UserId,
};
use temper_forge_forgejo::{ForgejoConfig, ForgejoForge, HttpMethod};

/// Renders a pull-request DTO JSON body with overridable fields.
fn pr_json(number: u64, state: &str, labels: &str, extra: &str) -> String {
    format!(
        r#"{{
            "number": {number},
            "title": "PR {number}",
            "body": "body {number}",
            "state": "{state}",
            "merged": false,
            "user": {{"login": "author"}},
            "head": {{"ref": "feature-{number}", "sha": "head{number}"}},
            "base": {{"ref": "main", "sha": "base{number}"}},
            "labels": {labels},
            "created_at": "2024-03-01T00:00:00Z",
            "updated_at": "2024-03-02T00:00:00Z"
            {extra}
        }}"#
    )
}

#[test]
fn unlabelled_open_pull_request_query_does_not_fetch_closed_history() {
    let client = MockHttpClient::new();
    client.push_response(200, "[]");
    let forge = forge_with_web_ui(client.clone());

    let query = PullRequestQuery {
        state: Some(PullRequestState::Open),
        details: ItemListDetails::summary(),
        ..PullRequestQuery::default()
    };
    let pulls = block_on(forge.list_pull_requests(&repo_id(), query)).unwrap();
    assert!(pulls.is_empty());

    let request = client.last_request().unwrap();
    assert_eq!(request.method, HttpMethod::Get);
    assert_eq!(request.path, format!("/api/v1/repos/{OWNER}/{REPO}/pulls"));
    assert!(
        request
            .query
            .contains(&("state".to_string(), "open".to_string()))
    );
    assert!(
        !request
            .query
            .contains(&("state".to_string(), "all".to_string()))
    );
    assert!(
        !client
            .recorded()
            .iter()
            .any(|request| request.path.contains("/user/login"))
    );
}

#[test]
fn list_pull_requests_dependency_detail_is_demand_driven() {
    let client = MockHttpClient::new();
    client.push_response(200, format!("[{}]", pr_json(1, "open", "[]", "")));
    let forge = forge(client.clone());

    let summary = block_on(forge.list_pull_requests(
        &repo_id(),
        PullRequestQuery {
            state: Some(PullRequestState::Open),
            details: ItemListDetails::summary(),
            ..PullRequestQuery::default()
        },
    ))
    .unwrap();
    assert_eq!(summary.len(), 1);
    assert!(summary[0].dependencies.is_empty());
    assert_eq!(client.call_count(), 1);

    client.push_response(200, format!("[{}]", pr_json(1, "open", "[]", "")));
    client.push_response(200, r#"[{"number": 4}]"#);
    let detailed = block_on(forge.list_pull_requests(
        &repo_id(),
        PullRequestQuery {
            state: Some(PullRequestState::Open),
            ..PullRequestQuery::default()
        },
    ))
    .unwrap();
    assert_eq!(detailed[0].dependencies, vec![ItemNumber::new(4)]);
    assert_eq!(client.call_count(), 3);
    assert_eq!(
        client.last_request().unwrap().path,
        format!("/api/v1/repos/{OWNER}/{REPO}/issues/1/dependencies")
    );
}

#[test]
fn list_pull_requests_maps_merged_state_to_closed_query() {
    let client = MockHttpClient::new();
    client.push_response(200, "[]");
    let forge = forge(client.clone());

    let query = PullRequestQuery {
        state: Some(PullRequestState::Merged),
        ..PullRequestQuery::default()
    };
    let _ = block_on(forge.list_pull_requests(&repo_id(), query)).unwrap();

    let request = client.last_request().unwrap();
    assert!(
        request
            .query
            .contains(&("state".to_string(), "closed".to_string()))
    );
}

#[test]
fn list_pull_requests_sorts_by_number_descending() {
    let client = MockHttpClient::new();
    let body = format!(
        "[{},{}]",
        pr_json(1, "open", "[]", ""),
        pr_json(2, "open", "[]", ""),
    );
    client.push_response(200, body);
    // Both pull requests match (no filter), so each is enriched with its
    // dependency links.
    client.push_response(200, "[]");
    client.push_response(200, "[]");
    let forge = forge(client);

    let query = PullRequestQuery {
        sort: Some(ItemSort {
            field: ItemSortField::Number,
            direction: SortDirection::Desc,
        }),
        ..PullRequestQuery::default()
    };
    let pulls = block_on(forge.list_pull_requests(&repo_id(), query)).unwrap();
    let numbers: Vec<u64> = pulls.iter().map(|pr| pr.number.get()).collect();
    assert_eq!(numbers, vec![2, 1]);
}

#[test]
fn list_pull_requests_paginates_until_short_page() {
    // page_limit 1: a full page (== limit) keeps paging; a short (empty) page
    // stops it, so the two pulls are concatenated across three requests.
    let client = MockHttpClient::new();
    client.push_response(200, format!("[{}]", pr_json(1, "open", "[]", "")));
    client.push_response(200, format!("[{}]", pr_json(2, "open", "[]", "")));
    client.push_response(200, "[]");
    // Dependency enrichment for the two listed pull requests, after paging.
    client.push_response(200, "[]");
    client.push_response(200, "[]");
    let config = ForgejoConfig::new("https://forge.example.com", "test-token").with_page_limit(1);
    let forge = ForgejoForge::with_client(config, client.clone());

    let pulls =
        block_on(forge.list_pull_requests(&repo_id(), PullRequestQuery::default())).unwrap();
    assert_eq!(pulls.len(), 2);
    // Three list pages followed by two dependency lookups.
    assert_eq!(client.call_count(), 5);
    let pages: Vec<String> = client
        .recorded()
        .iter()
        .filter_map(|request| {
            request
                .query
                .iter()
                .find(|(key, _)| key == "page")
                .map(|(_, value)| value.clone())
        })
        .collect();
    // Only the list requests carry a page parameter; dependency reads do not.
    assert_eq!(pages, vec!["1", "2", "3"]);
}

#[test]
fn get_pull_request_by_number_maps_fields() {
    let client = MockHttpClient::new();
    client.push_response(200, pr_json(42, "open", r#"[{"id":1,"name":"ready"}]"#, ""));
    // The read enriches the pull request with its dependency links.
    client.push_response(200, r#"[{"number": 7}, {"number": 3}, {"number": 7}]"#);
    let forge = forge(client.clone());

    let pull = block_on(forge.get_pull_request_by_number(&repo_id(), ItemNumber::new(42)))
        .unwrap()
        .expect("pull request present");
    assert_eq!(pull.number, ItemNumber::new(42));
    assert_eq!(pull.state, PullRequestState::Open);
    assert_eq!(pull.author_id, UserId::new("author"));
    assert_eq!(pull.source.branch, "feature-42");
    assert_eq!(pull.target.branch, "main");
    assert_eq!(pull.head_sha.as_deref(), Some("head42"));
    assert_eq!(pull.base_sha.as_deref(), Some("base42"));
    assert_eq!(pull.labels, vec!["ready".to_string()]);
    // Dependency item numbers are sorted and deduplicated.
    assert_eq!(
        pull.dependencies,
        vec![ItemNumber::new(3), ItemNumber::new(7)]
    );

    let requests = client.recorded();
    assert_eq!(requests[0].method, HttpMethod::Get);
    assert_eq!(
        requests[0].path,
        format!("/api/v1/repos/{OWNER}/{REPO}/pulls/42")
    );
    assert_eq!(
        requests[1].path,
        format!("/api/v1/repos/{OWNER}/{REPO}/issues/42/dependencies")
    );
}

#[test]
fn get_pull_request_missing_is_none() {
    let client = MockHttpClient::new();
    client.push_response(404, r#"{"message":"not found"}"#);
    let forge = forge(client);
    assert!(
        block_on(forge.get_pull_request(&pull_id(99)))
            .unwrap()
            .is_none()
    );
}

#[test]
fn create_pull_request_posts_then_applies_labels_and_assignees() {
    let client = MockHttpClient::new();
    client.push_response(201, pr_json(42, "open", "[]", "")); // POST create
    client.push_response(200, r#"[{"id":1,"name":"ready"}]"#); // GET labels (resolve ids)
    client.push_response(200, "[]"); // PUT labels (by id)
    client.push_response(200, "{}"); // PATCH assignees
    client.push_response(
        200,
        pr_json(
            42,
            "open",
            r#"[{"id":1,"name":"ready"}]"#,
            r#", "assignees": [{"login": "bob"}]"#,
        ),
    ); // GET refetch
    let forge = forge(client.clone());

    let input = CreatePullRequest {
        title: "Add widget".to_string(),
        body: "details".to_string(),
        source: BranchRef {
            repository_id: repo_id(),
            branch: "feature".to_string(),
        },
        target: BranchRef {
            repository_id: repo_id(),
            branch: "main".to_string(),
        },
        labels: vec!["ready".to_string()],
        assignees: vec![UserId::new("bob")],
    };
    let pull = block_on(forge.create_pull_request(&repo_id(), input)).unwrap();
    assert_eq!(pull.labels, vec!["ready".to_string()]);
    assert_eq!(pull.assignees, vec![UserId::new("bob")]);

    let requests = client.recorded();
    assert_eq!(requests.len(), 5);

    assert_eq!(requests[0].method, HttpMethod::Post);
    assert_eq!(
        requests[0].path,
        format!("/api/v1/repos/{OWNER}/{REPO}/pulls")
    );
    let create_body = body_json(&requests[0]);
    assert_eq!(create_body["title"], "Add widget");
    assert_eq!(create_body["head"], "feature");
    assert_eq!(create_body["base"], "main");
    assert_eq!(create_body["body"], "details");

    // Label names are resolved to numeric ids (Forgejo rejects a name array).
    assert_eq!(requests[1].method, HttpMethod::Get);
    assert_eq!(
        requests[1].path,
        format!("/api/v1/repos/{OWNER}/{REPO}/labels")
    );

    assert_eq!(requests[2].method, HttpMethod::Put);
    assert_eq!(
        requests[2].path,
        format!("/api/v1/repos/{OWNER}/{REPO}/issues/42/labels")
    );
    assert_eq!(body_json(&requests[2])["labels"], serde_json::json!([1]));

    assert_eq!(requests[3].method, HttpMethod::Patch);
    assert_eq!(
        requests[3].path,
        format!("/api/v1/repos/{OWNER}/{REPO}/issues/42")
    );
    assert_eq!(
        body_json(&requests[3])["assignees"],
        serde_json::json!(["bob"])
    );

    assert_eq!(requests[4].method, HttpMethod::Get);
    assert_eq!(
        requests[4].path,
        format!("/api/v1/repos/{OWNER}/{REPO}/pulls/42")
    );
}

#[test]
fn create_pull_request_without_labels_or_assignees_skips_issue_calls() {
    let client = MockHttpClient::new();
    client.push_response(201, pr_json(7, "open", "[]", "")); // POST create
    client.push_response(200, pr_json(7, "open", "[]", "")); // GET refetch
    let forge = forge(client.clone());

    let input = CreatePullRequest {
        title: "No metadata".to_string(),
        body: String::new(),
        source: BranchRef {
            repository_id: repo_id(),
            branch: "feature".to_string(),
        },
        target: BranchRef {
            repository_id: repo_id(),
            branch: "main".to_string(),
        },
        labels: Vec::new(),
        assignees: Vec::new(),
    };
    let _ = block_on(forge.create_pull_request(&repo_id(), input)).unwrap();

    let requests = client.recorded();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].method, HttpMethod::Post);
    assert_eq!(requests[1].method, HttpMethod::Get);
}

#[test]
fn update_pull_request_patches_fields_then_labels_and_assignees() {
    let client = MockHttpClient::new();
    client.push_response(200, pr_json(42, "open", "[]", "")); // GET current
    client.push_response(200, "{}"); // PATCH pull (title/state)
    client.push_response(200, r#"[{"id":9,"name":"done"}]"#); // GET labels (resolve ids)
    client.push_response(200, "[]"); // PUT labels (by id)
    client.push_response(200, "{}"); // PATCH issue (assignees)
    client.push_response(
        200,
        pr_json(
            42,
            "closed",
            r#"[{"id":9,"name":"done"}]"#,
            r#", "assignees": [{"login": "bob"}]"#,
        ),
    ); // GET refetch
    let forge = forge(client.clone());

    let input = UpdatePullRequest {
        title: Some("Renamed".to_string()),
        state: Some(PullRequestUpdateState::Closed),
        set_labels: Some(vec!["done".to_string()]),
        add_assignees: vec![UserId::new("bob")],
        ..UpdatePullRequest::default()
    };
    let pull = block_on(forge.update_pull_request(&pull_id(42), input)).unwrap();
    assert_eq!(pull.state, PullRequestState::Closed);

    let requests = client.recorded();
    assert_eq!(requests.len(), 6);

    assert_eq!(requests[0].method, HttpMethod::Get);

    assert_eq!(requests[1].method, HttpMethod::Patch);
    assert_eq!(
        requests[1].path,
        format!("/api/v1/repos/{OWNER}/{REPO}/pulls/42")
    );
    let edit = body_json(&requests[1]);
    assert_eq!(edit["title"], "Renamed");
    assert_eq!(edit["state"], "closed");

    // A single label-id read precedes the label write, which sends numeric ids.
    assert_eq!(requests[2].method, HttpMethod::Get);
    assert_eq!(
        requests[2].path,
        format!("/api/v1/repos/{OWNER}/{REPO}/labels")
    );

    assert_eq!(requests[3].method, HttpMethod::Put);
    assert_eq!(
        requests[3].path,
        format!("/api/v1/repos/{OWNER}/{REPO}/issues/42/labels")
    );
    assert_eq!(body_json(&requests[3])["labels"], serde_json::json!([9]));

    assert_eq!(requests[4].method, HttpMethod::Patch);
    assert_eq!(
        requests[4].path,
        format!("/api/v1/repos/{OWNER}/{REPO}/issues/42")
    );
    assert_eq!(
        body_json(&requests[4])["assignees"],
        serde_json::json!(["bob"])
    );

    assert_eq!(requests[5].method, HttpMethod::Get);
}

#[test]
fn update_pull_request_missing_is_not_found() {
    let client = MockHttpClient::new();
    client.push_response(404, r#"{"message":"not found"}"#);
    let forge = forge(client);
    let result = block_on(forge.update_pull_request(&pull_id(5), UpdatePullRequest::default()));
    assert!(matches!(result, Err(temper_forge::ForgeError::NotFound(_))));
}

#[test]
fn pull_request_comments_list_in_order_and_add() {
    let client = MockHttpClient::new();
    client.push_response(
        200,
        r#"[
            {"id": 2, "user": {"login": "bob"}, "body": "second", "created_at": "2024-03-02T00:00:00Z", "updated_at": "2024-03-02T00:00:00Z"},
            {"id": 1, "user": {"login": "carol"}, "body": "first", "created_at": "2024-03-01T00:00:00Z", "updated_at": "2024-03-01T00:00:00Z"}
        ]"#,
    );
    let backend = forge(client.clone());
    let comments = block_on(backend.list_pull_request_comments(&pull_id(42))).unwrap();
    assert_eq!(comments.len(), 2);
    assert_eq!(comments[0].body, "first");
    assert_eq!(comments[1].body, "second");
    assert_eq!(
        client.last_request().unwrap().path,
        format!("/api/v1/repos/{OWNER}/{REPO}/issues/42/comments")
    );

    let client = MockHttpClient::new();
    client.push_response(
        201,
        r#"{"id": 3, "user": {"login": "bob"}, "body": "looks good", "created_at": "2024-03-03T00:00:00Z", "updated_at": "2024-03-03T00:00:00Z"}"#,
    );
    let backend = forge(client.clone());
    let comment = block_on(backend.add_pull_request_comment(
        &pull_id(42),
        CreateComment {
            body: "looks good".to_string(),
        },
    ))
    .unwrap();
    assert_eq!(comment.body, "looks good");
    let request = client.last_request().unwrap();
    assert_eq!(request.method, HttpMethod::Post);
    assert_eq!(
        request.path,
        format!("/api/v1/repos/{OWNER}/{REPO}/issues/42/comments")
    );
    assert_eq!(body_json(&request)["body"], "looks good");
}
