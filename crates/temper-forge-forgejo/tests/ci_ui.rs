// SPDX-License-Identifier: MPL-2.0
//! Offline tests for the Forgejo password/web-UI CI read path (ADR 0019).
//!
//! These drive `list_ci_jobs`/`get_ci_job` through the recording mock HTTP
//! client with canned login HTML, an Actions page, and live-view JSON, proving
//! the CSRF login handshake, cookie storage, re-login on a login bounce, the
//! live-view JSON → `CiJob` mapping, and the REST-first/UI-fallback decision. No
//! test touches the network.
mod support;

use serde_json::json;
use support::{MockHttpClient, block_on, forge, forge_with_web_ui, pull_id, repo_id};
use temper_forge_forgejo::{HttpMethod, HttpResponse};
use temper_forge_model::{CiJobConclusion, CiJobId, CiJobQuery, CiJobStatus};

/// A `200` HTML login page carrying a hidden `_csrf` input.
fn login_page() -> HttpResponse {
    HttpResponse {
        status: 200,
        headers: vec![(
            "Set-Cookie".to_string(),
            "_csrf=csrf-token; Path=/".to_string(),
        )],
        body: r#"<form method="post"><input type="hidden" name="_csrf" value="csrf-token"></form>"#
            .to_string(),
    }
}

/// A successful `POST /user/login`: redirect away from the login page, with the
/// session cookies set.
fn login_success() -> HttpResponse {
    HttpResponse {
        status: 302,
        headers: vec![
            ("Location".to_string(), "/".to_string()),
            (
                "Set-Cookie".to_string(),
                "i_like_gitea=session-abc; Path=/, gitea_incredible=more; Path=/".to_string(),
            ),
        ],
        body: String::new(),
    }
}

/// The repository Actions HTML page listing one run link.
fn actions_page(run: u64) -> HttpResponse {
    HttpResponse::new(
        200,
        format!(r#"<a href="/acme/widgets/actions/runs/{run}">run {run}</a>"#),
    )
}

/// Live-view JSON for a run with the given job statuses and commit short SHA.
fn live_view(status: &str, jobs: &[(&str, &str)], short_sha: &str) -> HttpResponse {
    let jobs: Vec<_> = jobs
        .iter()
        .enumerate()
        .map(|(id, (name, status))| json!({ "id": id + 1, "name": name, "status": status }))
        .collect();
    HttpResponse::new(
        200,
        json!({
            "state": {
                "run": {
                    "status": status,
                    "jobs": jobs,
                    "commit": { "shortSHA": short_sha, "branch": { "name": "main" } }
                }
            },
            "logs": {}
        })
        .to_string(),
    )
}

/// A PR-detail REST response (for head ref/sha resolution).
fn pr_detail(sha: &str) -> HttpResponse {
    HttpResponse::new(
        200,
        json!({
            "number": 7,
            "state": "open",
            "user": { "login": "author" },
            "head": { "ref": "feature", "sha": sha },
            "base": { "ref": "main" },
            "created_at": "2024-01-01T00:00:00Z",
            "updated_at": "2024-01-01T00:00:00Z"
        })
        .to_string(),
    )
}

#[test]
fn falls_back_to_web_ui_when_rest_returns_404() {
    let client = MockHttpClient::new();
    // 1) PR detail (head sha/ref). 2) REST runs 404 → fall back.
    client.push_result(Ok(pr_detail("c456eec18b00")));
    client.push_response(404, json!({ "message": "Not Found" }).to_string());
    // 3) login GET + 4) login POST.
    client.push_result(Ok(login_page()));
    client.push_result(Ok(login_success()));
    // 5) actions page (run 1). 6) live view for run 1.
    client.push_result(Ok(actions_page(1)));
    client.push_result(Ok(live_view(
        "failure",
        &[("build", "failure")],
        "c456eec18b",
    )));

    let forge = forge_with_web_ui(client.clone());
    let query = CiJobQuery {
        pull_request_id: Some(pull_id(7)),
        ..Default::default()
    };
    let jobs = block_on(forge.list_ci_jobs(&repo_id(), query)).unwrap();

    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].name, "build");
    assert_eq!(jobs[0].status, CiJobStatus::Completed);
    assert_eq!(jobs[0].conclusion, Some(CiJobConclusion::Failure));
    assert_eq!(jobs[0].pull_request_id, Some(pull_id(7)));
    assert_eq!(jobs[0].commit_sha, "c456eec18b");
    assert_eq!(jobs[0].id.as_str(), "forgejo:acme/widgets:actions:1:0:1");

    // The login handshake sent the CSRF token and form fields, and the live-view
    // POST carried the cookie jar plus the X-Csrf-Token header.
    let recorded = client.recorded();
    let login_post = recorded
        .iter()
        .find(|r| r.method == HttpMethod::Post && r.path == "/user/login")
        .expect("login POST recorded");
    let body = login_post.body.as_deref().unwrap();
    assert!(body.contains("user_name=ci-reader"));
    assert!(body.contains("_csrf=csrf-token"));
    assert!(body.contains("remember=on"));

    let live = recorded
        .iter()
        .find(|r| r.path == "/acme/widgets/actions/runs/1/jobs/0")
        .expect("live-view POST recorded");
    assert_eq!(live.method, HttpMethod::Post);
    assert_eq!(live.body.as_deref(), Some("{\"logCursors\":[]}"));
    let cookie = live
        .headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("cookie"))
        .map(|(_, v)| v.as_str())
        .unwrap_or_default();
    assert!(cookie.contains("i_like_gitea=session-abc"));
    assert!(cookie.contains("_csrf=csrf-token"));
    assert!(
        live.headers
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case("x-csrf-token") && v == "csrf-token")
    );
    // The web-UI path never sends the API prefix or the token header.
    assert!(!live.path.starts_with("/api/v1"));
    assert!(
        !live
            .headers
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("authorization"))
    );
}

#[test]
fn re_logs_in_on_login_bounce() {
    let client = MockHttpClient::new();
    client.push_result(Ok(pr_detail("aaaaaaaaaaaa")));
    client.push_response(404, "{}");
    // First login.
    client.push_result(Ok(login_page()));
    client.push_result(Ok(login_success()));
    // Actions page bounces to the login page (session expired) → re-login.
    client.push_result(Ok(HttpResponse {
        status: 302,
        headers: vec![("Location".to_string(), "/user/login".to_string())],
        body: String::new(),
    }));
    // Re-login handshake.
    client.push_result(Ok(login_page()));
    client.push_result(Ok(login_success()));
    // Retried actions page + live view.
    client.push_result(Ok(actions_page(2)));
    client.push_result(Ok(live_view("success", &[("build", "success")], "aaaaaaa")));

    let forge = forge_with_web_ui(client.clone());
    let query = CiJobQuery {
        pull_request_id: Some(pull_id(7)),
        ..Default::default()
    };
    let jobs = block_on(forge.list_ci_jobs(&repo_id(), query)).unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].conclusion, Some(CiJobConclusion::Success));

    // Two login POSTs were issued (initial + re-login after the bounce).
    let login_posts = client
        .recorded()
        .into_iter()
        .filter(|r| r.method == HttpMethod::Post && r.path == "/user/login")
        .count();
    assert_eq!(login_posts, 2);
}

#[test]
fn rest_first_keeps_actions_path_when_available() {
    // REST returns a matching run + task; the web UI is never consulted.
    let client = MockHttpClient::new();
    client.push_result(Ok(pr_detail("abcdef1234567")));
    client.push_response(
        200,
        json!({
            "workflow_runs": [{
                "index_in_repo": 10, "run_number": 10, "status": "success",
                "event": "pull_request", "prettyref": "#7", "head_branch": "feature",
                "head_sha": "abcdef1234567", "created_at": "2024-01-02T00:00:00Z"
            }]
        })
        .to_string(),
    );
    client.push_response(
        200,
        json!({ "workflow_runs": [
            { "id": 1, "run_number": 10, "name": "build", "status": "success" }
        ] })
        .to_string(),
    );

    let forge = forge_with_web_ui(client.clone());
    let query = CiJobQuery {
        pull_request_id: Some(pull_id(7)),
        ..Default::default()
    };
    let jobs = block_on(forge.list_ci_jobs(&repo_id(), query)).unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].id.as_str(), "forgejo:acme/widgets:actions:10:0:1");
    // Exactly three REST calls (PR detail, runs, tasks); no /user/login.
    assert_eq!(client.call_count(), 3);
    assert!(
        !client
            .recorded()
            .iter()
            .any(|r| r.path.contains("/user/login"))
    );
}

#[test]
fn no_web_ui_credentials_keeps_hard_error() {
    // REST 404 and no web-UI creds: a hard backend error, never a fake verdict.
    let client = MockHttpClient::new();
    client.push_result(Ok(pr_detail("abc1234567890")));
    client.push_response(404, "{}");

    let forge = forge(client); // no web-UI credentials
    let query = CiJobQuery {
        pull_request_id: Some(pull_id(7)),
        ..Default::default()
    };
    let result = block_on(forge.list_ci_jobs(&repo_id(), query));
    assert!(result.is_err());
}

#[test]
fn failed_login_surfaces_backend_error_without_password() {
    let client = MockHttpClient::new();
    client.push_result(Ok(pr_detail("abc1234567890")));
    client.push_response(404, "{}");
    client.push_result(Ok(login_page()));
    // A 200 on POST /user/login means the form re-rendered: bad credentials.
    client.push_response(200, "<form>login again</form>");

    let forge = forge_with_web_ui(client);
    let query = CiJobQuery {
        pull_request_id: Some(pull_id(7)),
        ..Default::default()
    };
    let error = block_on(forge.list_ci_jobs(&repo_id(), query)).unwrap_err();
    let message = error.to_string();
    assert!(message.contains("login failed"));
    assert!(!message.contains("s3cret"));
}

#[test]
fn get_ci_job_falls_back_to_web_ui() {
    let client = MockHttpClient::new();
    // REST runs 404 → fall back to the web UI.
    client.push_response(404, "{}");
    client.push_result(Ok(login_page()));
    client.push_result(Ok(login_success()));
    client.push_result(Ok(live_view(
        "failure",
        &[("build", "failure"), ("lint", "success")],
        "deadbeef",
    )));

    let forge = forge_with_web_ui(client);
    // Job index 1 of run 5 → the "lint" job.
    let id = CiJobId::new("forgejo:acme/widgets:actions:5:1:5");
    let job = block_on(forge.get_ci_job(&id)).unwrap().unwrap();
    assert_eq!(job.name, "lint");
    assert_eq!(job.status, CiJobStatus::Completed);
    assert_eq!(job.conclusion, Some(CiJobConclusion::Success));
    assert_eq!(job.id.as_str(), "forgejo:acme/widgets:actions:5:1:5");
}
