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

use async_trait::async_trait;
use base64::Engine;
use bench_forgejo::{ForgejoRunner, ForgejoServer, ForgejoState, ServerError, download};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use temper_engine_io::http::{HttpCall, HttpResponseData, http_call};
use temper_forge_forgejo::{
    EngineHttpClient, ForgejoConfig, ForgejoForge, HttpClient, HttpError, HttpMethod, HttpRequest,
    HttpResponse,
};
use temper_forge_model::{
    CiJob, CiJobConclusion, CiJobQuery, CiJobStatus, CreateIssue, IssueQuery, IssueState,
    PullRequestQuery, RepositoryId, RepositoryPath, UpdateIssue,
};

#[path = "live/candidate.rs"]
mod candidate_live;

const ADMIN_USER: &str = "liveadmin";
const ADMIN_PASSWORD: &str = "L1ve-Smoke-Admin!";
const ADMIN_EMAIL: &str = "liveadmin@example.invalid";
const REPO: &str = "forgejo-live-smoke";
const CI_WORKFLOW_PATH: &str = ".forgejo/workflows/ci.yml";

const CI_WORKFLOW: &str = r#"name: api-only-ci
on: [push]
jobs:
  successful_job:
    runs-on: host
    steps:
      - run: echo temper forgejo API-only success
  intentionally_failing_job:
    runs-on: host
    steps:
      - run: exit 1
"#;

#[derive(serde::Deserialize, serde::Serialize)]
struct LiveMetadata {
    admin_token: String,
    head_sha: String,
}

#[derive(Clone)]
struct RecordingHttpClient {
    inner: EngineHttpClient,
    requests: Arc<Mutex<Vec<HttpRequest>>>,
}

impl RecordingHttpClient {
    fn new(base_url: &str) -> Self {
        Self {
            inner: EngineHttpClient::new(base_url),
            requests: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn clear(&self) {
        self.requests.lock().expect("request recorder").clear();
    }

    fn recorded(&self) -> Vec<HttpRequest> {
        self.requests.lock().expect("request recorder").clone()
    }
}

impl std::fmt::Debug for RecordingHttpClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RecordingHttpClient")
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl HttpClient for RecordingHttpClient {
    async fn execute(&self, request: HttpRequest) -> Result<HttpResponse, HttpError> {
        self.requests
            .lock()
            .expect("request recorder")
            .push(request.clone());
        self.inner.execute(request).await
    }
}

struct LiveWorld {
    _server: ForgejoServer,
    _runner: ForgejoRunner,
    forge: ForgejoForge<RecordingHttpClient>,
    recorder: RecordingHttpClient,
    admin_token: String,
    head_sha: String,
    repo_id: RepositoryId,
    repo_path: RepositoryPath,
}

#[test]
#[ignore = "boots local Forgejo + host-mode forgejo-runner; run with --ignored"]
fn live_smoke_suite_against_throwaway_forgejo() {
    temper_engine_io::block_on_with(move |cx, _handle| async move {
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

        candidate_live::candidate_label_filter_is_any_of(&world).await;
        candidate_live::bounded_candidate_contract_matches_live_forgejo(&world).await;
        create_and_close_issue(&world).await;
        validate_api_only_ci_contract(&cx, &world).await;
    });
}

async fn boot_world() -> LiveWorld {
    let state = ForgejoState::new(json!({
        "kind": "forgejo-backend-live-smoke",
        "version": 3,
        "forgejo_version": download::FORGEJO_VERSION,
        "admin": ADMIN_USER,
        "repo": REPO,
        "ci_workflow_path": CI_WORKFLOW_PATH,
        "ci_workflow": CI_WORKFLOW,
    }))
    .expect("live smoke state serializes");
    let cached = skein::runtime::spawn_blocking(move || {
        ForgejoServer::start_with_state(&state, |server| {
            let base = server.base_url().to_string();
            let admin_token = bootstrap_admin(server).expect("admin token bootstraps");
            // One-shot bootstrap on this blocking thread: build a fresh engine
            // runtime, perform the provisioning calls, tear it down.
            temper_engine_io::block_on(async move {
                create_initialized_repo(&base, &admin_token).await;
                enable_repo_actions(&base, &admin_token).await;
                let head_sha = put_workflow_file(&base, &admin_token).await;
                Ok::<LiveMetadata, String>(LiveMetadata {
                    admin_token,
                    head_sha,
                })
            })
        })
    })
    .await
    .expect("cached Forgejo state starts");
    let server = cached.server;
    let base = server.base_url().to_string();
    let LiveMetadata {
        admin_token,
        head_sha,
    } = cached.metadata;
    assert_fixture_version(&base, &admin_token).await;

    let mut runner = ForgejoRunner::register(&server).expect("forgejo-runner registers");
    assert!(runner.is_running(), "runner daemon exited immediately");

    let repo_path = RepositoryPath::new(ADMIN_USER, REPO);
    let recorder = RecordingHttpClient::new(&base);
    let forge = ForgejoForge::with_client(
        ForgejoConfig::new(&base, &admin_token).with_default_repo(ADMIN_USER, REPO),
        recorder.clone(),
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
        recorder,
        admin_token,
        head_sha,
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
async fn api_json(
    method: &str,
    url: String,
    token: &str,
    body: Option<&Value>,
) -> HttpResponseData {
    let client = temper_engine_io::http::build_http_client();
    http_call(
        &client,
        HttpCall {
            method: method.to_string(),
            url,
            headers: vec![
                ("Authorization".to_string(), format!("token {token}")),
                ("Content-Type".to_string(), "application/json".to_string()),
                ("Accept".to_string(), "application/json".to_string()),
            ],
            body: body
                .map(Value::to_string)
                .map(String::into_bytes)
                .unwrap_or_default(),
        },
    )
    .await
    .expect("api request sends")
}

async fn assert_fixture_version(base: &str, token: &str) {
    assert_eq!(
        download::FORGEJO_VERSION,
        "16.0.1",
        "the merged Bench fixture must remain pinned to the minimum supported Forgejo release"
    );
    let response = api_json("GET", format!("{base}/api/v1/version"), token, None).await;
    let body = assert_success_json(&response, "read fixture version");
    let reported = body["version"]
        .as_str()
        .unwrap_or_else(|| panic!("version response has no string version: {body}"));
    assert!(
        reported == download::FORGEJO_VERSION
            || reported.starts_with(&format!("{}+", download::FORGEJO_VERSION)),
        "fixture reported Forgejo {reported}, expected {}",
        download::FORGEJO_VERSION
    );
}

async fn create_initialized_repo(base: &str, token: &str) {
    let response = api_json(
        "POST",
        format!("{base}/api/v1/user/repos"),
        token,
        Some(&json!({
            "name": REPO,
            "auto_init": true,
            "default_branch": "main",
            "private": false,
        })),
    )
    .await;
    assert_success(&response, "create repo");
}

async fn enable_repo_actions(base: &str, token: &str) {
    let response = api_json(
        "PATCH",
        format!("{base}/api/v1/repos/{ADMIN_USER}/{REPO}"),
        token,
        Some(&json!({ "has_actions": true })),
    )
    .await;
    assert_success(&response, "enable actions");
}

async fn put_workflow_file(base: &str, token: &str) -> String {
    let content = base64::engine::general_purpose::STANDARD.encode(CI_WORKFLOW);
    let response = api_json(
        "POST",
        format!("{base}/api/v1/repos/{ADMIN_USER}/{REPO}/contents/{CI_WORKFLOW_PATH}"),
        token,
        Some(&json!({
            "content": content,
            "message": "add API-only CI workflow",
            "branch": "main",
        })),
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

async fn validate_api_only_ci_contract(cx: &temper_engine_io::Cx, world: &LiveWorld) {
    world.recorder.clear();
    let jobs = wait_for_terminal_ci(cx, world).await;

    assert_eq!(jobs.len(), 2, "the live workflow must expose both jobs");
    assert!(
        jobs.iter().all(|job| job.status == CiJobStatus::Completed),
        "both runner jobs must be terminal: {jobs:?}"
    );
    assert_eq!(
        jobs.iter()
            .filter(|job| job.conclusion == Some(CiJobConclusion::Success))
            .count(),
        1,
        "the successful runner job must stay distinguishable: {jobs:?}"
    );
    assert_eq!(
        jobs.iter()
            .filter(|job| job.conclusion == Some(CiJobConclusion::Unknown))
            .count(),
        1,
        "the status-only failing job must map conservatively to recovery-required Unknown: {jobs:?}"
    );

    let provider_run = jobs[0]
        .run_id
        .as_deref()
        .expect("live job carries provider run identity");
    assert!(
        provider_run.parse::<u64>().is_ok_and(|id| id > 0),
        "provider run identity must be a non-zero database id"
    );
    assert!(
        jobs.iter()
            .all(|job| job.run_id.as_deref() == Some(provider_run)),
        "both workflow jobs must retain their shared provider run identity"
    );
    assert!(jobs.iter().all(|job| {
        job.attempt
            .as_deref()
            .and_then(|attempt| attempt.parse::<u64>().ok())
            .is_some_and(|attempt| attempt > 0)
    }));
    assert_ne!(
        jobs[0].id, jobs[1].id,
        "provider jobs need stable distinct ids"
    );
    assert!(jobs.iter().all(|job| {
        job.id
            .as_str()
            .contains(&format!(":actions:{provider_run}:"))
    }));

    let repeated = world
        .forge
        .list_ci_jobs(
            &world.repo_id,
            CiJobQuery {
                commit_sha: Some(world.head_sha.clone()),
                ..CiJobQuery::default()
            },
        )
        .await
        .expect("completed jobs can be observed again");
    assert_eq!(
        repeated.iter().map(|job| &job.id).collect::<Vec<_>>(),
        jobs.iter().map(|job| &job.id).collect::<Vec<_>>(),
        "provider run/job identity must remain stable across observations"
    );

    for expected in &jobs {
        let round_tripped = world
            .forge
            .get_ci_job(&expected.id)
            .await
            .expect("provider-identified job lookup succeeds")
            .expect("listed provider job round-trips");
        assert_eq!(&round_tripped, expected);
    }

    assert_api_only_ci_requests(&world.recorder.recorded(), &world.admin_token);
}

async fn wait_for_terminal_ci(cx: &temper_engine_io::Cx, world: &LiveWorld) -> Vec<CiJob> {
    let deadline = Instant::now() + Duration::from_secs(180);
    loop {
        let observation = match world
            .forge
            .list_ci_jobs_with_presence(
                &world.repo_id,
                CiJobQuery {
                    commit_sha: Some(world.head_sha.clone()),
                    ..CiJobQuery::default()
                },
            )
            .await
        {
            Ok(listing)
                if listing.matching_ci_present()
                    && listing.jobs().len() == 2
                    && listing
                        .jobs()
                        .iter()
                        .all(|job| job.status == CiJobStatus::Completed) =>
            {
                return listing.into_jobs();
            }
            Ok(listing) => format!(
                "matching_ci_present={}, jobs={:?}",
                listing.matching_ci_present(),
                listing.jobs()
            ),
            Err(error) => error.to_string(),
        };
        if Instant::now() >= deadline {
            panic!(
                "successful and intentionally failing CI jobs were not observed within 180s; last observation: {observation}"
            );
        }
        temper_engine_io::runtime::sleep_for(cx, Duration::from_secs(2)).await;
    }
}

fn assert_api_only_ci_requests(requests: &[HttpRequest], token: &str) {
    let runs_path = format!("/api/v1/repos/{ADMIN_USER}/{REPO}/actions/runs");
    assert!(!requests.is_empty(), "CI observation recorded no requests");
    assert!(
        requests.iter().any(|request| {
            request
                .path
                .strip_prefix(&format!("{runs_path}/"))
                .and_then(|suffix| suffix.strip_suffix("/jobs"))
                .and_then(|run_id| run_id.parse::<u64>().ok())
                .is_some_and(|run_id| run_id > 0)
        }),
        "CI observation never called the provider-run jobs endpoint; paths={:?}",
        requests
            .iter()
            .map(|request| request.path.as_str())
            .collect::<Vec<_>>()
    );

    for request in requests {
        assert_eq!(request.method, HttpMethod::Get, "path={}", request.path);
        assert!(
            request.path == runs_path
                || request
                    .path
                    .strip_prefix(&format!("{runs_path}/"))
                    .and_then(|suffix| suffix.strip_suffix("/jobs"))
                    .and_then(|run_id| run_id.parse::<u64>().ok())
                    .is_some_and(|run_id| run_id > 0),
            "CI observation used a non-run/jobs API route: {} {}",
            request.method,
            request.path
        );
        assert!(
            request.headers.iter().any(|(name, value)| {
                name.eq_ignore_ascii_case("authorization") && value == &format!("token {token}")
            }),
            "CI request did not carry token authentication: {}",
            request.path
        );
        assert!(
            request.headers.iter().any(|(name, value)| {
                name.eq_ignore_ascii_case("accept") && value == "application/json"
            }),
            "CI request did not require JSON: {}",
            request.path
        );
        assert!(!request.path.contains("/user/login"));
        assert!(!request.path.contains("/actions/tasks"));
        if request.path == runs_path {
            assert_eq!(
                request
                    .query
                    .iter()
                    .filter(|(key, _)| key == "page")
                    .count(),
                1,
                "run-list request omitted or duplicated page: {:?}",
                request.query
            );
            assert!(
                request
                    .query
                    .contains(&("limit".to_string(), "200".to_string()))
            );
            assert_eq!(
                request.query.len(),
                2,
                "run-list query: {:?}",
                request.query
            );
        } else {
            assert!(
                request.query.is_empty(),
                "provider-run jobs request carried query parameters: {:?}",
                request.query
            );
        }
    }
}
