//! Offline contract tests for the (unsupported) dependency-link surface.
//!
//! GitHub exposes no native issue-dependency endpoint over its stable REST API,
//! so the backend reports no dependencies on reads and rejects mutations rather
//! than silently claiming success.

mod support;

use support::{MockHttpClient, block_on, forge, issue_id, pull_id};
use temper_forge::{ForgeError, IssueId, ItemNumber, PullRequestId};

#[test]
fn issue_dependency_mutations_are_rejected_without_http() {
    let client = MockHttpClient::new();
    let forge = forge(client.clone());

    let add = block_on(forge.add_issue_dependency(&issue_id(7), ItemNumber::new(9))).unwrap_err();
    assert!(matches!(add, ForgeError::InvalidRequest(_)));
    assert!(add.to_string().contains("does not support"));

    let remove =
        block_on(forge.remove_issue_dependency(&issue_id(7), ItemNumber::new(9))).unwrap_err();
    assert!(matches!(remove, ForgeError::InvalidRequest(_)));

    // No request ever leaves the backend for unsupported operations.
    assert_eq!(client.call_count(), 0);
}

#[test]
fn pull_request_dependency_mutations_are_rejected_without_http() {
    let client = MockHttpClient::new();
    let forge = forge(client.clone());

    let add =
        block_on(forge.add_pull_request_dependency(&pull_id(42), ItemNumber::new(9))).unwrap_err();
    assert!(matches!(add, ForgeError::InvalidRequest(_)));

    let remove = block_on(forge.remove_pull_request_dependency(&pull_id(42), ItemNumber::new(9)))
        .unwrap_err();
    assert!(matches!(remove, ForgeError::InvalidRequest(_)));

    assert_eq!(client.call_count(), 0);
}

#[test]
fn dependency_mutations_still_validate_id_shapes() {
    let client = MockHttpClient::new();
    let forge = forge(client);

    let foreign_issue = block_on(
        forge.add_issue_dependency(&IssueId::new("forgejo:a/b:issue:1"), ItemNumber::new(2)),
    )
    .unwrap_err();
    assert!(matches!(foreign_issue, ForgeError::InvalidRequest(_)));
    assert!(foreign_issue.to_string().contains("not a github"));

    let foreign_pull = block_on(
        forge.add_pull_request_dependency(&PullRequestId::new("not-an-id"), ItemNumber::new(2)),
    )
    .unwrap_err();
    assert!(matches!(foreign_pull, ForgeError::InvalidRequest(_)));
}

#[test]
fn reads_report_no_dependencies() {
    let client = MockHttpClient::new();
    client.push_response(
        200,
        r#"{
            "number": 7,
            "title": "t",
            "state": "open",
            "user": {"login": "author"},
            "created_at": "2024-03-01T00:00:00Z",
            "updated_at": "2024-03-02T00:00:00Z"
        }"#,
    );
    let forge = forge(client.clone());

    let issue = block_on(forge.get_issue(&issue_id(7))).unwrap().unwrap();
    assert!(issue.dependencies.is_empty());
    // Exactly one read: no extra dependency-enrichment request exists.
    assert_eq!(client.call_count(), 1);
}
