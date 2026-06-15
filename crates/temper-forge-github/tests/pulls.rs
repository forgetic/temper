//! Offline contract tests for pull-request list/get/create/update and
//! reviewer requests.

mod support;

use support::{MockHttpClient, block_on, body_json, forge, pull_id, repo_id};
use temper_forge_github::HttpMethod;
use temper_forge_model::{
    BranchRef, CreatePullRequest, ForgeError, ItemNumber, PullRequestQuery, PullRequestState,
    PullRequestUpdateState, RequestReviewers, UpdatePullRequest, UserId,
};

fn pull_json(number: u64, state: &str, merged_at: &str) -> String {
    let merged_at = if merged_at.is_empty() {
        "null".to_string()
    } else {
        format!("\"{merged_at}\"")
    };
    format!(
        r#"{{
            "number": {number},
            "title": "a pull",
            "body": "pr body",
            "state": "{state}",
            "merged_at": {merged_at},
            "merge_commit_sha": null,
            "user": {{"login": "author"}},
            "head": {{"label": "acme:feature", "ref": "feature", "sha": "headsha"}},
            "base": {{"label": "acme:main", "ref": "main", "sha": "basesha"}},
            "labels": [{{"id": 1, "name": "ready"}}],
            "assignees": [{{"login": "bob"}}],
            "requested_reviewers": [{{"login": "carol"}}],
            "created_at": "2024-03-01T00:00:00Z",
            "updated_at": "2024-03-02T00:00:00Z"
        }}"#
    )
}

#[test]
fn list_pull_requests_maps_states_and_filters_labels_client_side() {
    let client = MockHttpClient::new();
    client.push_response(
        200,
        format!(
            "[{}, {}, {}]",
            pull_json(1, "open", ""),
            pull_json(2, "closed", "2024-04-01T00:00:00Z"),
            r#"{
                "number": 3,
                "title": "unlabelled",
                "state": "open",
                "user": {"login": "author"},
                "labels": [],
                "created_at": "2024-03-01T00:00:00Z",
                "updated_at": "2024-03-02T00:00:00Z"
            }"#
        ),
    );
    let forge = forge(client.clone());

    let pulls = block_on(forge.list_pull_requests(
        &repo_id(),
        PullRequestQuery {
            labels: vec!["ready".to_string()],
            ..PullRequestQuery::default()
        },
    ))
    .unwrap();
    // #3 lacks the label; #1 and #2 carry it.
    assert_eq!(pulls.len(), 2);
    assert_eq!(pulls[0].number, ItemNumber::new(1));
    assert_eq!(pulls[0].state, PullRequestState::Open);
    // A closed row with merged_at is portable Merged.
    assert_eq!(pulls[1].state, PullRequestState::Merged);

    let request = client.last_request().unwrap();
    assert_eq!(request.path, "/repos/acme/widgets/pulls");
    assert!(
        request
            .query
            .iter()
            .any(|(key, value)| key == "state" && value == "all")
    );
}

#[test]
fn list_pull_requests_separates_closed_from_merged() {
    let client = MockHttpClient::new();
    client.push_response(
        200,
        format!(
            "[{}, {}]",
            pull_json(1, "closed", ""),
            pull_json(2, "closed", "2024-04-01T00:00:00Z")
        ),
    );
    let forge = forge(client.clone());

    let closed = block_on(forge.list_pull_requests(
        &repo_id(),
        PullRequestQuery {
            state: Some(PullRequestState::Closed),
            ..PullRequestQuery::default()
        },
    ))
    .unwrap();
    assert_eq!(closed.len(), 1);
    assert_eq!(closed[0].number, ItemNumber::new(1));

    // The provider was asked for `state=closed` (covers closed and merged).
    let request = client.last_request().unwrap();
    assert!(
        request
            .query
            .iter()
            .any(|(key, value)| key == "state" && value == "closed")
    );
}

#[test]
fn get_pull_request_by_number_maps_404_to_none() {
    let client = MockHttpClient::new();
    client.push_response(200, pull_json(42, "open", ""));
    client.push_response(404, r#"{"message": "Not Found"}"#);
    let forge = forge(client.clone());

    let found = block_on(forge.get_pull_request_by_number(&repo_id(), ItemNumber::new(42)))
        .unwrap()
        .unwrap();
    assert_eq!(found.id, pull_id(42));
    assert_eq!(found.source.branch, "feature");
    assert_eq!(found.head_sha.as_deref(), Some("headsha"));
    assert_eq!(found.requested_reviewers, vec![UserId::new("carol")]);
    assert_eq!(client.recorded()[0].path, "/repos/acme/widgets/pulls/42");

    let missing =
        block_on(forge.get_pull_request_by_number(&repo_id(), ItemNumber::new(99))).unwrap();
    assert!(missing.is_none());
}

#[test]
fn create_pull_request_applies_labels_and_assignees_after_create() {
    let client = MockHttpClient::new();
    client.push_response(201, pull_json(42, "open", "")); // create
    client.push_response(200, r#"[{"id": 1, "name": "ready"}]"#); // PUT labels
    client.push_response(200, pull_json(42, "open", "")); // PATCH assignees
    client.push_response(200, pull_json(42, "open", "")); // re-read
    let forge = forge(client.clone());

    let pull = block_on(forge.create_pull_request(
        &repo_id(),
        CreatePullRequest {
            title: "a pull".to_string(),
            body: "pr body".to_string(),
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
        },
    ))
    .unwrap();
    assert_eq!(pull.id, pull_id(42));

    let recorded = client.recorded();
    assert_eq!(recorded.len(), 4);
    assert_eq!(recorded[0].method, HttpMethod::Post);
    assert_eq!(recorded[0].path, "/repos/acme/widgets/pulls");
    let payload = body_json(&recorded[0]);
    assert_eq!(payload["title"], "a pull");
    assert_eq!(payload["head"], "feature");
    assert_eq!(payload["base"], "main");

    assert_eq!(recorded[1].method, HttpMethod::Put);
    assert_eq!(recorded[1].path, "/repos/acme/widgets/issues/42/labels");
    assert_eq!(recorded[2].method, HttpMethod::Patch);
    assert_eq!(recorded[2].path, "/repos/acme/widgets/issues/42");
}

#[test]
fn create_pull_request_without_metadata_skips_item_updates() {
    let client = MockHttpClient::new();
    client.push_response(201, pull_json(42, "open", "")); // create
    client.push_response(200, pull_json(42, "open", "")); // re-read
    let forge = forge(client.clone());

    block_on(forge.create_pull_request(
        &repo_id(),
        CreatePullRequest {
            title: "a pull".to_string(),
            body: "pr body".to_string(),
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
        },
    ))
    .unwrap();

    assert_eq!(client.call_count(), 2);
}

#[test]
fn update_pull_request_patches_state() {
    let client = MockHttpClient::new();
    client.push_response(200, pull_json(42, "open", "")); // current read
    client.push_response(200, pull_json(42, "closed", "")); // PATCH echo
    client.push_response(200, pull_json(42, "closed", "")); // final re-read
    let forge = forge(client.clone());

    let pull = block_on(forge.update_pull_request(
        &pull_id(42),
        UpdatePullRequest {
            state: Some(PullRequestUpdateState::Closed),
            ..UpdatePullRequest::default()
        },
    ))
    .unwrap();
    assert_eq!(pull.state, PullRequestState::Closed);

    let recorded = client.recorded();
    assert_eq!(recorded.len(), 3);
    assert_eq!(recorded[1].method, HttpMethod::Patch);
    assert_eq!(recorded[1].path, "/repos/acme/widgets/pulls/42");
    assert_eq!(body_json(&recorded[1])["state"], "closed");
}

#[test]
fn request_reviewers_posts_logins_and_rereads() {
    let client = MockHttpClient::new();
    client.push_response(201, pull_json(42, "open", "")); // POST reviewers
    client.push_response(200, pull_json(42, "open", "")); // re-read
    let forge = forge(client.clone());

    let pull = block_on(forge.request_pull_request_reviewers(
        &pull_id(42),
        RequestReviewers {
            reviewers: vec![UserId::new("carol")],
        },
    ))
    .unwrap();
    assert_eq!(pull.requested_reviewers, vec![UserId::new("carol")]);

    let recorded = client.recorded();
    assert_eq!(
        recorded[0].path,
        "/repos/acme/widgets/pulls/42/requested_reviewers"
    );
    assert_eq!(
        body_json(&recorded[0])["reviewers"],
        serde_json::json!(["carol"])
    );
}

#[test]
fn request_reviewers_is_idempotent_when_already_requested() {
    let client = MockHttpClient::new();
    client.push_response(
        422,
        r#"{"message": "Reviews may only be requested from collaborators."}"#,
    );
    client.push_response(200, pull_json(42, "open", "")); // re-read shows carol requested
    let forge = forge(client);

    let pull = block_on(forge.request_pull_request_reviewers(
        &pull_id(42),
        RequestReviewers {
            reviewers: vec![UserId::new("carol")],
        },
    ))
    .unwrap();
    assert_eq!(pull.requested_reviewers, vec![UserId::new("carol")]);
}

#[test]
fn request_reviewers_surfaces_definite_failures() {
    let client = MockHttpClient::new();
    client.push_response(
        422,
        r#"{"message": "Reviews may only be requested from collaborators."}"#,
    );
    // The re-read shows the reviewer was NOT applied.
    client.push_response(200, pull_json(42, "open", ""));
    let forge = forge(client);

    let error = block_on(forge.request_pull_request_reviewers(
        &pull_id(42),
        RequestReviewers {
            reviewers: vec![UserId::new("stranger")],
        },
    ))
    .unwrap_err();
    assert!(matches!(error, ForgeError::InvalidRequest(_)));
}

#[test]
fn pull_request_comments_use_issue_endpoints() {
    let client = MockHttpClient::new();
    client.push_response(200, "[]");
    client.push_response(
        201,
        r#"{"id": 5, "user": {"login": "bob"}, "body": "hi",
            "created_at": "2024-03-03T00:00:00Z", "updated_at": "2024-03-03T00:00:00Z"}"#,
    );
    let forge = forge(client.clone());

    let comments = block_on(forge.list_pull_request_comments(&pull_id(42))).unwrap();
    assert!(comments.is_empty());
    assert_eq!(
        client.recorded()[0].path,
        "/repos/acme/widgets/issues/42/comments"
    );

    let comment = block_on(forge.add_pull_request_comment(
        &pull_id(42),
        temper_forge_model::CreateComment {
            body: "hi".to_string(),
        },
    ))
    .unwrap();
    assert_eq!(comment.body, "hi");
}
