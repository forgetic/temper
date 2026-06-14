//! Phase 1b smoke test: a real host-mode `forgejo-runner` runs a real job.
//!
//! `#[ignore]`d, so the default `cargo test` never downloads a binary, opens a
//! socket, or spawns a runner. No extra environment variable is required; run it
//! with:
//!
//! ```sh
//! cargo test -p temper-testing --test forgejo_runner -- --ignored
//! ```
//!
//! It boots a [`ForgejoServer`] + [`ForgejoRunner`] (host mode, no containers),
//! provisions a minimal repo whose `.forgejo/workflows/ci.yml` deliberately
//! **fails** (`run: exit 1`), then polls the head commit's status API until the
//! real runner reports `state: "failure"`. That a real verdict appears confirms
//! the runner picked up and executed the job on this host. (Reading CI via the
//! web-UI live-view JSON is Phase 3b; commit status is the cheap confirmation.)
//!
//! Provisioning here is raw HTTP via an admin token created with the server CLI;
//! full role/identity provisioning is Phase 2.

use serde_json::{Value, json};
use std::time::{Duration, Instant};
use temper_testing::forgejo_server::{ForgejoRunner, ForgejoServer, ForgejoState};

const ADMIN_USER: &str = "temperadmin";
const ADMIN_PASSWORD: &str = "Sup3rSecret-Phase1b!";
const ADMIN_EMAIL: &str = "temperadmin@example.invalid";
const REPO: &str = "ci-smoke";

/// A failing host-mode workflow: `runs-on: host` so the `host:host` runner
/// claims it, and `exit 1` so the verdict is unambiguously a failure.
const FAILING_WORKFLOW: &str = "name: ci\n\
on: [push]\n\
jobs:\n\
\u{20}\u{20}build:\n\
\u{20}\u{20}\u{20}\u{20}runs-on: host\n\
\u{20}\u{20}\u{20}\u{20}steps:\n\
\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}- run: exit 1\n";

#[derive(serde::Deserialize, serde::Serialize)]
struct RunnerSmokeMetadata {
    admin_token: String,
}

#[test]
#[ignore = "boots a real Forgejo + host-mode runner; run with --ignored"]
fn runner_runs_failing_job_and_reports_failure() {
    let state = ForgejoState::new(json!({
        "kind": "runner-smoke",
        "version": 1,
        "admin": ADMIN_USER,
        "repo": REPO,
    }))
    .expect("runner smoke state serializes");
    let cached = ForgejoServer::start_with_state(&state, |server| {
        let base = server.base_url().to_string();
        let token = create_admin_token(server);
        let client = temper_io_engine::http::BlockingJsonClient::new();
        create_repo(&client, &base, &token);
        Ok::<RunnerSmokeMetadata, String>(RunnerSmokeMetadata { admin_token: token })
    })
    .expect("forgejo runner smoke state starts");
    let server = cached.server;
    let token = cached.metadata.admin_token;
    let base = server.base_url().to_string();
    let client = temper_io_engine::http::BlockingJsonClient::new();

    // Register the host-mode runner *before* pushing the workflow so it is ready
    // to claim the job the moment Actions sees it.
    let mut runner = ForgejoRunner::register(&server).expect("runner registers");
    assert!(runner.is_running(), "runner daemon exited immediately");

    // The cached state already has the admin and initialized repo. Commit the
    // failing workflow into this per-test copy and enable Actions.
    let head_sha = put_workflow_file(&client, &base, &token);
    enable_repo_actions(&client, &base, &token);

    // Wait (generously) for the runner to pick up and fail the job, observed via
    // the head commit's status.
    let state = wait_for_commit_state(&client, &base, &token, &head_sha);
    assert_eq!(
        state,
        "failure",
        "expected the real runner to report a failing commit status; \
         runner running={}, log: {}",
        runner.is_running(),
        runner.log_tail()
    );

    // Tear down explicitly so any panic in drop surfaces here, not at unwind.
    drop(runner);
    drop(server);
}

/// Creates an admin user, then mints a fully-scoped access token for it via the
/// server CLI and returns the raw token.
///
/// Two steps because `admin user create --access-token` yields a **scopeless**
/// token on Forgejo 7.0.x (REST calls 403 with "token does not have ... scope");
/// `generate-access-token --scopes all --raw` mints a usable one.
fn create_admin_token(server: &ForgejoServer) -> String {
    server
        .run_cli(&[
            "admin",
            "user",
            "create",
            "--username",
            ADMIN_USER,
            "--password",
            ADMIN_PASSWORD,
            "--email",
            ADMIN_EMAIL,
            "--admin",
            "--must-change-password=false",
        ])
        .expect("admin user create succeeds");
    let token = server
        .run_cli(&[
            "admin",
            "user",
            "generate-access-token",
            "--username",
            ADMIN_USER,
            "--scopes",
            "all",
            "--raw",
        ])
        .expect("generate-access-token succeeds");
    let token = token.trim().to_string();
    assert!(!token.is_empty(), "empty token from generate-access-token");
    token
}

fn create_repo(client: &temper_io_engine::http::BlockingJsonClient, base: &str, token: &str) {
    let (status, body) = client.send_expect_json(
        "POST",
        format!("{base}/api/v1/user/repos"),
        Some(token),
        Some(&json!({
            "name": REPO,
            "auto_init": true,
            "default_branch": "main",
            "private": false,
        })),
        "create repo",
    );
    assert!(
        (200..300).contains(&status),
        "create repo failed: {status} {body}"
    );
}

/// Commits the failing workflow file and returns the resulting commit SHA (the
/// head the runner will report a status against).
fn put_workflow_file(
    client: &temper_io_engine::http::BlockingJsonClient,
    base: &str,
    token: &str,
) -> String {
    use base64::Engine;
    let content = base64::engine::general_purpose::STANDARD.encode(FAILING_WORKFLOW);
    let (status, body) = client.send_expect_json(
        "POST",
        format!("{base}/api/v1/repos/{ADMIN_USER}/{REPO}/contents/.forgejo/workflows/ci.yml"),
        Some(token),
        Some(&json!({
            "content": content,
            "message": "add failing CI workflow",
            "branch": "main",
        })),
        "put workflow",
    );
    assert!(
        (200..300).contains(&status),
        "put workflow failed: {status} {body}"
    );
    body["commit"]["sha"]
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| panic!("no commit sha in contents response: {body}"))
}

fn enable_repo_actions(
    client: &temper_io_engine::http::BlockingJsonClient,
    base: &str,
    token: &str,
) {
    let (status, body) = client.send_expect_json(
        "PATCH",
        format!("{base}/api/v1/repos/{ADMIN_USER}/{REPO}"),
        Some(token),
        Some(&json!({ "has_actions": true })),
        "enable actions",
    );
    assert!(
        (200..300).contains(&status),
        "enable actions failed: {status} {body}"
    );
}

/// Polls the commit-status API until a terminal state appears or a generous
/// deadline passes. Returns the observed `state` (e.g. `failure`, `success`).
fn wait_for_commit_state(
    client: &temper_io_engine::http::BlockingJsonClient,
    base: &str,
    token: &str,
    sha: &str,
) -> String {
    // A real host CI job takes seconds; allow plenty of slack for cold start.
    let deadline = Instant::now() + Duration::from_secs(120);
    let url = format!("{base}/api/v1/repos/{ADMIN_USER}/{REPO}/commits/{sha}/status");
    let mut last = String::from("(none)");
    loop {
        if let Ok(resp) = client.send("GET", url.as_str(), Some(token), None)
            && let Ok(body) = serde_json::from_slice::<Value>(&resp.body)
        {
            let state = body["state"].as_str().unwrap_or("").to_string();
            last = state.clone();
            // `pending`/`running`/empty mean "not done yet"; keep polling.
            if matches!(state.as_str(), "success" | "failure" | "error") {
                return state;
            }
        }
        if Instant::now() >= deadline {
            return format!("timeout (last state: {last})");
        }
        std::thread::sleep(Duration::from_secs(2));
    }
}
