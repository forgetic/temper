//! Focused regressions for snapshot chaining and atomic issue creation.

mod support;

use support::{MockHttpClient, block_on, forge, issue_id, repo_id};
use temper_forge_forgejo::HttpMethod;
use temper_forge_model::{CreateIssue, ForgeError, ItemListDetails, UpdateIssue};

fn issue_json(number: u64, labels: &str) -> String {
    format!(
        r#"{{
            "number": {number},
            "title": "Issue {number}",
            "body": "body {number}",
            "state": "open",
            "user": {{"login": "author"}},
            "labels": {labels},
            "created_at": "2024-03-01T00:00:00Z",
            "updated_at": "2024-03-02T00:00:00Z"
        }}"#
    )
}

#[test]
fn create_issue_rejects_unknown_labels_before_posting() {
    let client = MockHttpClient::new();
    client.push_response(200, "[]");
    let forge = forge(client.clone());

    let result = block_on(forge.create_issue(
        &repo_id(),
        CreateIssue {
            title: "Unknown label".into(),
            body: String::new(),
            labels: vec!["missing".into()],
            assignees: Vec::new(),
        },
    ));

    assert!(matches!(result, Err(ForgeError::InvalidRequest(_))));
    assert_eq!(client.call_count(), 1);
    assert_eq!(client.recorded()[0].method, HttpMethod::Get);
}

#[test]
fn label_only_snapshot_result_is_valid_for_the_next_conditional_phase() {
    let client = MockHttpClient::new();
    client.push_response_with_etag(issue_json(7, "[]"), "etag-a");
    client.push_response_with_etag(issue_json(7, "[]"), "etag-a");
    client.push_response(200, r#"[{"id":1,"name":"ready"}]"#);
    client.push_response(200, "[]");
    client.push_response_with_etag(issue_json(7, r#"[{"id":1,"name":"ready"}]"#), "etag-b");
    client.push_response_with_etag(issue_json(7, r#"[{"id":1,"name":"ready"}]"#), "etag-c");
    let forge = forge(client.clone());
    let current = block_on(forge.get_issue_with_details(&issue_id(7), ItemListDetails::summary()))
        .unwrap()
        .unwrap();

    let labelled = block_on(forge.update_issue_from_snapshot(
        &current,
        UpdateIssue {
            add_labels: vec!["ready".into()],
            expected_version: Some(current.version),
            ..UpdateIssue::default()
        },
    ))
    .expect("label-only update commits");
    let committed = block_on(forge.update_issue_from_snapshot(
        &labelled,
        UpdateIssue {
            body: Some("next phase".into()),
            expected_version: Some(labelled.version),
            ..UpdateIssue::default()
        },
    ))
    .expect("returned label-only representation remains a valid snapshot");

    assert_eq!(committed.body, "body 7");
    assert_eq!(committed.labels, vec!["ready"]);
    assert_eq!(client.call_count(), 6);
}
