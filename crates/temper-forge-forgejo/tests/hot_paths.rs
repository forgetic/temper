//! Forgejo request-shape regressions for bounded workflow hot paths.
//!
//! These tests drive workflow reconciliation and idempotent create lookup through
//! the real Forgejo backend over a recording mock client. They lock in the HTTP
//! shapes that keep normal ticks and retry lookups away from all-history scans.

mod support;

use chrono::{DateTime, Utc};
use support::{block_on, forge, repo_id, MockHttpClient, OWNER, REPO};
use temper_forge::{
    BranchRef, CreateIssue, CreatePullRequest, ItemNumber, PullRequestState, UserId,
};
use temper_forge_forgejo::{HttpMethod, HttpRequest};
use temper_workflow::{
    render_metadata_block, DefaultRecoveryPolicy, EnsureOutcome, InMemoryJournal, RawWorkflowSpec,
    ValidatedWorkflow, WorkflowMetadata,
};

const HOT_PATH_WORKFLOW: &str = r#"
{
  "name": "forgejo-hot-paths",
  "labels": [
    { "id": "code" },
    { "id": "implementation" }
  ],
  "artifact_kinds": [
    { "id": "code", "target": "issue", "identifying_labels": ["code"] },
    {
      "id": "implementation_pr",
      "target": "pull_request",
      "identifying_labels": ["implementation"]
    }
  ]
}
"#;

fn workflow() -> ValidatedWorkflow {
    let spec: RawWorkflowSpec = serde_json::from_str(HOT_PATH_WORKFLOW).expect("workflow parses");
    spec.validate().expect("workflow validates")
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
        "updated_at": "2024-03-02T00:00:00Z"
    })
    .to_string()
}

fn pr_issue_json(number: u64, state: &str, body: &str, labels: &[&str]) -> String {
    serde_json::json!({
        "number": number,
        "title": format!("PR {number}"),
        "body": body,
        "state": state,
        "user": { "login": "author" },
        "labels": label_values(labels),
        "created_at": "2024-03-01T00:00:00Z",
        "updated_at": "2024-03-02T00:00:00Z",
        "pull_request": { "url": format!("https://forge.example.com/{OWNER}/{REPO}/pulls/{number}") }
    })
    .to_string()
}

fn pull_json(number: u64, state: &str, merged: bool, body: &str, labels: &[&str]) -> String {
    serde_json::json!({
        "number": number,
        "title": format!("PR {number}"),
        "body": body,
        "state": state,
        "merged": merged,
        "merge_commit_sha": if merged { Some("merge-sha") } else { None },
        "merged_at": if merged { Some("2024-03-03T00:00:00Z") } else { None },
        "user": { "login": "author" },
        "head": { "ref": format!("feature-{number}"), "sha": format!("head{number}") },
        "base": { "ref": "main", "sha": format!("base{number}") },
        "labels": label_values(labels),
        "created_at": "2024-03-01T00:00:00Z",
        "updated_at": "2024-03-02T00:00:00Z",
        "closed_at": if state == "closed" { Some("2024-03-03T00:00:00Z") } else { None }
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

fn pull_list_path() -> String {
    format!("/api/v1/repos/{OWNER}/{REPO}/pulls")
}

fn assert_no_all_history_lists(requests: &[HttpRequest]) {
    assert!(!requests.iter().any(|request| {
        (request.path == issue_list_path() || request.path == pull_list_path())
            && has_query(request, "state", "all")
    }));
    assert!(!requests.iter().any(|request| {
        request.path == issue_list_path()
            && matches!(query_value(request, "state"), Some("closed"))
            && query_value(request, "labels").is_none()
    }));
    assert!(!requests.iter().any(|request| {
        request.path == pull_list_path()
            && matches!(query_value(request, "state"), Some("closed"))
            && query_value(request, "labels").is_none()
    }));
}

#[test]
fn bounded_reconciliation_uses_state_label_summary_forgejo_queries() {
    let client = MockHttpClient::new();
    // Issue candidate queries: open(code), open(implementation), closed(code),
    // closed(implementation). One summary candidate proves no dependency
    // enrichment happens on list results.
    client.push_response(200, format!("[{}]", issue_json(1, "open", "", &["code"])));
    client.push_response(200, "[]");
    client.push_response(200, "[]");
    client.push_response(200, "[]");
    // PR candidate queries: open(code), open(implementation), closed(code),
    // closed(implementation), merged(code), merged(implementation). Labelled PR
    // discovery uses the issue label index, then exact `/pulls/{number}` detail
    // for the matching candidate only.
    client.push_response(200, "[]");
    client.push_response(
        200,
        format!("[{}]", pr_issue_json(2, "open", "", &["implementation"])),
    );
    client.push_response(200, pull_json(2, "open", false, "", &["implementation"]));
    client.push_response(200, "[]");
    client.push_response(200, "[]");
    client.push_response(200, "[]");
    client.push_response(200, "[]");

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

    assert_eq!(report.snapshot_count, 2);
    let requests = client.recorded();
    assert_no_all_history_lists(&requests);
    assert!(!requests
        .iter()
        .any(|request| request.path.ends_with("/dependencies")));

    let list_requests: Vec<&HttpRequest> = requests
        .iter()
        .filter(|request| request.path == issue_list_path() || request.path == pull_list_path())
        .collect();
    assert_eq!(list_requests.len(), 10);
    for request in list_requests {
        assert_eq!(request.method, HttpMethod::Get);
        assert!(matches!(
            query_value(request, "state"),
            Some("open" | "closed")
        ));
        assert!(query_value(request, "labels").is_some());
    }
    assert!(requests
        .iter()
        .all(|request| request.path != pull_list_path()));
    assert!(requests
        .iter()
        .any(|request| request.path == format!("/api/v1/repos/{OWNER}/{REPO}/pulls/2")));
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
            pr_issue_json(
                7,
                "closed",
                "body is only an index row",
                &["implementation"]
            )
        ),
    );
    client.push_response(
        200,
        pull_json(
            7,
            "closed",
            true,
            &body_with_correlation(pr_key),
            &["implementation"],
        ),
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
    assert!(issue_lists
        .iter()
        .any(|request| has_query(request, "state", "open")));
    assert!(issue_lists
        .iter()
        .any(|request| has_query(request, "state", "closed")));
    for request in issue_lists {
        assert!(has_query(request, "labels", "code"));
        // Forgejo 7.0.x has no reliable exact body search. The backend keeps
        // the provider request narrowed by state+labels and applies the portable
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
    assert!(!requests
        .iter()
        .any(|request| { request.path == pull_list_path() && has_query(request, "state", "all") }));
    assert!(requests
        .iter()
        .any(|request| request.path == format!("/api/v1/repos/{OWNER}/{REPO}/pulls/7")));
}
