//! Offline contract tests for Forgejo reviewer requests, native review events,
//! review submission, merge, and best-effort conditional updates. Every request
//! is served by a recording mock client; no test touches the network.

mod support;

use support::{MockHttpClient, OWNER, REPO, block_on, body_json, forge, forge_with, pull_id};
use temper_forge_forgejo::{CasMode, HttpMethod};
use temper_forge_model::{
    CreatePullRequestReview, ForgeError, MergeMethod, MergePullRequest, PullRequestReviewStatus,
    RequestReviewers, ReviewDecision, UpdatePullRequest, UserId,
};

/// Renders a pull-request DTO JSON body with overridable fields.
fn pr_json(number: u64, state: &str, extra: &str) -> String {
    format!(
        r#"{{
            "number": {number},
            "title": "PR {number}",
            "state": "{state}",
            "merged": false,
            "user": {{"login": "author"}},
            "head": {{"ref": "feature-{number}", "sha": "head{number}"}},
            "base": {{"ref": "main", "sha": "base{number}"}},
            "created_at": "2024-03-01T00:00:00Z",
            "updated_at": "2024-03-02T00:00:00Z"
            {extra}
        }}"#
    )
}

#[test]
fn request_reviewers_posts_logins_then_refetches() {
    let client = MockHttpClient::new();
    client.push_response(201, "{}"); // POST requested_reviewers
    client.push_response(
        200,
        pr_json(
            42,
            "open",
            r#", "requested_reviewers": [{"login": "carol"}, {"login": "dave"}]"#,
        ),
    ); // GET refetch
    let forge = forge(client.clone());

    let pull = block_on(forge.request_pull_request_reviewers(
        &pull_id(42),
        RequestReviewers {
            reviewers: vec![UserId::new("carol"), UserId::new("dave")],
        },
    ))
    .unwrap();
    assert_eq!(
        pull.requested_reviewers,
        vec![UserId::new("carol"), UserId::new("dave")]
    );

    let requests = client.recorded();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].method, HttpMethod::Post);
    assert_eq!(
        requests[0].path,
        format!("/api/v1/repos/{OWNER}/{REPO}/pulls/42/requested_reviewers")
    );
    let body = body_json(&requests[0]);
    assert_eq!(body["reviewers"], serde_json::json!(["carol", "dave"]));
    assert_eq!(body["team_reviewers"], serde_json::json!([]));
    assert_eq!(requests[1].method, HttpMethod::Get);
}

#[test]
fn request_reviewers_is_idempotent_when_already_present() {
    let client = MockHttpClient::new();
    // Forgejo rejects a duplicate reviewer request; the backend treats it as a
    // no-op when the desired reviewer is already present.
    client.push_response(422, r#"{"message":"user is already requested"}"#);
    client.push_response(
        200,
        pr_json(
            42,
            "open",
            r#", "requested_reviewers": [{"login": "carol"}]"#,
        ),
    );
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
fn request_reviewers_missing_pull_request_is_not_found() {
    let client = MockHttpClient::new();
    client.push_response(404, r#"{"message":"not found"}"#);
    let forge = forge(client.clone());

    let result = block_on(forge.request_pull_request_reviewers(
        &pull_id(7),
        RequestReviewers {
            reviewers: vec![UserId::new("carol")],
        },
    ));
    assert!(matches!(result, Err(ForgeError::NotFound(_))));
    assert_eq!(client.call_count(), 1);
}

#[test]
fn list_reviews_keeps_dismissed_filters_requests_and_sorts() {
    let client = MockHttpClient::new();
    client.push_response(
        200,
        r#"[
            {"id": 4, "user": {"login": "carol"}, "state": "APPROVED", "submitted_at": "2024-03-05T00:00:00Z"},
            {"id": 3, "user": {"login": "carol"}, "state": "COMMENT", "submitted_at": "2024-03-03T00:00:00Z"},
            {"id": 2, "user": {"login": "dave"}, "state": "APPROVED", "submitted_at": "2024-03-04T00:00:00Z", "dismissed": true},
            {"id": 1, "user": {"login": "erin"}, "state": "REQUEST_REVIEW", "created_at": "2024-03-02T00:00:00Z"}
        ]"#,
    );
    let forge = forge(client.clone());

    let reviews = block_on(forge.list_pull_request_reviews(&pull_id(42))).unwrap();
    // The review-request event (#1) is excluded, but the dismissed verdict (#2)
    // is **kept** (Forgejo auto-dismisses prior reviews; history must survive).
    // The rest sort by submission time.
    assert_eq!(reviews.len(), 3);
    assert_eq!(reviews[0].decision, ReviewDecision::Commented);
    assert_eq!(reviews[1].decision, ReviewDecision::Approved);
    assert_eq!(reviews[1].reviewer_id, UserId::new("dave"));
    assert_eq!(reviews[2].decision, ReviewDecision::Approved);
    assert_eq!(reviews[2].reviewer_id, UserId::new("carol"));
    assert_eq!(
        client.last_request().unwrap().path,
        format!("/api/v1/repos/{OWNER}/{REPO}/pulls/42/reviews")
    );
}

#[test]
fn submit_review_uses_one_call_event_payload() {
    for (decision, event) in [
        (ReviewDecision::Approved, "APPROVED"),
        (ReviewDecision::ChangesRequested, "REQUEST_CHANGES"),
        (ReviewDecision::Commented, "COMMENT"),
    ] {
        let client = MockHttpClient::new();
        client.push_response(
            200,
            format!(
                r#"{{"id": 11, "user": {{"login": "carol"}}, "state": "{event}", "body": "verdict", "submitted_at": "2024-03-06T00:00:00Z"}}"#
            ),
        );
        let forge = forge(client.clone());

        let review = block_on(forge.submit_pull_request_review(
            &pull_id(42),
            CreatePullRequestReview {
                decision,
                body: Some("verdict".to_string()),
            },
        ))
        .unwrap();
        assert_eq!(review.decision, decision);

        let request = client.last_request().unwrap();
        assert_eq!(request.method, HttpMethod::Post);
        assert_eq!(
            request.path,
            format!("/api/v1/repos/{OWNER}/{REPO}/pulls/42/reviews")
        );
        let body = body_json(&request);
        assert_eq!(body["event"], event);
        assert_eq!(body["body"], "verdict");
    }
}

#[test]
fn submit_pending_review_is_rejected_without_request() {
    let client = MockHttpClient::new();
    let forge = forge(client.clone());
    let result = block_on(forge.submit_pull_request_review(
        &pull_id(42),
        CreatePullRequestReview {
            decision: ReviewDecision::Pending,
            body: Some("later".to_string()),
        },
    ));
    assert!(matches!(result, Err(ForgeError::InvalidRequest(_))));
    // Pending is rejected before any HTTP call is made.
    assert_eq!(client.call_count(), 0);
}

#[test]
fn submit_review_falls_back_to_requested_decision_when_echo_is_sparse() {
    let client = MockHttpClient::new();
    // The provider echoes a review object with an unmapped state; the backend
    // falls back to the decision it submitted.
    client.push_response(
        200,
        r#"{"id": 12, "user": {"login": "carol"}, "state": "", "submitted_at": "2024-03-06T00:00:00Z"}"#,
    );
    let forge = forge(client);

    let review = block_on(forge.submit_pull_request_review(
        &pull_id(42),
        CreatePullRequestReview {
            decision: ReviewDecision::Approved,
            body: Some("ok".to_string()),
        },
    ))
    .unwrap();
    assert_eq!(review.decision, ReviewDecision::Approved);
    assert_eq!(review.reviewer_id, UserId::new("carol"));
}

#[test]
fn portable_review_aggregate_uses_mapped_reviews() {
    // Every reviewer approves: the portable aggregate is approved.
    let client = MockHttpClient::new();
    client.push_response(
        200,
        r#"[
            {"id": 1, "user": {"login": "carol"}, "state": "APPROVED", "submitted_at": "2024-03-05T00:00:00Z"},
            {"id": 2, "user": {"login": "dave"}, "state": "APPROVED", "submitted_at": "2024-03-06T00:00:00Z"}
        ]"#,
    );
    let backend = forge(client);
    let reviews = block_on(backend.list_pull_request_reviews(&pull_id(42))).unwrap();
    let approved = PullRequestReviewStatus::from_reviews(
        &[UserId::new("carol"), UserId::new("dave")],
        &reviews,
    );
    assert!(approved.is_approved());

    // A later changes-requested review blocks approval.
    let client = MockHttpClient::new();
    client.push_response(
        200,
        r#"[
            {"id": 1, "user": {"login": "carol"}, "state": "APPROVED", "submitted_at": "2024-03-05T00:00:00Z"},
            {"id": 2, "user": {"login": "dave"}, "state": "REQUEST_CHANGES", "submitted_at": "2024-03-06T00:00:00Z"}
        ]"#,
    );
    let backend = forge(client);
    let reviews = block_on(backend.list_pull_request_reviews(&pull_id(42))).unwrap();
    let blocked = PullRequestReviewStatus::from_reviews(
        &[UserId::new("carol"), UserId::new("dave")],
        &reviews,
    );
    assert!(!blocked.is_approved());
    assert!(blocked.has_changes_requested());
}

#[test]
fn merge_posts_method_payload_and_maps_record() {
    let client = MockHttpClient::new();
    client.push_response(200, ""); // POST merge (no body)
    client.push_response(
        200,
        r#"{
            "number": 42,
            "title": "PR 42",
            "state": "closed",
            "merged": true,
            "merged_at": "2024-04-01T00:00:00Z",
            "merge_commit_sha": "mergesha",
            "merged_by": {"login": "maintainer"},
            "user": {"login": "author"},
            "head": {"ref": "feature-42", "sha": "head42"},
            "base": {"ref": "main", "sha": "base42"},
            "created_at": "2024-03-01T00:00:00Z",
            "updated_at": "2024-04-01T00:00:00Z",
            "closed_at": "2024-04-01T00:00:00Z"
        }"#,
    ); // GET refetch
    let forge = forge(client.clone());

    let record = block_on(forge.merge_pull_request(
        &pull_id(42),
        MergePullRequest {
            method: MergeMethod::Squash,
            commit_title: Some("Squash title".to_string()),
            commit_body: Some("Squash message".to_string()),
        },
    ))
    .unwrap();
    // The merge method is reported from the request, not the (silent) provider.
    assert_eq!(record.method, MergeMethod::Squash);
    assert_eq!(record.commit_sha, "mergesha");
    assert_eq!(record.merged_by, UserId::new("maintainer"));

    let requests = client.recorded();
    assert_eq!(requests[0].method, HttpMethod::Post);
    assert_eq!(
        requests[0].path,
        format!("/api/v1/repos/{OWNER}/{REPO}/pulls/42/merge")
    );
    let body = body_json(&requests[0]);
    assert_eq!(body["Do"], "squash");
    assert_eq!(body["MergeTitleField"], "Squash title");
    assert_eq!(body["MergeMessageField"], "Squash message");
    assert_eq!(requests[1].method, HttpMethod::Get);
}

#[test]
fn merge_conflict_status_maps_to_conflict() {
    for status in [405u16, 409, 422] {
        let client = MockHttpClient::new();
        client.push_response(status, r#"{"message":"Pull request is not mergeable"}"#);
        let forge = forge(client.clone());
        let result = block_on(forge.merge_pull_request(
            &pull_id(42),
            MergePullRequest {
                method: MergeMethod::MergeCommit,
                commit_title: None,
                commit_body: None,
            },
        ));
        assert!(
            matches!(result, Err(ForgeError::Conflict(_))),
            "status {status} should map to Conflict"
        );
        // A conflict means no re-fetch is attempted.
        assert_eq!(client.call_count(), 1);
    }
}

#[test]
fn conditional_update_conflicts_when_validator_changes() {
    let client = MockHttpClient::new();
    client.push_response_with_etag(pr_json(42, "open", ""), "etag-v1"); // initial read
    client.push_response(200, "[]"); // initial read's dependency enrichment
    client.push_response_with_etag(pr_json(42, "open", ""), "etag-v2"); // update's read
    let forge = forge(client.clone());

    // Capture the version under validator "etag-v1".
    let pull = block_on(forge.get_pull_request(&pull_id(42)))
        .unwrap()
        .expect("pull request present");

    // The artifact has since changed (validator "etag-v2"); the conditional
    // update must report a conflict and mutate nothing.
    let result = block_on(forge.update_pull_request(
        &pull_id(42),
        UpdatePullRequest {
            title: Some("Renamed".to_string()),
            expected_version: Some(pull.version),
            ..UpdatePullRequest::default()
        },
    ));
    assert!(matches!(result, Err(ForgeError::Conflict(_))));
    // Read + dependency enrichment for the get, then the update's read; no PATCH.
    assert_eq!(client.call_count(), 3);
}

#[test]
fn conditional_update_succeeds_when_validator_matches() {
    let client = MockHttpClient::new();
    client.push_response_with_etag(pr_json(42, "open", ""), "etag-v1"); // initial read
    client.push_response(200, "[]"); // initial read's dependency enrichment
    client.push_response_with_etag(pr_json(42, "open", ""), "etag-v1"); // update's read
    client.push_response(200, "{}"); // PATCH pull
    client.push_response_with_etag(pr_json(42, "open", ""), "etag-v1"); // refetch
    let forge = forge_with(client.clone(), CasMode::Strict);

    let pull = block_on(forge.get_pull_request(&pull_id(42)))
        .unwrap()
        .expect("pull request present");

    let updated = block_on(forge.update_pull_request(
        &pull_id(42),
        UpdatePullRequest {
            title: Some("Renamed".to_string()),
            expected_version: Some(pull.version),
            ..UpdatePullRequest::default()
        },
    ))
    .unwrap();
    assert_eq!(updated.number.get(), 42);
    // get read + dependency enrichment, then the update's read, patch, refetch.
    // The mutation refetch does not re-read dependencies.
    assert_eq!(client.call_count(), 5);
}
