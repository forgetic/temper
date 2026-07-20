// SPDX-License-Identifier: MPL-2.0
//! Offline resilience tests for Forgejo web-UI live-view reads.
//!
//! Kept separate from `ci_ui.rs` so bounded recovery behavior and its
//! secret-safe diagnostics remain focused and easy to audit.
mod support;

use serde_json::json;
use support::{MockHttpClient, block_on, forge_with_web_ui, repo_id};
use temper_forge_forgejo::{HttpMethod, HttpRequest, HttpResponse};
use temper_forge_model::{CiJobConclusion, CiJobId, CiJobQuery};

const SHA: &str = "c456eec18b00";
const RESPONSE_SECRET: &str = "response-secret-must-not-escape";

fn login_page(csrf: &str) -> HttpResponse {
    HttpResponse {
        status: 200,
        headers: vec![("Set-Cookie".to_string(), format!("_csrf={csrf}; Path=/"))],
        body: format!(
            r#"<form method="post"><input type="hidden" name="_csrf" value="{csrf}"></form>"#
        ),
    }
}

fn login_success(session: &str) -> HttpResponse {
    HttpResponse {
        status: 302,
        headers: vec![
            ("Location".to_string(), "/".to_string()),
            (
                "Set-Cookie".to_string(),
                format!("i_like_gitea={session}; Path=/"),
            ),
        ],
        body: String::new(),
    }
}

fn actions_page(run: u64) -> HttpResponse {
    HttpResponse::new(
        200,
        format!(r#"<a href="/acme/widgets/actions/runs/{run}">run {run}</a>"#),
    )
}

fn successful_live_view() -> HttpResponse {
    HttpResponse::new(
        200,
        json!({
            "state": {
                "run": {
                    "status": "success",
                    "jobs": [{ "id": 1, "name": "build", "status": "success" }],
                    "commit": { "shortSHA": SHA, "branch": { "name": "main" } }
                }
            },
            "logs": {}
        })
        .to_string(),
    )
}

fn commit_query() -> CiJobQuery {
    CiJobQuery {
        commit_sha: Some(SHA.to_string()),
        ..Default::default()
    }
}

fn header<'a>(request: &'a HttpRequest, name: &str) -> Option<&'a str> {
    request
        .headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

fn live_view_requests(requests: &[HttpRequest], run: u64) -> Vec<&HttpRequest> {
    let path = format!("/acme/widgets/actions/runs/{run}/jobs/0/attempt/1");
    requests
        .iter()
        .filter(|request| request.method == HttpMethod::Post && request.path == path)
        .collect()
}

fn assert_two_login_handshakes(requests: &[HttpRequest]) {
    let login_gets = requests
        .iter()
        .filter(|request| request.method == HttpMethod::Get && request.path == "/user/login")
        .count();
    let login_posts = requests
        .iter()
        .filter(|request| request.method == HttpMethod::Post && request.path == "/user/login")
        .count();
    assert_eq!((login_gets, login_posts), (2, 2));
}

#[test]
fn live_view_500_reauthenticates_once_and_refreshes_session_headers() {
    let client = MockHttpClient::new();
    client.push_response(404, "{}"); // REST Actions unavailable.
    client.push_result(Ok(login_page("csrf-old")));
    client.push_result(Ok(login_success("session-old")));
    client.push_result(Ok(actions_page(3)));
    client.push_response(500, RESPONSE_SECRET);
    client.push_result(Ok(login_page("csrf-new")));
    client.push_result(Ok(login_success("session-new")));
    client.push_result(Ok(successful_live_view()));

    let jobs = block_on(forge_with_web_ui(client.clone()).list_ci_jobs(&repo_id(), commit_query()))
        .unwrap();

    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].conclusion, Some(CiJobConclusion::Success));

    let recorded = client.recorded();
    assert_two_login_handshakes(&recorded);
    let live = live_view_requests(&recorded, 3);
    assert_eq!(live.len(), 2, "a 500 receives exactly one retry");
    assert_eq!(header(live[0], "x-csrf-token"), Some("csrf-old"));
    assert_eq!(header(live[1], "x-csrf-token"), Some("csrf-new"));
    assert!(header(live[0], "cookie").unwrap().contains("session-old"));
    let refreshed_cookie = header(live[1], "cookie").unwrap();
    assert!(refreshed_cookie.contains("session-new"));
    assert!(refreshed_cookie.contains("_csrf=csrf-new"));
    assert!(!refreshed_cookie.contains("session-old"));
    assert!(!refreshed_cookie.contains("csrf-old"));
}

#[test]
fn persistent_live_view_500_stops_after_one_retry() {
    let client = MockHttpClient::new();
    client.push_response(404, "{}");
    client.push_result(Ok(login_page("csrf-old")));
    client.push_result(Ok(login_success("session-old")));
    client.push_result(Ok(actions_page(9)));
    client.push_response(500, "first server failure");
    client.push_result(Ok(login_page("csrf-new")));
    client.push_result(Ok(login_success("session-new")));
    client.push_response(500, "persistent server failure");

    let error =
        block_on(forge_with_web_ui(client.clone()).list_ci_jobs(&repo_id(), commit_query()))
            .unwrap_err();

    assert!(error.to_string().contains("final HTTP status 500"));
    let recorded = client.recorded();
    assert_two_login_handshakes(&recorded);
    assert_eq!(live_view_requests(&recorded, 9).len(), 2);
}

#[test]
fn exact_job_persistent_500_reports_only_safe_coordinates_and_status() {
    let client = MockHttpClient::new();
    client.push_response(404, "{}"); // Exact REST run lookup unavailable.
    client.push_result(Ok(login_page("csrf-secret-old")));
    client.push_result(Ok(login_success("session-secret-old")));
    client.push_response(500, RESPONSE_SECRET);
    client.push_result(Ok(login_page("csrf-secret-new")));
    client.push_result(Ok(login_success("session-secret-new")));
    client.push_response(500, format!("{RESPONSE_SECRET}-again"));

    let id = CiJobId::new("forgejo:acme/widgets:actions:5:0:1");
    let error = block_on(forge_with_web_ui(client.clone()).get_ci_job(&id)).unwrap_err();
    let message = error.to_string();

    assert_eq!(
        message,
        "backend error: forgejo web-ui: unreadable live view for repository acme/widgets, run 5, \
         job 0: final HTTP status 500, retry count 1"
    );
    for secret in [
        RESPONSE_SECRET,
        "s3cret",
        "csrf-secret-old",
        "csrf-secret-new",
        "session-secret-old",
        "session-secret-new",
    ] {
        assert!(!message.contains(secret), "error leaked {secret}");
    }

    let recorded = client.recorded();
    assert_two_login_handshakes(&recorded);
    assert_eq!(live_view_requests(&recorded, 5).len(), 2);
}
