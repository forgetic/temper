//! Offline request-shape tests for Forgejo consolidated issue candidates.

mod support;

use support::{MockHttpClient, block_on, forge, repo_id};
use temper_forge_forgejo::{ForgejoConfig, ForgejoForge};
use temper_forge_model::{CandidateLabelSelection, CandidateLifecycle, IssueCandidateQuery};

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
fn terminal_issue_candidates_use_one_any_label_request_and_exclude_pr_rows() {
    let client = MockHttpClient::new();
    client.push_response(
        200,
        format!(
            "[{},{},{},{}]",
            issue_json(2, "closed", r#"[{"id":2,"name":"queued"}]"#, ""),
            issue_json(1, "closed", r#"[{"id":1,"name":"ready"}]"#, ""),
            issue_json(3, "closed", r#"[{"id":3,"name":"other"}]"#, ""),
            issue_json(
                4,
                "closed",
                r#"[{"id":1,"name":"ready"}]"#,
                r#", "pull_request": {"merged": false}"#
            )
        ),
    );
    let forge = forge(client.clone());

    let issues = block_on(forge.list_issue_candidates(
        &repo_id(),
        IssueCandidateQuery {
            lifecycle: CandidateLifecycle::Terminal,
            labels: CandidateLabelSelection::AnyOf(vec![
                "ready".into(),
                "queued".into(),
                "ready".into(),
            ]),
            ..IssueCandidateQuery::default()
        },
    ))
    .unwrap();

    assert_eq!(
        issues
            .iter()
            .map(|issue| issue.number.get())
            .collect::<Vec<_>>(),
        vec![1, 2]
    );
    let requests = client.recorded();
    assert_eq!(requests.len(), 1, "label count must not add list requests");
    assert!(
        requests[0]
            .query
            .contains(&("state".into(), "closed".into()))
    );
    assert!(
        requests[0]
            .query
            .contains(&("type".into(), "issues".into()))
    );
    assert!(
        requests[0]
            .query
            .contains(&("labels".into(), "queued,ready".into()))
    );
}

#[test]
fn paginated_issue_candidates_deduplicate_and_keep_any_label_semantics() {
    let client = MockHttpClient::new();
    client.push_response(
        200,
        format!(
            "[{},{}]",
            issue_json(3, "open", r#"[{"id":1,"name":"ready"}]"#, ""),
            issue_json(1, "open", r#"[{"id":2,"name":"queued"}]"#, "")
        ),
    );
    client.push_response(
        200,
        format!(
            "[{},{}]",
            issue_json(1, "open", r#"[{"id":2,"name":"queued"}]"#, ""),
            issue_json(2, "open", r#"[{"id":1,"name":"ready"}]"#, "")
        ),
    );
    client.push_response(200, "[]");
    let config = ForgejoConfig::new("https://forge.example.com", "test-token").with_page_limit(2);
    let forge = ForgejoForge::with_client(config, client.clone());

    let issues = block_on(forge.list_issue_candidates(
        &repo_id(),
        IssueCandidateQuery {
            labels: CandidateLabelSelection::AnyOf(vec!["ready".into(), "queued".into()]),
            ..IssueCandidateQuery::default()
        },
    ))
    .unwrap();

    assert_eq!(
        issues
            .iter()
            .map(|issue| issue.number.get())
            .collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
    let requests = client.recorded();
    assert_eq!(requests.len(), 3);
    assert!(requests.iter().all(|request| {
        request
            .query
            .contains(&("labels".into(), "queued,ready".into()))
    }));
}
