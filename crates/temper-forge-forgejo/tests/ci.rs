// SPDX-License-Identifier: MPL-2.0
//! Offline contracts for Forgejo 16 per-provider-run Actions job reads.
#[path = "ci/pagination.rs"]
mod pagination;
mod support;
#[path = "ci/transport.rs"]
mod transport;

use serde_json::{Value, json};
use support::{MockHttpClient, block_on, forge, pull_id, repo_id};
use temper_forge_forgejo::{HttpMethod, HttpRequest};
use temper_forge_model::{
    CiJobConclusion, CiJobId, CiJobQuery, CiJobSort, CiJobSortField, CiJobStatus, ForgeError,
    SortDirection,
};

const HEAD: &str = "abcdef1234567";

fn run(id: u64, display: u64, prettyref: &str, branch: &str, sha: &str, status: &str) -> Value {
    json!({
        "id": id,
        "status": status,
        "prettyref": prettyref,
        "head_branch": branch,
        "head_sha": sha,
        "html_url": format!("https://forge.example.com/acme/widgets/actions/runs/{display}"),
        "created_at": "2024-01-02T00:00:00Z",
        "updated_at": "2024-01-02T00:05:00Z"
    })
}

fn job(id: u64, run_id: u64, attempt: u64, task_id: u64, name: &str, status: &str) -> Value {
    json!({
        "id": id,
        "run_id": run_id,
        "attempt": attempt,
        "task_id": task_id,
        "name": name,
        "status": status
    })
}

fn runs(rows: Vec<Value>) -> String {
    json!({ "workflow_runs": rows }).to_string()
}

fn jobs(rows: Vec<Value>) -> String {
    Value::Array(rows).to_string()
}

fn pull(number: u64, head_ref: &str, head_sha: &str) -> String {
    json!({
        "number": number,
        "state": "open",
        "user": { "login": "author" },
        "head": { "ref": head_ref, "sha": head_sha },
        "base": { "ref": "main" },
        "created_at": "2024-01-01T00:00:00Z",
        "updated_at": "2024-01-01T00:00:00Z"
    })
    .to_string()
}

fn assert_run_list_pagination(request: &HttpRequest) {
    let page = request
        .query
        .iter()
        .find(|(key, _)| key == "page")
        .and_then(|(_, value)| value.parse::<u32>().ok());
    let limit = request
        .query
        .iter()
        .find(|(key, _)| key == "limit")
        .map(|(_, value)| value.as_str());
    assert!(page.is_some_and(|page| page >= 1), "{:?}", request.query);
    assert_eq!(limit, Some("200"), "{:?}", request.query);
    assert_eq!(request.query.len(), 2, "{:?}", request.query);
}

fn assert_api_only(requests: &[HttpRequest]) {
    for request in requests {
        assert_eq!(request.method, HttpMethod::Get);
        assert!(request.path.starts_with("/api/v1/"), "{}", request.path);
        assert!(
            request
                .headers
                .iter()
                .any(|(name, value)| name == "Authorization" && value == "token test-token")
        );
        assert!(
            request
                .headers
                .iter()
                .any(|(name, value)| name == "Accept" && value == "application/json")
        );
        assert!(!request.path.contains("/actions/tasks"));
        assert!(!request.path.contains("/user/login"));
        assert!(!request.path.ends_with("/acme/widgets/actions"));
        assert_ne!(request.method, HttpMethod::Post);
        if request.path.ends_with("/actions/runs") {
            assert_run_list_pagination(request);
        } else if request.path.contains("/actions/runs/") && request.path.ends_with("/jobs") {
            assert!(request.query.is_empty());
        }
    }
}

#[test]
fn list_uses_provider_run_route_and_provider_job_attempt_task_identity() {
    let client = MockHttpClient::new();
    client.push_response(200, pull(7, "feature", HEAD));
    client.push_response(
        200,
        runs(vec![
            run(900, 10, "#7", "feature", HEAD, "failure"),
            run(901, 11, "#8", "other", "9999999999999", "success"),
        ]),
    );
    // Repeated names and response order do not infer attempts. Each provider
    // job carries its own latest attempt, so jobs at lower attempts remain
    // visible; provider identity, not an array index, forms each id.
    client.push_response(
        200,
        jobs(vec![
            job(44, 900, 2, 504, "test", "failure"),
            job(11, 900, 1, 501, "build", "success"),
            job(33, 900, 2, 503, "build", "success"),
            job(22, 900, 1, 502, "test", "success"),
        ]),
    );

    let listing = block_on(forge(client.clone()).list_ci_jobs_with_presence(
        &repo_id(),
        CiJobQuery {
            pull_request_id: Some(pull_id(7)),
            ..Default::default()
        },
    ))
    .unwrap();

    assert!(listing.matching_ci_present());
    let listed = listing.jobs();
    assert_eq!(listed.len(), 4);
    assert_eq!(listed[0].name, "build");
    assert_eq!(
        listed[0].id.as_str(),
        "forgejo:acme/widgets:actions:900:11:1:501"
    );
    assert_eq!(listed[1].name, "build");
    assert_eq!(
        listed[1].id.as_str(),
        "forgejo:acme/widgets:actions:900:33:2:503"
    );
    assert_eq!(listed[2].name, "test");
    assert_eq!(
        listed[2].id.as_str(),
        "forgejo:acme/widgets:actions:900:22:1:502"
    );
    assert_eq!(listed[3].name, "test");
    assert_eq!(
        listed[3].id.as_str(),
        "forgejo:acme/widgets:actions:900:44:2:504"
    );
    assert_eq!(listed[0].run_id.as_deref(), Some("900"));
    assert_eq!(listed[0].attempt.as_deref(), Some("1"));
    assert_eq!(listed[1].attempt.as_deref(), Some("2"));
    assert_eq!(listed[0].pull_request_id, Some(pull_id(7)));
    assert_eq!(listed[0].commit_sha, HEAD);
    assert_eq!(
        listed[0].url.as_deref(),
        Some("https://forge.example.com/acme/widgets/actions/runs/10")
    );
    assert_eq!(listed[3].conclusion, Some(CiJobConclusion::Unknown));

    let recorded = client.recorded();
    assert_eq!(
        recorded[2].path,
        "/api/v1/repos/acme/widgets/actions/runs/900/jobs"
    );
    assert!(recorded[2].query.is_empty());
    assert_api_only(&recorded);
}

#[test]
fn every_matched_run_gets_its_own_jobs_request_and_pr_history_is_preserved() {
    let client = MockHttpClient::new();
    client.push_response(200, pull(7, "feature", "newhead1234567"));
    client.push_response(
        200,
        runs(vec![
            run(701, 1, "feature", "", "oldhead1234567", "failure"),
            run(702, 2, "feature", "", "newhead1234567", "success"),
        ]),
    );
    // Runs have equal timestamps in this fixture, so the provider display index
    // orders 702 first. Each response is still scoped by database id.
    client.push_response(200, jobs(vec![job(72, 702, 1, 82, "build", "success")]));
    client.push_response(200, jobs(vec![job(71, 701, 1, 81, "build", "failure")]));

    let result = block_on(forge(client.clone()).list_ci_jobs(
        &repo_id(),
        CiJobQuery {
            pull_request_id: Some(pull_id(7)),
            sort: Some(CiJobSort {
                field: CiJobSortField::CreatedAt,
                direction: SortDirection::Asc,
            }),
            ..Default::default()
        },
    ))
    .unwrap();

    assert_eq!(result.len(), 2);
    assert_eq!(
        result
            .iter()
            .map(|job| job.commit_sha.as_str())
            .collect::<Vec<_>>(),
        ["oldhead1234567", "newhead1234567"]
    );
    let recorded = client.recorded();
    assert_eq!(
        recorded[2].path,
        "/api/v1/repos/acme/widgets/actions/runs/702/jobs"
    );
    assert_eq!(
        recorded[3].path,
        "/api/v1/repos/acme/widgets/actions/runs/701/jobs"
    );
    assert_api_only(&recorded);
}

#[test]
fn explicit_empty_jobs_keeps_matching_ci_presence() {
    let client = MockHttpClient::new();
    client.push_response(
        200,
        runs(vec![run(900, 10, "main", "main", HEAD, "queued")]),
    );
    client.push_response(200, "[]");

    let listing = block_on(forge(client.clone()).list_ci_jobs_with_presence(
        &repo_id(),
        CiJobQuery {
            commit_sha: Some(HEAD.to_string()),
            ..Default::default()
        },
    ))
    .unwrap();

    assert!(listing.matching_ci_present());
    assert!(listing.jobs().is_empty());
    assert_api_only(&client.recorded());
}

#[test]
fn unassigned_queued_job_preserves_provider_values_and_round_trips() {
    let client = MockHttpClient::new();
    client.push_response(
        200,
        runs(vec![run(900, 10, "main", "main", HEAD, "queued")]),
    );
    client.push_response(200, jobs(vec![job(31, 900, 0, 0, "build", "waiting")]));
    client.push_response(
        200,
        runs(vec![run(900, 10, "main", "main", HEAD, "queued")]),
    );
    client.push_response(200, jobs(vec![job(31, 900, 0, 0, "build", "waiting")]));

    let forge = forge(client.clone());
    let listing = block_on(forge.list_ci_jobs_with_presence(
        &repo_id(),
        CiJobQuery {
            commit_sha: Some(HEAD.to_string()),
            ..Default::default()
        },
    ))
    .unwrap();

    assert!(listing.matching_ci_present());
    assert_eq!(listing.jobs().len(), 1);
    let listed = &listing.jobs()[0];
    assert_eq!(listed.status, CiJobStatus::Queued);
    assert_eq!(listed.attempt.as_deref(), Some("0"));
    assert_eq!(
        listed.id.as_str(),
        "forgejo:acme/widgets:actions:900:31:0:0"
    );

    let found = block_on(forge.get_ci_job(&listed.id)).unwrap().unwrap();
    assert_eq!(found.id, listed.id);
    assert_eq!(found.status, CiJobStatus::Queued);
    assert_eq!(found.attempt.as_deref(), Some("0"));
    assert_api_only(&client.recorded());
}

#[test]
fn query_values_are_not_synthetic_job_evidence() {
    let client = MockHttpClient::new();
    client.push_response(200, pull(7, "feature", HEAD));
    // This push run matches the strict head/ref target but has no provider PR
    // identity. The query PR may not be copied onto its job.
    client.push_response(
        200,
        runs(vec![run(900, 10, "feature", "feature", HEAD, "success")]),
    );
    client.push_response(200, jobs(vec![job(31, 900, 1, 41, "build", "success")]));

    let listed = block_on(forge(client).list_ci_jobs(
        &repo_id(),
        CiJobQuery {
            pull_request_id: Some(pull_id(7)),
            commit_sha: Some(HEAD.to_string()),
            ..Default::default()
        },
    ))
    .unwrap();

    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].commit_sha, HEAD);
    assert_eq!(listed[0].pull_request_id, None);
}

#[test]
fn explicit_commit_requires_provider_run_sha_and_skips_job_reads_without_it() {
    let client = MockHttpClient::new();
    client.push_response(
        200,
        runs(vec![run(
            900,
            10,
            "#7",
            "feature",
            "different1234567",
            "success",
        )]),
    );

    let listing = block_on(forge(client.clone()).list_ci_jobs_with_presence(
        &repo_id(),
        CiJobQuery {
            commit_sha: Some(HEAD.to_string()),
            ..Default::default()
        },
    ))
    .unwrap();

    assert!(!listing.matching_ci_present());
    assert!(listing.jobs().is_empty());
    assert_eq!(client.call_count(), 1);
}

#[test]
fn status_filter_and_sort_are_deterministic() {
    let client = MockHttpClient::new();
    client.push_response(
        200,
        runs(vec![run(900, 10, "main", "main", HEAD, "success")]),
    );
    client.push_response(
        200,
        jobs(vec![
            job(33, 900, 1, 43, "zebra", "success"),
            job(11, 900, 1, 41, "alpha", "failure"),
            job(22, 900, 1, 42, "mid", "running"),
        ]),
    );

    let listed = block_on(forge(client).list_ci_jobs(
        &repo_id(),
        CiJobQuery {
            commit_sha: Some(HEAD.to_string()),
            status: Some(CiJobStatus::Completed),
            sort: Some(CiJobSort {
                field: CiJobSortField::Name,
                direction: SortDirection::Desc,
            }),
            ..Default::default()
        },
    ))
    .unwrap();

    assert_eq!(
        listed
            .iter()
            .map(|job| job.name.as_str())
            .collect::<Vec<_>>(),
        ["zebra", "alpha"]
    );
    assert_eq!(listed[1].conclusion, Some(CiJobConclusion::Unknown));
}

#[test]
fn list_id_round_trips_exactly_through_get() {
    let list_client = MockHttpClient::new();
    list_client.push_response(
        200,
        runs(vec![run(900, 10, "#7", "feature", HEAD, "success")]),
    );
    list_client.push_response(200, jobs(vec![job(31, 900, 3, 41, "build", "success")]));
    let listed = block_on(forge(list_client).list_ci_jobs(
        &repo_id(),
        CiJobQuery {
            commit_sha: Some(HEAD.to_string()),
            ..Default::default()
        },
    ))
    .unwrap();
    let expected = listed[0].clone();

    let get_client = MockHttpClient::new();
    get_client.push_response(
        200,
        runs(vec![run(900, 10, "#7", "feature", HEAD, "success")]),
    );
    get_client.push_response(
        200,
        jobs(vec![
            job(30, 900, 2, 40, "build", "failure"),
            job(31, 900, 3, 41, "build", "success"),
        ]),
    );
    let found = block_on(forge(get_client.clone()).get_ci_job(&expected.id))
        .unwrap()
        .unwrap();

    assert_eq!(found.id, expected.id);
    assert_eq!(found.name, expected.name);
    assert_eq!(found.commit_sha, expected.commit_sha);
    assert_eq!(found.run_id, expected.run_id);
    assert_eq!(found.attempt, expected.attempt);
    assert_eq!(found.pull_request_id, expected.pull_request_id);
    assert_eq!(
        get_client.recorded()[1].path,
        "/api/v1/repos/acme/widgets/actions/runs/900/jobs"
    );
    assert_api_only(&get_client.recorded());
}

#[test]
fn get_requires_exact_run_job_attempt_and_task_identity() {
    for id in [
        "forgejo:acme/widgets:actions:900:32:2:41",
        "forgejo:acme/widgets:actions:900:31:3:41",
        "forgejo:acme/widgets:actions:900:31:2:42",
    ] {
        let client = MockHttpClient::new();
        client.push_response(
            200,
            runs(vec![run(900, 10, "main", "main", HEAD, "success")]),
        );
        client.push_response(200, jobs(vec![job(31, 900, 2, 41, "build", "success")]));
        assert!(
            block_on(forge(client).get_ci_job(&CiJobId::new(id)))
                .unwrap()
                .is_none()
        );
    }
}

#[test]
fn get_returns_none_for_unknown_provider_run_without_fetching_jobs() {
    let client = MockHttpClient::new();
    client.push_response(200, runs(vec![]));
    let id = CiJobId::new("forgejo:acme/widgets:actions:999:31:1:41");
    assert!(
        block_on(forge(client.clone()).get_ci_job(&id))
            .unwrap()
            .is_none()
    );
    assert_eq!(client.call_count(), 1);
}

#[test]
fn malformed_or_missing_jobs_shape_fails_closed() {
    for body in [
        "",
        "{",
        "{}",
        r#"{"jobs":null}"#,
        r#"{"jobs":[]}"#,
        r#"{"tasks":[]}"#,
    ] {
        let client = MockHttpClient::new();
        client.push_response(
            200,
            runs(vec![run(900, 10, "main", "main", HEAD, "success")]),
        );
        client.push_response(200, body);
        let error = block_on(forge(client.clone()).list_ci_jobs(
            &repo_id(),
            CiJobQuery {
                commit_sha: Some(HEAD.to_string()),
                ..Default::default()
            },
        ))
        .unwrap_err();
        assert!(
            matches!(error, ForgeError::Backend(_)),
            "body {body:?}: {error}"
        );
        assert_api_only(&client.recorded());
    }
}

#[test]
fn jobs_transport_auth_missing_and_unexpected_statuses_fail_closed_without_fallback() {
    for status in [401, 403, 404, 418] {
        let client = MockHttpClient::new();
        client.push_response(
            200,
            runs(vec![run(900, 10, "main", "main", HEAD, "success")]),
        );
        client.push_response(status, json!({ "message": "unavailable" }).to_string());
        let result = block_on(forge(client.clone()).list_ci_jobs(
            &repo_id(),
            CiJobQuery {
                commit_sha: Some(HEAD.to_string()),
                ..Default::default()
            },
        ));
        assert!(
            matches!(result, Err(ForgeError::Backend(_))),
            "status {status}"
        );
        assert_eq!(client.call_count(), 2);
        assert_api_only(&client.recorded());
    }

    let client = MockHttpClient::new();
    client.push_response(
        200,
        runs(vec![run(900, 10, "main", "main", HEAD, "success")]),
    );
    client.push_transport_error("connection reset");
    let result = block_on(forge(client.clone()).list_ci_jobs(
        &repo_id(),
        CiJobQuery {
            commit_sha: Some(HEAD.to_string()),
            ..Default::default()
        },
    ));
    assert!(matches!(result, Err(ForgeError::Backend(_))));
    assert_api_only(&client.recorded());
}

#[test]
fn invalid_or_mismatched_provider_identity_fails_closed() {
    let invalid = [
        job(0, 900, 1, 41, "build", "success"),
        job(31, 0, 1, 41, "build", "success"),
        job(31, 901, 1, 41, "build", "success"),
        job(31, 900, 1, 41, "", "success"),
        job(31, 900, 1, 41, "build", ""),
    ];
    for row in invalid {
        let client = MockHttpClient::new();
        client.push_response(
            200,
            runs(vec![run(900, 10, "main", "main", HEAD, "success")]),
        );
        client.push_response(200, jobs(vec![row]));
        assert!(
            block_on(forge(client).list_ci_jobs(
                &repo_id(),
                CiJobQuery {
                    commit_sha: Some(HEAD.to_string()),
                    ..Default::default()
                },
            ))
            .is_err()
        );
    }
}

#[test]
fn zero_provider_run_id_is_rejected_instead_of_using_display_coordinate() {
    let client = MockHttpClient::new();
    client.push_response(200, runs(vec![run(0, 77, "main", "main", HEAD, "success")]));
    let error = block_on(forge(client.clone()).list_ci_jobs(
        &repo_id(),
        CiJobQuery {
            commit_sha: Some(HEAD.to_string()),
            ..Default::default()
        },
    ))
    .unwrap_err();
    assert!(matches!(error, ForgeError::Backend(_)));
    assert_eq!(client.call_count(), 1);
}

#[test]
fn malformed_opaque_identity_is_rejected_before_http() {
    for id in [
        "forgejo:acme/widgets:actions:900:31:41",
        "forgejo:acme/widgets:actions:0:31:1:41",
        "forgejo:acme/widgets:actions:900:0:1:41",
    ] {
        let client = MockHttpClient::new();
        let result = block_on(forge(client.clone()).get_ci_job(&CiJobId::new(id)));
        assert!(matches!(result, Err(ForgeError::InvalidRequest(_))));
        assert_eq!(client.call_count(), 0);
    }
}
