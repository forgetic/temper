// SPDX-License-Identifier: MPL-2.0
//! Bounded pagination contracts for Forgejo 16 Actions run inventory.
use super::support::{MockHttpClient, block_on, forge, repo_id};
use serde_json::{Value, json};
use temper_forge_forgejo::{HttpMethod, HttpRequest};
use temper_forge_model::{CiJobId, CiJobQuery};

const HEAD: &str = "abcdef1234567";

fn run(id: u64, sha: &str) -> Value {
    json!({
        "id": id,
        "status": "success",
        "prettyref": "main",
        "head_branch": "main",
        "head_sha": sha,
        "html_url": format!("https://forge.example.com/acme/widgets/actions/runs/{id}"),
        "created_at": "2024-01-02T00:00:00Z",
        "updated_at": "2024-01-02T00:05:00Z"
    })
}

fn job(id: u64, run_id: u64, name: &str) -> Value {
    json!({
        "id": id,
        "run_id": run_id,
        "attempt": 1,
        "task_id": id + 10,
        "name": name,
        "status": "success"
    })
}

fn workflow_runs(rows: Vec<Value>) -> String {
    json!({ "workflow_runs": rows }).to_string()
}

fn runs_alias(rows: Vec<Value>) -> String {
    json!({ "runs": rows }).to_string()
}

fn jobs(rows: Vec<Value>) -> String {
    Value::Array(rows).to_string()
}

fn noise_runs(first_id: u64, count: usize) -> Vec<Value> {
    (0..count)
        .map(|offset| run(first_id + offset as u64, "noise0000000"))
        .collect()
}

fn assert_bounded_api_requests(requests: &[HttpRequest]) {
    for request in requests {
        assert_eq!(request.method, HttpMethod::Get);
        assert!(request.path.starts_with("/api/v1/"));
        assert!(!request.path.contains("/actions/tasks"));
        assert!(!request.path.contains("/user/login"));
        if request.path.ends_with("/actions/runs") {
            let page = request
                .query
                .iter()
                .find(|(key, _)| key == "page")
                .and_then(|(_, value)| value.parse::<u32>().ok());
            assert!(page.is_some_and(|page| page >= 1), "{:?}", request.query);
            assert!(
                request
                    .query
                    .contains(&("limit".to_string(), "50".to_string())),
                "{:?}",
                request.query
            );
            assert_eq!(request.query.len(), 2, "{:?}", request.query);
        } else if request.path.ends_with("/jobs") {
            assert!(request.query.is_empty());
        }
    }
}

#[test]
fn later_page_exact_head_runs_are_aggregated_before_stable_selection() {
    let client = MockHttpClient::new();
    let mut first_page = noise_runs(1_000, 49);
    first_page.push(run(900, HEAD));
    client.push_response(200, workflow_runs(first_page));
    client.push_response(200, runs_alias(vec![run(902, HEAD)]));
    // Equal timestamps make provider id the deterministic newest-first tie-break.
    client.push_response(200, jobs(vec![job(32, 902, "new")]));
    client.push_response(200, jobs(vec![job(30, 900, "old")]));

    let listing = block_on(forge(client.clone()).list_ci_jobs_with_presence(
        &repo_id(),
        CiJobQuery {
            commit_sha: Some(HEAD.to_string()),
            ..Default::default()
        },
    ))
    .unwrap();

    assert!(listing.matching_ci_present());
    assert_eq!(listing.jobs().len(), 2);
    let recorded = client.recorded();
    assert_eq!(recorded[0].query[0], ("page".to_string(), "1".to_string()));
    assert_eq!(recorded[1].query[0], ("page".to_string(), "2".to_string()));
    assert!(recorded[2].path.ends_with("/actions/runs/902/jobs"));
    assert!(recorded[3].path.ends_with("/actions/runs/900/jobs"));
    assert_bounded_api_requests(&recorded);
}

#[test]
fn opaque_get_finds_a_provider_run_after_page_one() {
    let client = MockHttpClient::new();
    client.push_response(200, workflow_runs(noise_runs(1_000, 50)));
    client.push_response(200, workflow_runs(vec![run(900, HEAD)]));
    client.push_response(200, jobs(vec![job(31, 900, "build")]));

    let found = block_on(
        forge(client.clone()).get_ci_job(&CiJobId::new("forgejo:acme/widgets:actions:900:31:1:41")),
    )
    .unwrap()
    .unwrap();

    assert_eq!(
        found.id.as_str(),
        "forgejo:acme/widgets:actions:900:31:1:41"
    );
    let recorded = client.recorded();
    assert_eq!(recorded.len(), 3);
    assert!(recorded[2].path.ends_with("/actions/runs/900/jobs"));
    assert_bounded_api_requests(&recorded);
}

#[test]
fn short_and_empty_pages_terminate_without_extra_requests() {
    for body in [
        workflow_runs(vec![run(900, "other0000000")]),
        workflow_runs(vec![]),
    ] {
        let client = MockHttpClient::new();
        client.push_response(200, body);

        let listed = block_on(forge(client.clone()).list_ci_jobs(
            &repo_id(),
            CiJobQuery {
                commit_sha: Some(HEAD.to_string()),
                ..Default::default()
            },
        ))
        .unwrap();

        assert!(listed.is_empty());
        assert_eq!(client.call_count(), 1);
        assert_bounded_api_requests(&client.recorded());
    }
}

#[test]
fn repeated_full_pages_fail_closed_without_an_unpaged_fallback() {
    let client = MockHttpClient::new();
    let mut rows = noise_runs(1_000, 50);
    rows[0]["prettyref"] = json!("RESPONSE-BODY-SENTINEL");
    let body = workflow_runs(rows);
    client.push_response(200, body.clone());
    client.push_response(200, body);

    let error = block_on(forge(client.clone()).list_ci_jobs(
        &repo_id(),
        CiJobQuery {
            commit_sha: Some(HEAD.to_string()),
            ..Default::default()
        },
    ))
    .unwrap_err();

    let rendered = error.to_string();
    assert!(rendered.contains("failure=non_advancing_page"));
    assert!(rendered.contains("page=2 limit=50"));
    assert!(!rendered.contains("RESPONSE-BODY-SENTINEL"));
    assert!(!rendered.contains("test-token"));
    assert_eq!(client.call_count(), 2);
    assert_bounded_api_requests(&client.recorded());
}

#[test]
fn full_page_at_the_fixed_ceiling_fails_closed() {
    let client = MockHttpClient::new();
    for page in 0..64_u64 {
        client.push_response(200, workflow_runs(noise_runs(10_000 + page * 50, 50)));
    }

    let error = block_on(forge(client.clone()).list_ci_jobs(
        &repo_id(),
        CiJobQuery {
            commit_sha: Some(HEAD.to_string()),
            ..Default::default()
        },
    ))
    .unwrap_err();

    let rendered = error.to_string();
    assert!(rendered.contains("failure=page_ceiling"));
    assert!(rendered.contains("page=64 limit=50"));
    assert_eq!(client.call_count(), 64);
    assert_bounded_api_requests(&client.recorded());
}

#[test]
fn pagination_failures_report_only_bounded_redacted_diagnostics() {
    let status_client = MockHttpClient::new();
    status_client.push_response(401, "STATUS-BODY-SENTINEL token=provider-secret");
    let status_error =
        block_on(forge(status_client.clone()).list_ci_jobs(&repo_id(), CiJobQuery::default()))
            .unwrap_err()
            .to_string();
    assert!(
        status_error
            .contains("endpoint=/api/v1/repos/{owner}/{repo}/actions/runs operation=list_runs")
    );
    assert!(status_error.contains("page=1 limit=50 status=401 failure=status"));
    assert!(status_error.contains("response_bytes="));
    assert!(!status_error.contains("STATUS-BODY-SENTINEL"));
    assert!(!status_error.contains("provider-secret"));
    assert!(!status_error.contains("test-token"));
    assert_eq!(status_client.call_count(), 1);
    assert_bounded_api_requests(&status_client.recorded());

    let malformed_client = MockHttpClient::new();
    malformed_client.push_response(200, r#"{"workflow_runs":["MALFORMED-BODY-SENTINEL"]}"#);
    let malformed_error =
        block_on(forge(malformed_client.clone()).list_ci_jobs(&repo_id(), CiJobQuery::default()))
            .unwrap_err()
            .to_string();
    assert!(malformed_error.contains("status=none failure=malformed"));
    assert!(malformed_error.contains("response_rows=1"));
    assert!(!malformed_error.contains("MALFORMED-BODY-SENTINEL"));
    assert_bounded_api_requests(&malformed_client.recorded());

    let transport_client = MockHttpClient::new();
    transport_client.push_transport_error("TRANSPORT-SENTINEL authorization=provider-secret");
    let transport_error =
        block_on(forge(transport_client.clone()).list_ci_jobs(&repo_id(), CiJobQuery::default()))
            .unwrap_err()
            .to_string();
    assert!(transport_error.contains("status=none failure=transport"));
    assert!(!transport_error.contains("TRANSPORT-SENTINEL"));
    assert!(!transport_error.contains("provider-secret"));
    assert_bounded_api_requests(&transport_client.recorded());
}
