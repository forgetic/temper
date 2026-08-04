//! Offline request-shape tests for Forgejo consolidated issue candidates.

mod support;

use support::{MockHttpClient, OWNER, REPO, block_on, forge, repo_id};
use temper_forge_forgejo::{ForgejoConfig, ForgejoForge, MAX_CANDIDATE_PROVIDER_REQUESTS};
use temper_forge_model::{
    CandidateLabelSelection, CandidateLifecycle, CandidatePageRequest, IssueCandidateQuery,
    RepositoryId,
};

fn issue_json(number: u64, state: &str, labels: &str, extra: &str) -> String {
    issue_json_at(number, state, labels, extra, "2024-03-02T00:00:00Z")
}

fn issue_json_at(number: u64, state: &str, labels: &str, extra: &str, updated_at: &str) -> String {
    format!(
        r#"{{
            "number": {number},
            "title": "Issue {number}",
            "body": "body {number}",
            "state": "{state}",
            "user": {{"login": "author"}},
            "labels": {labels},
            "created_at": "2024-03-01T00:00:00Z",
            "updated_at": "{updated_at}",
            "repository": {{"name": "{REPO}", "full_name": "{OWNER}/{REPO}"}}
            {extra}
        }}"#
    )
}

#[test]
fn terminal_issue_candidates_are_repository_isolated_and_any_label() {
    let client = MockHttpClient::new();
    client.push_response(
        200,
        format!(
            "[{},{}]",
            issue_json(2, "closed", r#"[{"id":2,"name":"queued"}]"#, ""),
            issue_json(2, "closed", r#"[{"id":2,"name":"queued"}]"#, "")
        ),
    );
    client.push_response(
        200,
        format!(
            "[{},{},{}]",
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
    assert!(issues.exhausted);
    assert_eq!(issues.raw_count, 5);
    let requests = client.recorded();
    assert_eq!(requests.len(), 2, "one bounded stream per normalized label");
    assert!(requests.iter().all(|request| {
        request.path == format!("/api/v1/repos/{OWNER}/{REPO}/issues")
            && request.query.contains(&("state".into(), "closed".into()))
            && request.query.contains(&("type".into(), "issues".into()))
            && request.query.contains(&("sort".into(), "updated".into()))
            && request.query.contains(&("direction".into(), "asc".into()))
            && request.query.iter().any(|(key, _)| key == "before")
    }));
    let labels = requests
        .iter()
        .map(|request| {
            request
                .query
                .iter()
                .find(|(key, _)| key == "labels")
                .expect("single-label stream")
                .1
                .clone()
        })
        .collect::<Vec<_>>();
    assert_eq!(labels, vec!["queued", "ready"]);
    assert!(
        requests
            .iter()
            .all(|request| request.path != "/api/v1/repos/issues/search"),
        "owner-scoped sibling history must not consume the bounded page"
    );
}

#[test]
fn unbounded_open_candidates_deduplicate_across_repository_label_streams() {
    let client = MockHttpClient::new();
    // Normalization visits queued, then ready. Full pages are followed by one
    // partial page for each exhaustive open stream.
    client.push_response(
        200,
        format!(
            "[{},{}]",
            issue_json(1, "open", r#"[{"id":2,"name":"queued"}]"#, ""),
            issue_json(1, "open", r#"[{"id":2,"name":"queued"}]"#, "")
        ),
    );
    client.push_response(200, "[]");
    client.push_response(
        200,
        format!(
            "[{},{}]",
            issue_json(3, "open", r#"[{"id":1,"name":"ready"}]"#, ""),
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
    assert_eq!(client.call_count(), 4);
}

#[test]
fn bounded_continuation_moves_provider_pages_across_equal_timestamps() {
    let client = MockHttpClient::new();
    let tied = "2024-03-02T00:00:00Z";
    client.push_response(
        200,
        format!(
            "[{},{}]",
            issue_json_at(1, "closed", r#"[{"id":1,"name":"ready"}]"#, "", tied),
            issue_json_at(2, "closed", r#"[{"id":1,"name":"ready"}]"#, "", tied)
        ),
    );
    client.push_response(
        200,
        format!(
            "[{},{}]",
            issue_json_at(3, "closed", r#"[{"id":1,"name":"ready"}]"#, "", tied),
            issue_json_at(4, "closed", r#"[{"id":1,"name":"ready"}]"#, "", tied)
        ),
    );
    let config = ForgejoConfig::new("https://forge.example.com", "test-token").with_page_limit(2);
    let forge = ForgejoForge::with_client(config, client.clone());
    let query = |continuation| IssueCandidateQuery {
        lifecycle: CandidateLifecycle::Terminal,
        labels: CandidateLabelSelection::AnyOf(vec!["ready".into()]),
        page: Some(CandidatePageRequest {
            limit: 2,
            continuation,
        }),
        ..IssueCandidateQuery::default()
    };

    let first = block_on(forge.list_issue_candidates(&repo_id(), query(None))).unwrap();
    assert_eq!(
        first
            .iter()
            .map(|issue| issue.number.get())
            .collect::<Vec<_>>(),
        vec![1, 2]
    );
    assert!(first.overflow);
    let boundary = first
        .continuation
        .as_ref()
        .expect("overflow continuation")
        .boundary
        .updated_at;

    client.push_response(
        200,
        format!(
            "[{},{}]",
            issue_json_at(3, "closed", r#"[{"id":1,"name":"ready"}]"#, "", tied),
            issue_json_at(4, "closed", r#"[{"id":1,"name":"ready"}]"#, "", tied)
        ),
    );
    // A concurrent row beyond the frozen boundary is returned by the mock but
    // must not enter this sweep.
    let newer = (boundary + chrono::Duration::seconds(1)).to_rfc3339();
    client.push_response(
        200,
        format!(
            "[{}]",
            issue_json_at(5, "closed", r#"[{"id":1,"name":"ready"}]"#, "", &newer)
        ),
    );
    let second =
        block_on(forge.list_issue_candidates(&repo_id(), query(first.continuation))).unwrap();
    assert_eq!(
        second
            .iter()
            .map(|issue| issue.number.get())
            .collect::<Vec<_>>(),
        vec![3, 4]
    );
    assert!(second.overflow);
    let third =
        block_on(forge.list_issue_candidates(&repo_id(), query(second.continuation))).unwrap();
    assert!(third.is_empty());
    assert!(third.exhausted);

    let requests = client.recorded();
    assert_eq!(requests.len(), 4);
    assert_eq!(
        requests[1]
            .query
            .iter()
            .find(|(key, _)| key == "page")
            .map(|(_, value)| value.as_str()),
        Some("1"),
        "the first timestamp boundary resets provider page numbering"
    );
    assert_eq!(
        requests[2]
            .query
            .iter()
            .find(|(key, _)| key == "page")
            .map(|(_, value)| value.as_str()),
        Some("2"),
        "backend cursor must then move through the equal-timestamp tie"
    );
    assert!(requests[1].query.iter().any(|(key, _)| key == "since"));
    assert_eq!(
        requests[1]
            .query
            .iter()
            .find(|(key, _)| key == "before")
            .map(|(_, value)| value.clone()),
        requests[0]
            .query
            .iter()
            .find(|(key, _)| key == "before")
            .map(|(_, value)| value.clone()),
        "every generation must retain the frozen timestamp boundary"
    );
}

#[test]
fn timestamp_progress_resets_provider_pages_instead_of_skipping_rows() {
    let client = MockHttpClient::new();
    client.push_response(
        200,
        format!(
            "[{},{}]",
            issue_json_at(
                1,
                "closed",
                r#"[{"id":1,"name":"ready"}]"#,
                "",
                "2024-03-01T00:00:00Z"
            ),
            issue_json_at(
                2,
                "closed",
                r#"[{"id":1,"name":"ready"}]"#,
                "",
                "2024-03-02T00:00:00Z"
            )
        ),
    );
    // `since` is inclusive. Once the portable cursor moves to issue #2's
    // timestamp, this is provider page one of a different filtered result set.
    client.push_response(
        200,
        format!(
            "[{},{}]",
            issue_json_at(
                2,
                "closed",
                r#"[{"id":1,"name":"ready"}]"#,
                "",
                "2024-03-02T00:00:00Z"
            ),
            issue_json_at(
                3,
                "closed",
                r#"[{"id":1,"name":"ready"}]"#,
                "",
                "2024-03-03T00:00:00Z"
            )
        ),
    );
    client.push_response(200, "[]");
    let config = ForgejoConfig::new("https://forge.example.com", "test-token").with_page_limit(2);
    let forge = ForgejoForge::with_client(config, client.clone());
    let query = |continuation| IssueCandidateQuery {
        lifecycle: CandidateLifecycle::Terminal,
        labels: CandidateLabelSelection::AnyOf(vec!["ready".into()]),
        page: Some(CandidatePageRequest {
            limit: 2,
            continuation,
        }),
        ..IssueCandidateQuery::default()
    };

    let first = block_on(forge.list_issue_candidates(&repo_id(), query(None))).unwrap();
    assert_eq!(
        first
            .iter()
            .map(|issue| issue.number.get())
            .collect::<Vec<_>>(),
        vec![1, 2]
    );
    assert!(first.overflow);

    let second =
        block_on(forge.list_issue_candidates(&repo_id(), query(first.continuation))).unwrap();
    assert_eq!(
        second
            .iter()
            .map(|issue| issue.number.get())
            .collect::<Vec<_>>(),
        vec![3],
        "issue #3 must not be skipped when the inclusive `since` result set changes"
    );
    let requests = client.recorded();
    assert_eq!(requests.len(), 3);
    assert_eq!(
        requests[1]
            .query
            .iter()
            .find(|(key, _)| key == "page")
            .map(|(_, value)| value.as_str()),
        Some("1")
    );
    assert_eq!(
        requests[1]
            .query
            .iter()
            .find(|(key, _)| key == "since")
            .map(|(_, value)| value.as_str()),
        Some("2024-03-02T00:00:00+00:00")
    );
}

#[test]
fn skewed_label_streams_are_merged_before_the_cursor_advances() {
    let client = MockHttpClient::new();
    client.push_response(
        200,
        format!(
            "[{}]",
            issue_json_at(
                100,
                "closed",
                r#"[{"id":2,"name":"queued"}]"#,
                "",
                "2024-03-02T00:00:00Z"
            )
        ),
    );
    client.push_response(
        200,
        format!(
            "[{}]",
            issue_json_at(
                1,
                "closed",
                r#"[{"id":1,"name":"ready"}]"#,
                "",
                "2024-03-01T00:00:00Z"
            )
        ),
    );
    // The ready stream's next page is still older than queued #100. It must be
    // fetched before the portable cursor can advance past #2.
    client.push_response(
        200,
        format!(
            "[{}]",
            issue_json_at(
                2,
                "closed",
                r#"[{"id":1,"name":"ready"}]"#,
                "",
                "2024-03-01T00:00:01Z"
            )
        ),
    );
    let config = ForgejoConfig::new("https://forge.example.com", "test-token").with_page_limit(1);
    let forge = ForgejoForge::with_client(config, client.clone());
    let page = block_on(forge.list_issue_candidates(
        &repo_id(),
        IssueCandidateQuery {
            lifecycle: CandidateLifecycle::Terminal,
            labels: CandidateLabelSelection::AnyOf(vec!["ready".into(), "queued".into()]),
            page: Some(CandidatePageRequest::first(2)),
            ..IssueCandidateQuery::default()
        },
    ))
    .unwrap();

    assert_eq!(
        page.iter()
            .map(|issue| issue.number.get())
            .collect::<Vec<_>>(),
        vec![1, 2]
    );
    assert!(page.overflow);
    let requests = client.recorded();
    assert_eq!(requests.len(), 3);
    assert_eq!(
        requests
            .iter()
            .map(|request| {
                let label = request
                    .query
                    .iter()
                    .find(|(key, _)| key == "labels")
                    .unwrap()
                    .1
                    .as_str();
                let provider_page = request
                    .query
                    .iter()
                    .find(|(key, _)| key == "page")
                    .unwrap()
                    .1
                    .as_str();
                (label, provider_page)
            })
            .collect::<Vec<_>>(),
        vec![("queued", "1"), ("ready", "1"), ("ready", "2")]
    );
}

#[test]
fn mismatched_continuation_is_rejected_before_a_provider_request() {
    let client = MockHttpClient::new();
    client.push_response(
        200,
        format!(
            "[{}]",
            issue_json(1, "closed", r#"[{"id":1,"name":"ready"}]"#, "")
        ),
    );
    let config = ForgejoConfig::new("https://forge.example.com", "test-token").with_page_limit(1);
    let forge = ForgejoForge::with_client(config, client.clone());
    let query = |continuation| IssueCandidateQuery {
        lifecycle: CandidateLifecycle::Terminal,
        labels: CandidateLabelSelection::AnyOf(vec!["ready".into()]),
        page: Some(CandidatePageRequest {
            limit: 1,
            continuation,
        }),
        ..IssueCandidateQuery::default()
    };

    let first = block_on(forge.list_issue_candidates(&repo_id(), query(None))).unwrap();
    let mut continuation = first.continuation.expect("full stream must continue");
    continuation.repository_id = RepositoryId::new("forgejo:acme/other");
    let error =
        block_on(forge.list_issue_candidates(&repo_id(), query(Some(continuation)))).unwrap_err();

    assert!(matches!(
        error,
        temper_forge_model::ForgeError::InvalidRequest(_)
    ));
    assert_eq!(
        client.call_count(),
        1,
        "scope mismatch must fail before crossing the HTTP seam"
    );
}

#[test]
fn failed_multi_stream_page_returns_no_partial_continuation() {
    let client = MockHttpClient::new();
    client.push_response(
        200,
        format!(
            "[{}]",
            issue_json(1, "closed", r#"[{"id":2,"name":"queued"}]"#, "")
        ),
    );
    client.push_transport_error("ready stream unavailable");
    let forge = forge(client.clone());
    let query = IssueCandidateQuery {
        lifecycle: CandidateLifecycle::Terminal,
        labels: CandidateLabelSelection::AnyOf(vec!["ready".into(), "queued".into()]),
        page: Some(CandidatePageRequest::first(2)),
        ..IssueCandidateQuery::default()
    };
    let error = block_on(forge.list_issue_candidates(&repo_id(), query.clone())).unwrap_err();
    assert!(matches!(error, temper_forge_model::ForgeError::Backend(_)));
    assert_eq!(
        client.call_count(),
        2,
        "candidate pages do not hide retries"
    );

    client.push_response(
        200,
        format!(
            "[{}]",
            issue_json(1, "closed", r#"[{"id":2,"name":"queued"}]"#, "")
        ),
    );
    client.push_response(
        200,
        format!(
            "[{}]",
            issue_json(2, "closed", r#"[{"id":1,"name":"ready"}]"#, "")
        ),
    );
    let retried = block_on(forge.list_issue_candidates(&repo_id(), query)).unwrap();
    assert_eq!(
        retried
            .iter()
            .map(|issue| issue.number.get())
            .collect::<Vec<_>>(),
        vec![1, 2]
    );
    assert_eq!(
        client.recorded()[2]
            .query
            .iter()
            .find(|(key, _)| key == "page")
            .map(|(_, value)| value.as_str()),
        Some("1"),
        "retry starts from the uncommitted provider page"
    );
}

#[test]
fn duplicate_rows_cannot_exceed_the_fixed_provider_request_ceiling() {
    let client = MockHttpClient::new();
    for _ in 0..MAX_CANDIDATE_PROVIDER_REQUESTS {
        client.push_response(
            200,
            format!(
                "[{}]",
                issue_json(1, "closed", r#"[{"id":1,"name":"ready"}]"#, "")
            ),
        );
    }
    let config = ForgejoConfig::new("https://forge.example.com", "test-token").with_page_limit(1);
    let forge = ForgejoForge::with_client(config, client.clone());
    let page = block_on(forge.list_issue_candidates(
        &repo_id(),
        IssueCandidateQuery {
            lifecycle: CandidateLifecycle::Terminal,
            labels: CandidateLabelSelection::AnyOf(vec!["ready".into()]),
            page: Some(CandidatePageRequest::first(2)),
            ..IssueCandidateQuery::default()
        },
    ))
    .unwrap();

    assert_eq!(page.returned_count, 1);
    assert!(page.overflow, "a full provider stream is not exhausted");
    assert_eq!(client.call_count(), MAX_CANDIDATE_PROVIDER_REQUESTS);
}
