//! Forgejo request-shape regressions for bounded workflow hot paths.
//!
//! These tests drive workflow reconciliation and idempotent create lookup through
//! the real Forgejo backend over a recording mock client. They lock in the HTTP
//! shapes that keep normal ticks and retry lookups away from all-history scans.

mod support;

use chrono::{DateTime, Utc};
use support::{MockHttpClient, OWNER, REPO, block_on, forge, repo_id};
use temper_forge_forgejo::{HttpMethod, HttpRequest};
use temper_forge_model::{
    BranchRef, CandidateLabelSelection, CandidateLifecycle, CreateIssue, CreatePullRequest,
    ItemNumber, PullRequestCandidateQuery, PullRequestState, UserId,
};
use temper_workflow::{
    DefaultRecoveryPolicy, EnsureOutcome, InMemoryJournal, RawWorkflowSpec, ValidatedWorkflow,
    WorkflowMetadata, render_metadata_block, workflow_interest,
};

const REFERENCE_WORKFLOW: &str =
    include_str!("../../temper-workflow/fixtures/reference-delivery.json");

fn workflow() -> ValidatedWorkflow {
    let spec: RawWorkflowSpec =
        serde_json::from_str(REFERENCE_WORKFLOW).expect("reference workflow parses");
    assert_eq!(spec.labels.len(), 17, "request budget fixture label count");
    spec.validate().expect("reference workflow validates")
}

fn ts(value: &str) -> DateTime<Utc> {
    value.parse().expect("valid RFC 3339 timestamp")
}

fn label_values(labels: &[&str]) -> serde_json::Value {
    serde_json::Value::Array(
        labels
            .iter()
            .enumerate()
            .map(|(index, label)| serde_json::json!({ "id": index + 1, "name": label }))
            .collect(),
    )
}

fn issue_json(number: u64, state: &str, body: &str, labels: &[&str]) -> String {
    serde_json::json!({
        "number": number,
        "title": format!("Issue {number}"),
        "body": body,
        "state": state,
        "user": { "login": "author" },
        "labels": label_values(labels),
        "created_at": "2024-03-01T00:00:00Z",
        "updated_at": "2024-03-02T00:00:00Z",
        "repository": {"name": REPO, "full_name": format!("{OWNER}/{REPO}")}
    })
    .to_string()
}

fn pr_issue_json(number: u64, state: &str, body: &str, labels: &[&str]) -> String {
    pr_issue_json_with_merge(number, state, body, labels, None)
}

fn pr_issue_json_with_merge(
    number: u64,
    state: &str,
    body: &str,
    labels: &[&str],
    merged: Option<bool>,
) -> String {
    let marker = match merged {
        Some(true) => serde_json::json!({
            "url": format!("https://forge.example.com/{OWNER}/{REPO}/pulls/{number}"),
            "merged": true,
            "merged_at": "2024-03-03T00:00:00Z"
        }),
        Some(false) => serde_json::json!({
            "url": format!("https://forge.example.com/{OWNER}/{REPO}/pulls/{number}"),
            "merged": false,
            "merged_at": null
        }),
        None => serde_json::json!({
            "url": format!("https://forge.example.com/{OWNER}/{REPO}/pulls/{number}")
        }),
    };
    serde_json::json!({
        "number": number,
        "title": format!("PR {number}"),
        "body": body,
        "state": state,
        "user": { "login": "author" },
        "labels": label_values(labels),
        "created_at": "2024-03-01T00:00:00Z",
        "updated_at": "2024-03-02T00:00:00Z",
        "repository": {"name": REPO, "full_name": format!("{OWNER}/{REPO}")},
        "pull_request": marker
    })
    .to_string()
}

fn body_with_correlation(correlation_key: &str) -> String {
    render_metadata_block(&WorkflowMetadata {
        correlation_key: Some(correlation_key.to_string()),
        ..WorkflowMetadata::default()
    })
}

fn query_value<'a>(request: &'a HttpRequest, key: &str) -> Option<&'a str> {
    request
        .query
        .iter()
        .find(|(candidate, _)| candidate == key)
        .map(|(_, value)| value.as_str())
}

fn has_query(request: &HttpRequest, key: &str, value: &str) -> bool {
    request
        .query
        .iter()
        .any(|(candidate, candidate_value)| candidate == key && candidate_value == value)
}

fn issue_list_path() -> String {
    format!("/api/v1/repos/{OWNER}/{REPO}/issues")
}

fn candidate_search_path() -> &'static str {
    "/api/v1/repos/issues/search"
}

fn pull_list_path() -> String {
    format!("/api/v1/repos/{OWNER}/{REPO}/pulls")
}

fn assert_no_all_history_lists(requests: &[HttpRequest]) {
    assert!(!requests.iter().any(|request| {
        (request.path == issue_list_path() || request.path == pull_list_path())
            && has_query(request, "state", "all")
    }));
    assert!(!requests.iter().any(|request| {
        matches!(query_value(request, "state"), Some("closed"))
            && query_value(request, "labels").is_none()
    }));
}

#[test]
fn reference_workflow_bounded_reconciliation_uses_four_one_page_buckets() {
    let client = MockHttpClient::new();
    // The checked-in 17-label workflow has both artifact kinds and both
    // lifecycle states populated in its interest plan. Empty provider pages
    // keep this assertion focused on aggregate list shape, not enrichment.
    for _ in 0..4 {
        client.push_response(200, "[]");
    }

    let forge = forge(client.clone());
    let workflow = workflow();
    let policy = DefaultRecoveryPolicy;
    let journal = InMemoryJournal::new();

    let report = block_on(workflow.reconciler(&policy).reconcile(
        &forge,
        &repo_id(),
        &journal,
        ts("2026-05-29T00:00:00Z"),
    ))
    .expect("bounded reconciliation succeeds");

    assert_eq!(report.snapshot_count, 0);
    let requests = client.recorded();
    assert_no_all_history_lists(&requests);
    assert_eq!(
        requests.len(),
        4,
        "17 labels must collapse to issue/PR x open/terminal buckets"
    );
    assert!(requests.iter().all(|request| {
        request.method == HttpMethod::Get && request.path == candidate_search_path()
    }));
    assert_eq!(
        requests
            .iter()
            .filter(|request| has_query(request, "type", "issues"))
            .count(),
        2
    );
    assert_eq!(
        requests
            .iter()
            .filter(|request| has_query(request, "type", "pulls"))
            .count(),
        2
    );
    assert_eq!(
        requests
            .iter()
            .filter(|request| has_query(request, "state", "open"))
            .count(),
        2
    );
    assert_eq!(
        requests
            .iter()
            .filter(|request| has_query(request, "state", "closed"))
            .count(),
        2
    );
    assert!(requests.iter().all(|request| {
        query_value(request, "labels").is_some() || has_query(request, "state", "open")
    }));
    assert!(
        !requests
            .iter()
            .any(|request| request.path.ends_with("/dependencies"))
    );
}

#[test]
fn reference_terminal_pr_bucket_adds_exact_read_only_for_ambiguous_row() {
    let client = MockHttpClient::new();
    let unambiguous =
        pr_issue_json_with_merge(7, "closed", "", &["implementation", "landed"], Some(true));
    client.push_response(
        200,
        format!(
            "[{unambiguous},{}]",
            pr_issue_json(8, "closed", "", &["implementation", "landed"]),
        ),
    );
    client.push_response(
        200,
        serde_json::json!({
            "number": 8,
            "title": "PR 8",
            "body": "",
            "state": "closed",
            "merged": false,
            "user": {"login": "author"},
            "head": {"ref": "feature", "sha": "head8"},
            "base": {"ref": "main", "sha": "base8"},
            "labels": label_values(&["implementation", "landed"]),
            "created_at": "2024-03-01T00:00:00Z",
            "updated_at": "2024-03-02T00:00:00Z"
        })
        .to_string(),
    );
    let forge = forge(client.clone());
    let workflow = workflow();
    let labels = workflow_interest(&workflow)
        .terminal_labels(temper_workflow::ArtifactTarget::PullRequest)
        .to_vec();

    let pulls = block_on(forge.list_pull_request_candidates(
        &repo_id(),
        PullRequestCandidateQuery {
            lifecycle: CandidateLifecycle::Terminal,
            labels: CandidateLabelSelection::AnyOf(labels),
            ..PullRequestCandidateQuery::default()
        },
    ))
    .expect("terminal candidate discovery succeeds");

    assert_eq!(pulls.len(), 2);
    let requests = client.recorded();
    assert_eq!(requests.len(), 2, "one bucket plus one ambiguous fallback");
    assert_eq!(requests[0].path, candidate_search_path());
    assert!(query_value(&requests[0], "labels").is_some());
    assert_eq!(
        requests[1].path,
        format!("/api/v1/repos/{OWNER}/{REPO}/pulls/8")
    );
    assert!(
        requests
            .iter()
            .all(|request| request.path != format!("/api/v1/repos/{OWNER}/{REPO}/pulls/7"))
    );
}

#[test]
fn correlation_lookup_uses_labelled_state_queries_and_client_side_body_filtering() {
    let issue_key = "issue-key";
    let issue_marker = "\"correlation_key\": \"issue-key\"";
    let pr_key = "pr-key";
    let client = MockHttpClient::new();

    client.push_response(
        200,
        format!(
            "[{},{}]",
            issue_json(
                1,
                "open",
                &format!("prose mentions {issue_marker} but has no metadata"),
                &["code"],
            ),
            issue_json(2, "open", &body_with_correlation(issue_key), &["code"]),
        ),
    );
    client.push_response(200, "[]");

    client.push_response(200, "[]");
    client.push_response(200, "[]");
    client.push_response(
        200,
        format!(
            "[{}]",
            pr_issue_json_with_merge(
                7,
                "closed",
                &body_with_correlation(pr_key),
                &["implementation"],
                Some(true),
            )
        ),
    );
    client.push_response(
        200,
        serde_json::json!({
            "number": 7,
            "title": "PR 7",
            "body": body_with_correlation(pr_key),
            "state": "closed",
            "merged": true,
            "merged_at": "2024-03-03T00:00:00Z",
            "user": {"login": "author"},
            "head": {"ref": "feature", "sha": "head7"},
            "base": {"ref": "main", "sha": "base7"},
            "labels": label_values(&["implementation"]),
            "created_at": "2024-03-01T00:00:00Z",
            "updated_at": "2024-03-03T00:00:00Z"
        })
        .to_string(),
    );

    let forge = forge(client.clone());
    let workflow = workflow();
    let executor = workflow.executor(&forge);

    let issue = block_on(executor.ensure_issue(
        &repo_id(),
        issue_key,
        CreateIssue {
            title: "new issue".into(),
            body: String::new(),
            labels: vec!["code".into()],
            assignees: Vec::<UserId>::new(),
        },
    ))
    .expect("issue ensure succeeds");
    assert!(matches!(issue, EnsureOutcome::Existing(_)));
    assert_eq!(issue.artifact().number, ItemNumber::new(2));

    let pull_request = block_on(executor.ensure_pull_request(
        &repo_id(),
        pr_key,
        CreatePullRequest {
            title: "new pr".into(),
            body: String::new(),
            source: BranchRef {
                repository_id: repo_id(),
                branch: "feature".into(),
            },
            target: BranchRef {
                repository_id: repo_id(),
                branch: "main".into(),
            },
            labels: vec!["implementation".into()],
            assignees: Vec::<UserId>::new(),
        },
    ))
    .expect("pull-request ensure succeeds");
    assert!(matches!(pull_request, EnsureOutcome::Existing(_)));
    assert_eq!(pull_request.artifact().number, ItemNumber::new(7));
    assert_eq!(pull_request.artifact().state, PullRequestState::Merged);

    let requests = client.recorded();
    assert_no_all_history_lists(&requests);

    let issue_lists: Vec<&HttpRequest> = requests
        .iter()
        .filter(|request| request.path == issue_list_path() && has_query(request, "type", "issues"))
        .collect();
    assert_eq!(issue_lists.len(), 2);
    assert!(
        issue_lists
            .iter()
            .any(|request| has_query(request, "state", "open"))
    );
    assert!(
        issue_lists
            .iter()
            .any(|request| has_query(request, "state", "closed"))
    );
    for request in issue_lists {
        assert!(has_query(request, "labels", "code"));
        // The backend does not rely on provider exact body search. It keeps
        // the request narrowed by state+labels and applies the portable
        // body_contains filter client-side.
        assert!(query_value(request, "q").is_none());
        assert!(query_value(request, "body").is_none());
    }

    let pr_discovery: Vec<&HttpRequest> = requests
        .iter()
        .filter(|request| request.path == issue_list_path() && has_query(request, "type", "pulls"))
        .collect();
    assert_eq!(pr_discovery.len(), 3);
    assert_eq!(
        pr_discovery
            .iter()
            .filter(|request| has_query(request, "state", "closed"))
            .count(),
        2
    );
    for request in pr_discovery {
        assert!(has_query(request, "labels", "implementation"));
        assert!(query_value(request, "q").is_none());
        assert!(query_value(request, "body").is_none());
    }
    assert!(
        !requests.iter().any(|request| {
            request.path == pull_list_path() && has_query(request, "state", "all")
        })
    );
    assert!(
        requests
            .iter()
            .any(|request| { request.path == format!("/api/v1/repos/{OWNER}/{REPO}/pulls/7") })
    );
}
