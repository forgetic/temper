//! Offline contract tests for native reviews and merging.

mod support;

use support::{MockHttpClient, block_on, body_json, forge, pull_id};
use temper_forge_github::HttpMethod;
use temper_forge_model::{
    CreatePullRequestReview, ForgeError, MergeMethod, MergePullRequest, ReviewDecision, UserId,
};

fn merged_pull_json(number: u64) -> String {
    format!(
        r#"{{
            "number": {number},
            "title": "a pull",
            "state": "closed",
            "merged": true,
            "merged_at": "2024-04-01T00:00:00Z",
            "merge_commit_sha": "mergesha",
            "merged_by": {{"login": "maintainer"}},
            "user": {{"login": "author"}},
            "created_at": "2024-03-01T00:00:00Z",
            "updated_at": "2024-04-01T00:00:00Z",
            "closed_at": "2024-04-01T00:00:00Z"
        }}"#
    )
}

#[test]
fn list_reviews_sorts_chronologically_and_drops_dismissed() {
    let client = MockHttpClient::new();
    client.push_response(
        200,
        r#"[
            {"id": 3, "user": {"login": "carol"}, "state": "APPROVED",
             "submitted_at": "2024-03-05T00:00:00Z"},
            {"id": 1, "user": {"login": "carol"}, "state": "DISMISSED",
             "submitted_at": "2024-03-03T00:00:00Z"},
            {"id": 2, "user": {"login": "dave"}, "state": "CHANGES_REQUESTED",
             "body": "please fix", "submitted_at": "2024-03-04T00:00:00Z"}
        ]"#,
    );
    let forge = forge(client.clone());

    let reviews = block_on(forge.list_pull_request_reviews(&pull_id(42))).unwrap();
    assert_eq!(reviews.len(), 2);
    assert_eq!(reviews[0].decision, ReviewDecision::ChangesRequested);
    assert_eq!(reviews[0].reviewer_id, UserId::new("dave"));
    assert_eq!(reviews[1].decision, ReviewDecision::Approved);
    assert_eq!(reviews[1].id.as_str(), "github:acme/widgets:review:3");

    assert_eq!(
        client.recorded()[0].path,
        "/repos/acme/widgets/pulls/42/reviews"
    );
}

#[test]
fn submit_review_posts_github_event_token() {
    let client = MockHttpClient::new();
    client.push_response(
        200,
        r#"{"id": 9, "user": {"login": "carol"}, "body": "ship it",
            "state": "APPROVED", "submitted_at": "2024-03-05T00:00:00Z"}"#,
    );
    let forge = forge(client.clone());

    let review = block_on(forge.submit_pull_request_review(
        &pull_id(42),
        CreatePullRequestReview {
            decision: ReviewDecision::Approved,
            body: Some("ship it".to_string()),
        },
    ))
    .unwrap();
    assert_eq!(review.decision, ReviewDecision::Approved);
    assert_eq!(review.body.as_deref(), Some("ship it"));

    let request = client.last_request().unwrap();
    assert_eq!(request.method, HttpMethod::Post);
    assert_eq!(request.path, "/repos/acme/widgets/pulls/42/reviews");
    let payload = body_json(&request);
    assert_eq!(payload["event"], "APPROVE");
    assert_eq!(payload["body"], "ship it");
}

#[test]
fn submit_review_rejects_pending_without_a_request() {
    let client = MockHttpClient::new();
    let forge = forge(client.clone());

    let error = block_on(forge.submit_pull_request_review(
        &pull_id(42),
        CreatePullRequestReview {
            decision: ReviewDecision::Pending,
            body: None,
        },
    ))
    .unwrap_err();
    assert!(matches!(error, ForgeError::InvalidRequest(_)));
    assert_eq!(client.call_count(), 0);
}

#[test]
fn merge_puts_merge_method_and_rereads_for_the_record() {
    let client = MockHttpClient::new();
    client.push_response(
        200,
        r#"{"sha": "mergesha", "merged": true, "message": "Pull Request successfully merged"}"#,
    );
    client.push_response(200, merged_pull_json(42)); // re-read
    let forge = forge(client.clone());

    let record = block_on(forge.merge_pull_request(
        &pull_id(42),
        MergePullRequest {
            method: MergeMethod::Squash,
            commit_title: Some("Squash title".to_string()),
            commit_body: Some("Squash body".to_string()),
            delete_source_branch: false,
        },
    ))
    .unwrap();
    // The record reports the method actually requested, not the default.
    assert_eq!(record.method, MergeMethod::Squash);
    assert_eq!(record.commit_sha, "mergesha");
    assert_eq!(record.merged_by, UserId::new("maintainer"));

    let recorded = client.recorded();
    assert_eq!(recorded[0].method, HttpMethod::Put);
    assert_eq!(recorded[0].path, "/repos/acme/widgets/pulls/42/merge");
    let payload = body_json(&recorded[0]);
    assert_eq!(payload["merge_method"], "squash");
    assert_eq!(payload["commit_title"], "Squash title");
    assert_eq!(payload["commit_message"], "Squash body");
}

#[test]
fn merge_maps_not_mergeable_statuses_to_conflict() {
    for status in [405u16, 409] {
        let client = MockHttpClient::new();
        client.push_response(status, r#"{"message": "Pull Request is not mergeable"}"#);
        let forge = forge(client);

        let error = block_on(forge.merge_pull_request(
            &pull_id(42),
            MergePullRequest {
                method: MergeMethod::MergeCommit,
                commit_title: None,
                commit_body: None,
                delete_source_branch: false,
            },
        ))
        .unwrap_err();
        assert!(
            matches!(error, ForgeError::Conflict(_)),
            "status {status} should map to Conflict, got {error:?}"
        );
    }
}

#[test]
fn merge_maps_missing_pull_request_to_not_found() {
    let client = MockHttpClient::new();
    client.push_response(404, r#"{"message": "Not Found"}"#);
    let forge = forge(client);

    let error = block_on(forge.merge_pull_request(
        &pull_id(99),
        MergePullRequest {
            method: MergeMethod::MergeCommit,
            commit_title: None,
            commit_body: None,
            delete_source_branch: false,
        },
    ))
    .unwrap_err();
    assert!(matches!(error, ForgeError::NotFound(_)));
}
