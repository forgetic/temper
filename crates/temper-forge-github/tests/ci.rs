//! Offline contract tests for GitHub Actions CI reads.

mod support;

use support::{MockHttpClient, block_on, forge, pull_id, repo_id};
use temper_forge_github::HttpMethod;
use temper_forge_model::{
    CiJob, CiJobConclusion, CiJobId, CiJobQuery, CiJobStatus, CiRetryOutcome, CiRetryRejection,
    CiRetryRequest, ItemNumber,
};

fn runs_envelope(run_id: u64, head_sha: &str) -> String {
    format!(
        r#"{{
            "total_count": 1,
            "workflow_runs": [{{"id": {run_id}, "head_sha": "{head_sha}", "status": "completed"}}]
        }}"#
    )
}

fn jobs_envelope() -> String {
    r#"{
        "total_count": 2,
        "jobs": [
            {
                "id": 200,
                "run_id": 12,
                "head_sha": "abc123",
                "name": "test",
                "status": "in_progress",
                "conclusion": null,
                "html_url": "https://github.com/acme/widgets/runs/200",
                "created_at": "2024-01-02T03:00:00Z",
                "started_at": "2024-01-02T03:01:00Z",
                "completed_at": null
            },
            {
                "id": 100,
                "run_id": 12,
                "head_sha": "abc123",
                "name": "build",
                "status": "completed",
                "conclusion": "success",
                "html_url": "https://github.com/acme/widgets/runs/100",
                "created_at": "2024-01-02T03:00:00Z",
                "started_at": "2024-01-02T03:01:00Z",
                "completed_at": "2024-01-02T03:05:00Z"
            }
        ]
    }"#
    .to_string()
}

fn pull_json(number: u64, head_sha: &str) -> String {
    format!(
        r#"{{
            "number": {number},
            "title": "a pull",
            "state": "open",
            "user": {{"login": "author"}},
            "head": {{"ref": "feature", "sha": "{head_sha}"}},
            "base": {{"ref": "main", "sha": "basesha"}},
            "created_at": "2024-03-01T00:00:00Z",
            "updated_at": "2024-03-02T00:00:00Z"
        }}"#
    )
}

fn retry_run_json(attempt: u64) -> String {
    format!(
        r#"{{
            "id": 12,
            "run_attempt": {attempt},
            "head_sha": "abc123",
            "status": "completed",
            "conclusion": "failure"
        }}"#
    )
}

fn retry_jobs_envelope() -> String {
    r#"{
        "total_count": 1,
        "jobs": [{
            "id": 300,
            "run_id": 12,
            "run_attempt": 2,
            "head_sha": "abc123",
            "name": "validate",
            "status": "completed",
            "conclusion": "runner_lost",
            "failure_reason": "runner disconnected",
            "html_url": "https://github.com/acme/widgets/runs/300",
            "created_at": "2024-01-02T03:00:00Z",
            "started_at": "2024-01-02T03:01:00Z",
            "completed_at": "2024-01-02T03:05:00Z"
        }]
    }"#
    .to_string()
}

fn retry_request() -> CiRetryRequest {
    let created_at = chrono::DateTime::parse_from_rfc3339("2024-01-02T03:00:00Z")
        .unwrap()
        .to_utc();
    let started_at = chrono::DateTime::parse_from_rfc3339("2024-01-02T03:01:00Z")
        .unwrap()
        .to_utc();
    let completed_at = chrono::DateTime::parse_from_rfc3339("2024-01-02T03:05:00Z")
        .unwrap()
        .to_utc();
    let job = CiJob {
        id: CiJobId::new("github:acme/widgets:job:300"),
        repo_id: repo_id(),
        pull_request_id: Some(pull_id(5)),
        commit_sha: "abc123".into(),
        name: "validate".into(),
        status: CiJobStatus::Completed,
        conclusion: Some(CiJobConclusion::RunnerLost),
        provider_conclusion: Some("runner_lost".into()),
        provider_reason: Some("runner disconnected".into()),
        run_id: Some("12".into()),
        attempt: Some("2".into()),
        url: Some("https://github.com/acme/widgets/runs/300".into()),
        created_at,
        started_at: Some(started_at),
        completed_at: Some(completed_at),
        updated_at: completed_at,
    };
    CiRetryRequest::new(repo_id(), pull_id(5), "abc123", "12", "2", &[job]).unwrap()
}

#[test]
fn retry_ci_attempt_posts_only_the_exact_fenced_run_and_observes_a_new_attempt() {
    let client = MockHttpClient::new();
    client.push_response(200, pull_json(5, "abc123"));
    client.push_response(200, retry_run_json(2));
    client.push_response(200, retry_jobs_envelope());
    client.push_response(201, "");
    let forge = forge(client.clone());
    let request = retry_request();

    assert_eq!(
        block_on(forge.retry_ci_attempt(request.clone())).unwrap(),
        CiRetryOutcome::Accepted
    );
    let recorded = client.recorded();
    assert_eq!(recorded[0].path, "/repos/acme/widgets/pulls/5");
    assert_eq!(recorded[1].path, "/repos/acme/widgets/actions/runs/12");
    assert_eq!(recorded[2].path, "/repos/acme/widgets/actions/runs/12/jobs");
    assert_eq!(recorded[3].method, HttpMethod::Post);
    assert_eq!(
        recorded[3].path,
        "/repos/acme/widgets/actions/runs/12/rerun"
    );
    assert_eq!(recorded[3].body, None);
    assert!(recorded.iter().all(|request| {
        !request.path.contains("/git/")
            && !request.path.contains("/commits")
            && !request.path.contains("/refs")
    }));

    // A later authoritative run read showing a higher attempt reconciles the
    // prior operation without issuing another POST or relying on its response.
    client.push_response(200, pull_json(5, "abc123"));
    client.push_response(200, retry_run_json(3));
    assert_eq!(
        block_on(forge.retry_ci_attempt(request)).unwrap(),
        CiRetryOutcome::AlreadyObserved
    );
    assert_eq!(
        client
            .recorded()
            .iter()
            .filter(|request| request.method == HttpMethod::Post)
            .count(),
        1
    );
}

#[test]
fn retry_ci_attempt_reports_unsupported_and_uncertain_without_fallback_writes() {
    for (post_result, expected) in [
        (
            Ok(temper_forge_github::HttpResponse::new(404, "")),
            CiRetryOutcome::Unsupported,
        ),
        (
            Err(temper_forge_github::HttpError::Transport(
                "connection reset".into(),
            )),
            CiRetryOutcome::Uncertain,
        ),
    ] {
        let client = MockHttpClient::new();
        client.push_response(200, pull_json(5, "abc123"));
        client.push_response(200, retry_run_json(2));
        client.push_response(200, retry_jobs_envelope());
        client.push_result(post_result);
        let forge = forge(client.clone());

        assert_eq!(
            block_on(forge.retry_ci_attempt(retry_request())).unwrap(),
            expected
        );
        let writes = client
            .recorded()
            .into_iter()
            .filter(|request| request.method != HttpMethod::Get)
            .collect::<Vec<_>>();
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0].path, "/repos/acme/widgets/actions/runs/12/rerun");
    }
}

#[test]
fn retry_ci_attempt_reports_missing_attempt_identity_as_unsupported() {
    let client = MockHttpClient::new();
    client.push_response(200, pull_json(5, "abc123"));
    client.push_response(200, retry_run_json(0));
    let forge = forge(client.clone());

    assert_eq!(
        block_on(forge.retry_ci_attempt(retry_request())).unwrap(),
        CiRetryOutcome::Unsupported
    );
    assert!(
        client
            .recorded()
            .iter()
            .all(|request| request.method == HttpMethod::Get)
    );
}

#[test]
fn retry_ci_attempt_rejects_cross_repository_coordinates_before_http() {
    let mut value = serde_json::to_value(retry_request()).unwrap();
    value["repo_id"] = serde_json::json!("github:other/widgets");
    let widened: CiRetryRequest = serde_json::from_value(value).unwrap();
    let client = MockHttpClient::new();
    let forge = forge(client.clone());

    assert_eq!(
        block_on(forge.retry_ci_attempt(widened)).unwrap(),
        CiRetryOutcome::Rejected(CiRetryRejection::RepositoryMismatch)
    );
    assert_eq!(client.call_count(), 0);
}

#[test]
fn list_ci_jobs_by_commit_narrows_runs_with_head_sha() {
    let client = MockHttpClient::new();
    client.push_response(200, runs_envelope(12, "abc123"));
    client.push_response(200, jobs_envelope());
    let forge = forge(client.clone());

    let jobs = block_on(forge.list_ci_jobs(
        &repo_id(),
        CiJobQuery {
            commit_sha: Some("abc123".to_string()),
            ..CiJobQuery::default()
        },
    ))
    .unwrap();

    assert_eq!(jobs.len(), 2);
    // Sorted by name: build before test.
    assert_eq!(jobs[0].name, "build");
    assert_eq!(jobs[0].id.as_str(), "github:acme/widgets:job:100");
    assert_eq!(jobs[0].status, CiJobStatus::Completed);
    assert_eq!(
        jobs[0].conclusion,
        Some(temper_forge_model::CiJobConclusion::Success)
    );
    assert_eq!(jobs[0].commit_sha, "abc123");
    assert_eq!(
        jobs[0].url.as_deref(),
        Some("https://github.com/acme/widgets/runs/100")
    );
    assert_eq!(jobs[1].name, "test");
    assert_eq!(jobs[1].status, CiJobStatus::Running);
    assert_eq!(jobs[1].conclusion, None);

    let recorded = client.recorded();
    assert_eq!(recorded[0].path, "/repos/acme/widgets/actions/runs");
    assert!(
        recorded[0]
            .query
            .iter()
            .any(|(key, value)| key == "head_sha" && value == "abc123")
    );
    assert_eq!(recorded[1].path, "/repos/acme/widgets/actions/runs/12/jobs");
}

#[test]
fn list_ci_jobs_by_pull_request_resolves_head_sha_first() {
    let client = MockHttpClient::new();
    client.push_response(200, pull_json(5, "abc123"));
    client.push_response(200, runs_envelope(12, "abc123"));
    client.push_response(200, jobs_envelope());
    let forge = forge(client.clone());

    let jobs = block_on(forge.list_ci_jobs(
        &repo_id(),
        CiJobQuery {
            pull_request_id: Some(pull_id(5)),
            ..CiJobQuery::default()
        },
    ))
    .unwrap();

    assert_eq!(jobs.len(), 2);
    assert_eq!(jobs[0].pull_request_id, Some(pull_id(5)));

    let recorded = client.recorded();
    assert_eq!(recorded[0].path, "/repos/acme/widgets/pulls/5");
    assert!(
        recorded[1]
            .query
            .iter()
            .any(|(key, value)| key == "head_sha" && value == "abc123")
    );
}

#[test]
fn list_ci_jobs_for_missing_pull_request_is_empty() {
    let client = MockHttpClient::new();
    client.push_response(404, r#"{"message": "Not Found"}"#);
    let forge = forge(client.clone());

    let jobs = block_on(forge.list_ci_jobs(
        &repo_id(),
        CiJobQuery {
            pull_request_id: Some(pull_id(99)),
            ..CiJobQuery::default()
        },
    ))
    .unwrap();
    assert!(jobs.is_empty());
    // Only the pull-request lookup happened; no run scans.
    assert_eq!(client.call_count(), 1);
}

#[test]
fn list_ci_jobs_filters_by_status() {
    let client = MockHttpClient::new();
    client.push_response(200, runs_envelope(12, "abc123"));
    client.push_response(200, jobs_envelope());
    let forge = forge(client);

    let jobs = block_on(forge.list_ci_jobs(
        &repo_id(),
        CiJobQuery {
            commit_sha: Some("abc123".to_string()),
            status: Some(CiJobStatus::Completed),
            ..CiJobQuery::default()
        },
    ))
    .unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].name, "build");
}

#[test]
fn get_ci_job_reads_the_job_endpoint() {
    let client = MockHttpClient::new();
    client.push_response(
        200,
        r#"{
            "id": 100,
            "run_id": 12,
            "head_sha": "abc123",
            "name": "build",
            "status": "completed",
            "conclusion": "failure",
            "html_url": "https://github.com/acme/widgets/runs/100",
            "created_at": "2024-01-02T03:00:00Z",
            "started_at": "2024-01-02T03:01:00Z",
            "completed_at": "2024-01-02T03:05:00Z"
        }"#,
    );
    let forge = forge(client.clone());

    let id = CiJobId::new("github:acme/widgets:job:100");
    let job = block_on(forge.get_ci_job(&id)).unwrap().unwrap();
    assert_eq!(job.id, id);
    assert_eq!(job.repo_id, repo_id());
    assert_eq!(job.name, "build");
    assert_eq!(job.status, CiJobStatus::Completed);
    assert_eq!(
        job.conclusion,
        Some(temper_forge_model::CiJobConclusion::Failure)
    );
    assert!(job.pull_request_id.is_none());

    assert_eq!(
        client.recorded()[0].path,
        "/repos/acme/widgets/actions/jobs/100"
    );
}

#[test]
fn get_ci_job_maps_404_to_none() {
    let client = MockHttpClient::new();
    client.push_response(404, r#"{"message": "Not Found"}"#);
    let forge = forge(client);

    let job = block_on(forge.get_ci_job(&CiJobId::new("github:acme/widgets:job:999"))).unwrap();
    assert!(job.is_none());
}

#[test]
fn get_ci_job_rejects_foreign_id_shapes() {
    let client = MockHttpClient::new();
    let forge = forge(client.clone());

    let error = block_on(forge.get_ci_job(&CiJobId::new("forgejo:acme/widgets:actions:1:2:3")))
        .unwrap_err();
    assert!(matches!(
        error,
        temper_forge_model::ForgeError::InvalidRequest(_)
    ));
    assert_eq!(client.call_count(), 0);
}

#[test]
fn list_ci_jobs_uses_explicit_commit_over_pull_head() {
    let client = MockHttpClient::new();
    client.push_response(200, pull_json(5, "headsha"));
    client.push_response(200, runs_envelope(12, "explicit"));
    client.push_response(200, jobs_envelope());
    let forge = forge(client.clone());

    block_on(forge.list_ci_jobs(
        &repo_id(),
        CiJobQuery {
            pull_request_id: Some(pull_id(5)),
            commit_sha: Some("explicit".to_string()),
            ..CiJobQuery::default()
        },
    ))
    .unwrap();

    let recorded = client.recorded();
    assert!(
        recorded[1]
            .query
            .iter()
            .any(|(key, value)| key == "head_sha" && value == "explicit")
    );
    // The pull request is still resolved (number check) but its head SHA does
    // not override the explicit commit.
    let _ = ItemNumber::new(5);
}

#[test]
fn terminal_evidence_is_typed_bounded_and_attempt_identified() {
    let client = MockHttpClient::new();
    client.push_response(
        200,
        r#"{
            "total_count": 1,
            "workflow_runs": [{
                "id": 12,
                "run_attempt": 2,
                "head_sha": "abc123",
                "status": "completed"
            }]
        }"#,
    );
    client.push_response(
        200,
        serde_json::json!({
            "total_count": 1,
            "jobs": [{
                "id": 100,
                "run_id": 12,
                "head_sha": "abc123",
                "name": "build",
                "status": "completed",
                "conclusion": "action_required",
                "failure_reason": format!("approval required\r\n{}", "x".repeat(400)),
                "completed_at": "2024-01-02T03:05:00Z"
            }]
        })
        .to_string(),
    );

    let jobs = block_on(forge(client).list_ci_jobs(
        &repo_id(),
        CiJobQuery {
            commit_sha: Some("abc123".to_string()),
            ..CiJobQuery::default()
        },
    ))
    .unwrap();

    let job = &jobs[0];
    assert_eq!(
        job.conclusion,
        Some(temper_forge_model::CiJobConclusion::ActionRequired)
    );
    assert_eq!(job.provider_conclusion.as_deref(), Some("action_required"));
    assert!(
        job.provider_reason.as_ref().unwrap().len()
            <= temper_forge_model::MAX_CI_PROVIDER_EVIDENCE_BYTES
    );
    assert!(!job.provider_reason.as_ref().unwrap().contains('\n'));
    assert_eq!(job.run_id.as_deref(), Some("12"));
    assert_eq!(job.attempt.as_deref(), Some("2"));
}
