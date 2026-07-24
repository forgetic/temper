//! Terminal-evidence contracts for Forgejo REST and password/web-UI CI reads.

mod support;

use serde_json::json;
use support::{MockHttpClient, block_on, forge, forge_with_web_ui, pull_id, repo_id};
use temper_forge_forgejo::HttpResponse;
use temper_forge_model::{CiJobConclusion, CiJobQuery, CiJobStatus};

fn login_page() -> HttpResponse {
    HttpResponse {
        status: 200,
        headers: vec![(
            "Set-Cookie".to_string(),
            "_csrf=csrf-token; Path=/".to_string(),
        )],
        body: r#"<form><input name="_csrf" value="csrf-token"></form>"#.to_string(),
    }
}

fn login_success() -> HttpResponse {
    HttpResponse {
        status: 302,
        headers: vec![
            ("Location".to_string(), "/".to_string()),
            (
                "Set-Cookie".to_string(),
                "i_like_gitea=session-abc; Path=/".to_string(),
            ),
        ],
        body: String::new(),
    }
}

#[test]
fn run_591_rest_bare_failure_is_ambiguous_terminalization() {
    let client = MockHttpClient::new();
    client.push_response(
        200,
        json!({
            "workflow_runs": [{
                "id": 591,
                "index_in_repo": 591,
                "run_number": 591,
                "status": "failure",
                "event": "push",
                "head_sha": "c456eec18b00",
                "created_at": "2026-07-23T15:23:00Z",
                "updated_at": "2026-07-23T15:38:32Z"
            }]
        })
        .to_string(),
    );
    client.push_response(200, include_str!("fixtures/run-591-rest-tasks.json"));

    let jobs = block_on(forge(client).list_ci_jobs(
        &repo_id(),
        CiJobQuery {
            commit_sha: Some("c456eec18b00".to_string()),
            ..CiJobQuery::default()
        },
    ))
    .unwrap();

    assert_eq!(jobs.len(), 1);
    let job = &jobs[0];
    assert_eq!(job.id.as_str(), "forgejo:acme/widgets:actions:591:0:3385");
    assert_eq!(job.status, CiJobStatus::Completed);
    assert_eq!(job.conclusion, Some(CiJobConclusion::Unknown));
    assert_eq!(job.provider_conclusion.as_deref(), Some("failure"));
    assert_eq!(job.provider_reason, None);
    assert!(temper_workflow::CiStatus::from_jobs(&jobs).is_recovery_required());
}

#[test]
fn web_ui_run_591_bare_failure_is_ambiguous_terminalization() {
    let client = MockHttpClient::new();
    client.push_response(
        200,
        json!({
            "number": 7,
            "state": "open",
            "user": { "login": "author" },
            "head": { "ref": "feature", "sha": "c456eec18b00" },
            "base": { "ref": "main" },
            "created_at": "2026-07-23T15:00:00Z",
            "updated_at": "2026-07-23T15:38:32Z"
        })
        .to_string(),
    );
    client.push_response(404, json!({ "message": "Not Found" }).to_string());
    client.push_result(Ok(login_page()));
    client.push_result(Ok(login_success()));
    client.push_response(
        200,
        r#"<a href="/acme/widgets/actions/runs/591">run 591</a>"#,
    );
    client.push_response(200, include_str!("fixtures/run-591-web-ui.json"));

    let jobs = block_on(forge_with_web_ui(client).list_ci_jobs(
        &repo_id(),
        CiJobQuery {
            pull_request_id: Some(pull_id(7)),
            ..Default::default()
        },
    ))
    .unwrap();

    assert_eq!(jobs.len(), 1);
    let job = &jobs[0];
    assert_eq!(job.status, CiJobStatus::Completed);
    assert_eq!(job.conclusion, Some(CiJobConclusion::Unknown));
    assert_eq!(job.provider_conclusion.as_deref(), Some("failure"));
    assert_eq!(job.provider_reason, None);
    assert_eq!(job.run_id.as_deref(), Some("591"));
    assert!(temper_workflow::CiStatus::from_jobs(&jobs).is_recovery_required());
}

#[test]
fn web_ui_preserves_terminal_evidence_attempt_and_cancellation() {
    let client = MockHttpClient::new();
    client.push_response(
        200,
        json!({
            "number": 7,
            "state": "open",
            "user": { "login": "author" },
            "head": { "ref": "feature", "sha": "c456eec18b00" },
            "base": { "ref": "main" },
            "created_at": "2024-01-01T00:00:00Z",
            "updated_at": "2024-01-01T00:00:00Z"
        })
        .to_string(),
    );
    client.push_response(404, json!({ "message": "Not Found" }).to_string());
    client.push_result(Ok(login_page()));
    client.push_result(Ok(login_success()));
    client.push_response(200, r#"<a href="/acme/widgets/actions/runs/44">run 44</a>"#);
    client.push_response(
        200,
        json!({
            "state": {
                "run": {
                    "status": "completed",
                    "conclusion": "RUNNER_LOST",
                    "failureReason": format!("runner disconnected\n{}", "x".repeat(400)),
                    "runAttempt": 3,
                    "jobs": [
                        { "name": "build", "status": "completed" },
                        { "name": "cleanup", "status": "cancelled" }
                    ],
                    "commit": {
                        "shortSHA": "c456eec18b",
                        "branch": { "name": "feature" }
                    }
                }
            },
            "logs": {}
        })
        .to_string(),
    );

    let jobs = block_on(forge_with_web_ui(client).list_ci_jobs(
        &repo_id(),
        CiJobQuery {
            pull_request_id: Some(pull_id(7)),
            ..Default::default()
        },
    ))
    .unwrap();

    assert_eq!(jobs.len(), 2);
    let build = &jobs[0];
    assert_eq!(build.name, "build");
    assert_eq!(build.status, CiJobStatus::Completed);
    assert_eq!(build.conclusion, Some(CiJobConclusion::RunnerLost));
    assert_eq!(build.provider_conclusion.as_deref(), Some("RUNNER_LOST"));
    assert!(
        build.provider_reason.as_ref().unwrap().len()
            <= temper_forge_model::MAX_CI_PROVIDER_EVIDENCE_BYTES
    );
    assert!(!build.provider_reason.as_ref().unwrap().contains('\n'));
    assert_eq!(build.run_id.as_deref(), Some("44"));
    assert_eq!(build.attempt.as_deref(), Some("3"));

    assert_eq!(jobs[1].name, "cleanup");
    assert_eq!(jobs[1].conclusion, Some(CiJobConclusion::Cancelled));
}
