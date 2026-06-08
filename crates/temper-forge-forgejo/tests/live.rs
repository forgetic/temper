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
use temper_forge_forgejo::{ForgejoConfig, ForgejoForge, ReqwestHttpClient};

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
    forge: ForgejoForge<ReqwestHttpClient>,
    repo_id: RepositoryId,
    repo_path: RepositoryPath,
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "boots local Forgejo + host-mode forgejo-runner; run with --ignored"]
async fn live_smoke_suite_against_throwaway_forgejo() {
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
    wait_for_ci_success(&world).await;
}

async fn boot_world() -> LiveWorld {
    let state = ForgejoState::new(json!({
        "kind": "forgejo-backend-live-smoke",
        "version": 1,
        "admin": ADMIN_USER,
        "repo": REPO,
    }))
    .expect("live smoke state serializes");
    let cached = tokio::task::spawn_blocking(move || {
        ForgejoServer::start_with_state(&state, |server| {
            let base = server.base_url().to_string();
            let admin_token = bootstrap_admin(server).expect("admin token bootstraps");
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio runtime builds")
                .block_on(async {
                    let client = reqwest::Client::builder()
                        .timeout(Duration::from_secs(15))
                        .build()
                        .map_err(|err| err.to_string())?;
                    create_initialized_repo(&client, &base, &admin_token).await;
                    Ok::<LiveMetadata, String>(LiveMetadata { admin_token })
                })
        })
    })
    .await
    .expect("server boot task joins")
    .expect("cached Forgejo state starts");
    let server = cached.server;
    let base = server.base_url().to_string();
    let admin_token = cached.metadata.admin_token;

    let mut runner = ForgejoRunner::register(&server).expect("forgejo-runner registers");
    assert!(runner.is_running(), "runner daemon exited immediately");

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .expect("reqwest client builds");
    enable_repo_actions(&client, &base, &admin_token).await;
    put_workflow_file(&client, &base, &admin_token).await;

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

async fn create_initialized_repo(client: &reqwest::Client, base: &str, token: &str) {
    let response = client
        .post(format!("{base}/api/v1/user/repos"))
        .header("Authorization", format!("token {token}"))
        .json(&json!({
            "name": REPO,
            "auto_init": true,
            "default_branch": "main",
            "private": false,
        }))
        .send()
        .await
        .expect("create repo request sends");
    assert_success(response, "create repo").await;
}

async fn enable_repo_actions(client: &reqwest::Client, base: &str, token: &str) {
    let response = client
        .patch(format!("{base}/api/v1/repos/{ADMIN_USER}/{REPO}"))
        .header("Authorization", format!("token {token}"))
        .json(&json!({ "has_actions": true }))
        .send()
        .await
        .expect("enable actions request sends");
    assert_success(response, "enable actions").await;
}

async fn put_workflow_file(client: &reqwest::Client, base: &str, token: &str) -> String {
    let content = base64::engine::general_purpose::STANDARD.encode(CI_WORKFLOW);
    let response = client
        .post(format!(
            "{base}/api/v1/repos/{ADMIN_USER}/{REPO}/contents/.forgejo/workflows/ci.yml"
        ))
        .header("Authorization", format!("token {token}"))
        .json(&json!({
            "content": content,
            "message": "add CI workflow",
            "branch": "main",
        }))
        .send()
        .await
        .expect("put workflow request sends");
    let body = assert_success_json(response, "put workflow").await;
    body["commit"]["sha"]
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| panic!("no commit sha in contents response: {body}"))
}

async fn assert_success(response: reqwest::Response, what: &str) {
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        panic!("{what} failed: {status} {body}");
    }
}

async fn assert_success_json(response: reqwest::Response, what: &str) -> Value {
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        panic!("{what} failed: {status} {body}");
    }
    response
        .json()
        .await
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

async fn wait_for_ci_success(world: &LiveWorld) {
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
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}
