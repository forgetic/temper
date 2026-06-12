//! Ignored live smoke suite against a throwaway local Forgejo.
//!
//! A plain `cargo test` stays hermetic because this file is `#[ignore]`d. When
//! run with `--ignored`, the suite boots a local Forgejo plus a host-mode
//! `forgejo-runner`, provisions a fresh repository, exercises the Forgejo backend
//! against it, and tears the processes down on drop. No credentials or opt-in
//! environment variables are required; missing cached binaries are downloaded
//! from the pinned upstream release assets and checksum-verified on first use.
//!
//! ```sh
//! cargo test -p temper-forge-forgejo --test live -- --ignored
//! ```

use base64::Engine;
use bench_forgejo::{ForgejoRunner, ForgejoServer, ForgejoState, ServerError};
use serde_json::{json, Value};
use std::time::{Duration, Instant};
use temper_forge::{
    CiJobConclusion, CiJobQuery, CiJobStatus, CreateIssue, IssueQuery, IssueState,
    PullRequestQuery, RepositoryId, RepositoryPath, UpdateIssue,
};
use temper_forge_forgejo::{EngineHttpClient, ForgejoConfig, ForgejoForge};
use temper_io_engine::http::{http_call, HttpCall, HttpResponseData};

const ADMIN_USER: &str = "liveadmin";
const ADMIN_PASSWORD: &str = "L1ve-Smoke-Admin!";
const ADMIN_EMAIL: &str = "liveadmin@example.invalid";
const REPO: &str = "forgejo-live-smoke";

const CI_WORKFLOW: &str = r#"name: ci
on: [push]
jobs:
  build:
    runs-on: host
    steps:
      - run: echo temper forgejo live smoke
"#;

#[derive(serde::Deserialize, serde::Serialize)]
struct LiveMetadata {
    admin_token: String,
}

struct LiveWorld {
    _server: ForgejoServer,
    _runner: ForgejoRunner,
    forge: ForgejoForge<EngineHttpClient>,
    repo_id: RepositoryId,
    repo_path: RepositoryPath,
}

#[test]
#[ignore = "boots local Forgejo + host-mode forgejo-runner; run with --ignored"]
fn live_smoke_suite_against_throwaway_forgejo() {
    temper_io_engine::block_on_with(move |cx, _handle| async move {
        let world = boot_world().await;

        let user = world
            .forge
            .current_user()
            .await
            .expect("current_user should succeed");
        assert_eq!(user.handle, ADMIN_USER);

        let repo = world
            .forge
            .get_repository_by_path(&world.repo_path)
            .await
            .expect("get_repository_by_path should succeed")
            .expect("provisioned repository should exist");
        assert_eq!(repo.id, world.repo_id);
        assert_eq!(repo.owner, world.repo_path.owner);
        assert_eq!(repo.name, world.repo_path.name);
        assert_eq!(repo.default_branch, "main");

        let labels = world
            .forge
            .list_labels(&world.repo_id)
            .await
            .expect("list_labels should succeed");
        let mut names: Vec<&str> = labels.iter().map(|label| label.name.as_str()).collect();
        let sorted = {
            let mut copy = names.clone();
            copy.sort_unstable();
            copy
        };
        names.sort_unstable();
        assert_eq!(names, sorted, "labels should come back name-sorted");

        let issues = world
            .forge
            .list_issues(&world.repo_id, IssueQuery::default())
            .await
            .expect("list_issues should succeed");
        assert!(issues.iter().all(|issue| issue.repo_id == world.repo_id));

        let pulls = world
            .forge
            .list_pull_requests(&world.repo_id, PullRequestQuery::default())
            .await
            .expect("list_pull_requests should succeed");
        assert!(pulls.iter().all(|pull| pull.repo_id == world.repo_id));

        create_and_close_issue(&world).await;
        wait_for_ci_success(&cx, &world).await;
    });
}

async fn boot_world() -> LiveWorld {
    let state = ForgejoState::new(json!({
        "kind": "forgejo-backend-live-smoke",
        "version": 1,
        "admin": ADMIN_USER,
        "repo": REPO,
    }))
    .expect("live smoke state serializes");
    let cached = skein::runtime::spawn_blocking(move || {
        ForgejoServer::start_with_state(&state, |server| {
            let base = server.base_url().to_string();
            let admin_token = bootstrap_admin(server).expect("admin token bootstraps");
            // One-shot bootstrap on this blocking thread: build a fresh engine
            // runtime, perform the provisioning calls, tear it down.
            temper_io_engine::block_on(async move {
                create_initialized_repo(&base, &admin_token).await;
                Ok::<LiveMetadata, String>(LiveMetadata { admin_token })
            })
        })
    })
    .await
    .expect("cached Forgejo state starts");
    let server = cached.server;
    let base = server.base_url().to_string();
    let admin_token = cached.metadata.admin_token;

    let mut runner = ForgejoRunner::register(&server).expect("forgejo-runner registers");
    assert!(runner.is_running(), "runner daemon exited immediately");

    enable_repo_actions(&base, &admin_token).await;
    put_workflow_file(&base, &admin_token).await;

    let repo_path = RepositoryPath::new(ADMIN_USER, REPO);
    let forge = ForgejoForge::new(
        ForgejoConfig::new(&base, &admin_token)
            .with_default_repo(ADMIN_USER, REPO)
            .with_web_ui_credentials(ADMIN_USER, ADMIN_PASSWORD),
    );
    let repo_id = forge
        .get_repository_by_path(&repo_path)
        .await
        .expect("repository lookup should succeed")
        .expect("provisioned repository exists")
        .id;

    LiveWorld {
        _server: server,
        _runner: runner,
        forge,
        repo_id,
        repo_path,
    }
}

fn bootstrap_admin(server: &ForgejoServer) -> Result<String, ServerError> {
    server.run_cli(&[
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
    ])?;
    let token = server.run_cli(&[
        "admin",
        "user",
        "generate-access-token",
        "--username",
        ADMIN_USER,
        "--scopes",
        "all",
        "--raw",
    ])?;
    Ok(token.trim().to_string())
}

/// Send one authorized JSON request through the engine HTTP client.
async fn api_json(method: &str, url: String, token: &str, body: &Value) -> HttpResponseData {
    let client = temper_io_engine::http::build_http_client();
    http_call(
        &client,
        HttpCall {
            method: method.to_string(),
            url,
            headers: vec![
                ("Authorization".to_string(), format!("token {token}")),
                ("Content-Type".to_string(), "application/json".to_string()),
            ],
            body: body.to_string().into_bytes(),
        },
    )
    .await
    .expect("api request sends")
}

async fn create_initialized_repo(base: &str, token: &str) {
    let response = api_json(
        "POST",
        format!("{base}/api/v1/user/repos"),
        token,
        &json!({
            "name": REPO,
            "auto_init": true,
            "default_branch": "main",
            "private": false,
        }),
    )
    .await;
    assert_success(&response, "create repo");
}

async fn enable_repo_actions(base: &str, token: &str) {
    let response = api_json(
        "PATCH",
        format!("{base}/api/v1/repos/{ADMIN_USER}/{REPO}"),
        token,
        &json!({ "has_actions": true }),
    )
    .await;
    assert_success(&response, "enable actions");
}

async fn put_workflow_file(base: &str, token: &str) -> String {
    let content = base64::engine::general_purpose::STANDARD.encode(CI_WORKFLOW);
    let response = api_json(
        "POST",
        format!("{base}/api/v1/repos/{ADMIN_USER}/{REPO}/contents/.forgejo/workflows/ci.yml"),
        token,
        &json!({
            "content": content,
            "message": "add CI workflow",
            "branch": "main",
        }),
    )
    .await;
    let body = assert_success_json(&response, "put workflow");
    body["commit"]["sha"]
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| panic!("no commit sha in contents response: {body}"))
}

fn assert_success(response: &HttpResponseData, what: &str) {
    if !(200..300).contains(&response.status) {
        let status = response.status;
        let body = String::from_utf8_lossy(&response.body);
        panic!("{what} failed: {status} {body}");
    }
}

fn assert_success_json(response: &HttpResponseData, what: &str) -> Value {
    assert_success(response, what);
    serde_json::from_slice(&response.body)
        .unwrap_or_else(|error| panic!("{what} response should be json: {error}"))
}

async fn create_and_close_issue(world: &LiveWorld) {
    let title = "temper-forge-forgejo live smoke issue";
    let created = world
        .forge
        .create_issue(
            &world.repo_id,
            CreateIssue {
                title: title.to_string(),
                body: "Created by the throwaway live smoke suite.".to_string(),
                labels: Vec::new(),
                assignees: Vec::new(),
            },
        )
        .await
        .expect("create_issue should succeed");
    assert_eq!(created.title, title);

    let closed = world
        .forge
        .update_issue(
            &created.id,
            UpdateIssue {
                state: Some(IssueState::Closed),
                ..UpdateIssue::default()
            },
        )
        .await
        .expect("update_issue (close) should succeed");
    assert_eq!(closed.state, IssueState::Closed);
}

async fn wait_for_ci_success(cx: &temper_io_engine::Cx, world: &LiveWorld) {
    let deadline = Instant::now() + Duration::from_secs(180);
    loop {
        let observation = match world
            .forge
            .list_ci_jobs(&world.repo_id, CiJobQuery::default())
            .await
        {
            Ok(jobs) => {
                if jobs.iter().any(|job| {
                    job.status == CiJobStatus::Completed
                        && job.conclusion == Some(CiJobConclusion::Success)
                }) {
                    return;
                }
                format!("jobs={jobs:?}")
            }
            Err(error) => error.to_string(),
        };
        if Instant::now() >= deadline {
            panic!("CI success was not observed within 180s; last observation: {observation}");
        }
        temper_io_engine::runtime::sleep_for(cx, Duration::from_secs(2)).await;
    }
}
