// SPDX-License-Identifier: MPL-2.0
//! Offline resilience tests for Forgejo web-UI live-view reads.
//!
//! Kept separate from `ci_ui.rs` so bounded recovery behavior and its
//! secret-safe diagnostics remain focused and easy to audit.
mod support;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};

use serde_json::json;
use support::{MockHttpClient, block_on, forge_with_web_ui, repo_id};
use temper_forge_forgejo::{HttpMethod, HttpRequest, HttpResponse};
use temper_forge_model::{CiJobConclusion, CiJobId, CiJobQuery};
use tracing::Level;
use tracing::field::{Field, Visit};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::registry;

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

fn actions_page(runs: &[u64]) -> HttpResponse {
    let body = runs
        .iter()
        .map(|run| format!(r#"<a href="/acme/widgets/actions/runs/{run}">run {run}</a>"#))
        .collect::<Vec<_>>()
        .join("\n");
    HttpResponse::new(200, body)
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

fn queue_login(client: &MockHttpClient, suffix: &str) {
    client.push_result(Ok(login_page(&format!("csrf-{suffix}"))));
    client.push_result(Ok(login_success(&format!("session-{suffix}"))));
}

fn queue_persistent_500(client: &MockHttpClient, suffix: &str) {
    client.push_response(500, format!("{RESPONSE_SECRET}-{suffix}-first"));
    queue_login(client, suffix);
    client.push_response(500, format!("{RESPONSE_SECRET}-{suffix}-second"));
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

#[derive(Clone, Debug)]
struct CapturedEvent {
    level: Level,
    target: String,
    fields: BTreeMap<String, String>,
}

impl CapturedEvent {
    fn field(&self, name: &str) -> Option<&str> {
        self.fields.get(name).map(|value| value.trim_matches('"'))
    }
}

#[derive(Default)]
struct CapturedVisitor {
    fields: BTreeMap<String, String>,
}

impl Visit for CapturedVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.fields
            .insert(field.name().to_string(), format!("{value:?}"));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.fields
            .insert(field.name().to_string(), value.to_string());
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.fields
            .insert(field.name().to_string(), value.to_string());
    }
}

#[derive(Clone, Default)]
struct CaptureLayer {
    events: Arc<Mutex<Vec<CapturedEvent>>>,
}

impl<S: tracing::Subscriber> Layer<S> for CaptureLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _context: Context<'_, S>) {
        let mut visitor = CapturedVisitor::default();
        event.record(&mut visitor);
        self.events.lock().unwrap().push(CapturedEvent {
            level: *event.metadata().level(),
            target: event.metadata().target().to_string(),
            fields: visitor.fields,
        });
    }
}

/// Installs one process-wide capture layer before any test invokes the shared
/// warning callsite. A thread-local subscriber can race other parallel tests'
/// disabled dispatch and leave that callsite temporarily uninterested.
fn event_store() -> &'static Arc<Mutex<Vec<CapturedEvent>>> {
    static EVENTS: OnceLock<Arc<Mutex<Vec<CapturedEvent>>>> = OnceLock::new();
    EVENTS.get_or_init(|| {
        let events = Arc::new(Mutex::new(Vec::new()));
        tracing::subscriber::set_global_default(registry().with(CaptureLayer {
            events: Arc::clone(&events),
        }))
        .expect("install Forgejo CI warning capture subscriber");
        events
    })
}

#[test]
fn live_view_500_reauthenticates_once_and_refreshes_session_headers() {
    event_store();
    let client = MockHttpClient::new();
    client.push_response(404, "{}"); // REST Actions unavailable.
    queue_login(&client, "old");
    client.push_result(Ok(actions_page(&[3])));
    client.push_response(500, RESPONSE_SECRET);
    queue_login(&client, "new");
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
fn newer_success_survives_an_older_unreadable_run() {
    event_store();
    let client = MockHttpClient::new();
    client.push_response(404, "{}");
    queue_login(&client, "initial");
    client.push_result(Ok(actions_page(&[10, 9])));
    client.push_result(Ok(successful_live_view()));
    queue_persistent_500(&client, "run-9-retry");

    let jobs = block_on(forge_with_web_ui(client.clone()).list_ci_jobs(&repo_id(), commit_query()))
        .unwrap();

    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].conclusion, Some(CiJobConclusion::Success));
    assert_eq!(jobs[0].id.as_str(), "forgejo:acme/widgets:actions:10:0:10");
    let requests = client.recorded();
    assert_eq!(live_view_requests(&requests, 10).len(), 1);
    assert_eq!(live_view_requests(&requests, 9).len(), 2);
}

#[test]
fn newer_unreadable_run_suppresses_an_older_success() {
    event_store();
    let client = MockHttpClient::new();
    client.push_response(404, "{}");
    queue_login(&client, "initial");
    client.push_result(Ok(actions_page(&[10, 9])));
    queue_persistent_500(&client, "run-10-retry");
    client.push_result(Ok(successful_live_view()));

    let jobs = block_on(forge_with_web_ui(client.clone()).list_ci_jobs(&repo_id(), commit_query()))
        .unwrap();

    assert!(
        jobs.is_empty(),
        "older green evidence must leave the gate pending"
    );
    let requests = client.recorded();
    assert_eq!(live_view_requests(&requests, 10).len(), 2);
    assert_eq!(
        live_view_requests(&requests, 9).len(),
        1,
        "the scan continues after the unreadable run"
    );
}

#[test]
fn all_unreadable_runs_remain_empty_and_are_fetched_again() {
    event_store();
    let client = MockHttpClient::new();
    for read in ["first", "second"] {
        client.push_response(404, "{}");
        queue_login(&client, &format!("{read}-initial"));
        client.push_result(Ok(actions_page(&[12, 11])));
        queue_persistent_500(&client, &format!("{read}-run-12"));
        queue_persistent_500(&client, &format!("{read}-run-11"));
    }

    let forge = forge_with_web_ui(client.clone());
    let first = block_on(forge.list_ci_jobs(&repo_id(), commit_query())).unwrap();
    let second = block_on(forge.list_ci_jobs(&repo_id(), commit_query())).unwrap();

    assert!(first.is_empty());
    assert!(second.is_empty());
    let requests = client.recorded();
    assert_eq!(
        requests
            .iter()
            .filter(|request| {
                request.method == HttpMethod::Get && request.path == "/acme/widgets/actions"
            })
            .count(),
        2,
        "an empty degraded read must not become a terminal cache hit"
    );
    assert_eq!(live_view_requests(&requests, 12).len(), 4);
    assert_eq!(live_view_requests(&requests, 11).len(), 4);
}

#[test]
fn degraded_list_warning_is_single_bounded_structured_and_secret_free() {
    let events = event_store();
    let client = MockHttpClient::new();
    client.push_response(404, "{}");
    queue_login(&client, "initial-secret");
    client.push_result(Ok(actions_page(&[10, 9, 8])));
    client.push_result(Ok(successful_live_view()));
    queue_persistent_500(&client, "csrf-session-secret-run-9");
    queue_persistent_500(&client, "csrf-session-secret-run-8");

    let jobs =
        block_on(forge_with_web_ui(client).list_ci_jobs(&repo_id(), commit_query())).unwrap();
    assert_eq!(jobs.len(), 1);

    let captured = events.lock().unwrap();
    let warnings = captured
        .iter()
        .filter(|event| {
            event.level == Level::WARN
                && event.target == "temper_forge_forgejo"
                && event.field("run") == Some("9")
                && event.field("unreadable_count") == Some("2")
                && event.field("outcome") == Some("continued")
        })
        .collect::<Vec<_>>();
    assert_eq!(
        warnings.len(),
        1,
        "one warning summarizes the degraded read: {captured:#?}"
    );
    let warning = warnings[0];
    assert_eq!(warning.field("repository"), Some("acme/widgets"));
    assert_eq!(warning.field("run"), Some("9"));
    assert_eq!(warning.field("job"), Some("0"));
    assert_eq!(warning.field("status"), Some("500"));
    assert_eq!(warning.field("retry_count"), Some("1"));
    assert_eq!(warning.field("unreadable_count"), Some("2"));
    assert_eq!(warning.field("omitted_count"), Some("1"));
    assert_eq!(warning.field("outcome"), Some("continued"));
    assert_eq!(
        warning.field("message"),
        Some(
            "forgejo web-ui degraded CI list read: repository acme/widgets, representative run \
             9, job 0, status 500, retry count 1, unreadable count 2, omitted count 1, outcome \
             continued"
        )
    );

    let rendered = format!("{warning:?}");
    for secret in [
        RESPONSE_SECRET,
        "s3cret",
        "csrf-initial-secret",
        "session-initial-secret",
        "csrf-session-secret-run-9",
        "csrf-session-secret-run-8",
    ] {
        assert!(!rendered.contains(secret), "warning leaked {secret}");
    }
}

#[test]
fn exact_job_persistent_500_reports_only_safe_coordinates_and_status() {
    event_store();
    let client = MockHttpClient::new();
    client.push_response(404, "{}"); // Exact REST run lookup unavailable.
    queue_login(&client, "secret-old");
    client.push_response(500, RESPONSE_SECRET);
    queue_login(&client, "secret-new");
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
