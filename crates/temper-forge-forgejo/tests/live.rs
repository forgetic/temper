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
use serde_json::{Value, json};
use std::time::{Duration, Instant};
use temper_engine_io::http::{HttpCall, HttpResponseData, http_call};
use temper_forge_forgejo::{EngineHttpClient, ForgejoConfig, ForgejoForge};
use temper_forge_model::{
    CandidateLabelSelection, CandidateLifecycle, CiJobConclusion, CiJobQuery, CiJobStatus,
    CreateIssue, IssueCandidateQuery, IssueQuery, IssueState, PullRequestQuery, RepositoryId,
    RepositoryPath, UpdateIssue, UpsertLabel,
};

const ADMIN_USER: &str = "liveadmin";
const ADMIN_PASSWORD: &str = "L1ve-Smoke-Admin!";
const ADMIN_EMAIL: &str = "liveadmin@example.invalid";
const REPO: &str = "forgejo-live-smoke";
const CI_WORKFLOW_PATH: &str = ".forgejo/workflows/ci.yml";

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

        candidate_label_filter_is_any_of(&world).await;
        create_and_close_issue(&world).await;
        wait_for_ci_success(&cx, &world).await;
    });
}

// The fixture is now Forgejo 16 and REST-only. The dependent cleanup removes
// this superseded Forgejo 15 web-UI contract and its support module entirely.
#[cfg(any())]
#[test]
#[ignore = "superseded Forgejo 15 web-UI compatibility contract"]
fn forgejo_15_0_3_web_ui_ci_contract() {
    temper_engine_io::block_on_with(move |cx, _handle| async move {
        let world = boot_world().await;

        // First prove the fixture's host-mode runner has produced a real,
        // terminal job. This backend uses Forgejo 15's healthy REST endpoint;
        // the contract backend below then reads that same provider job with
        // only REST run discovery forced unavailable.
        wait_for_ci_success(&cx, &world).await;

        let base_url = world._server.base_url().to_string();
        let rest_runs_path = format!("/api/v1/repos/{ADMIN_USER}/{REPO}/actions/runs");
        let client = RestRuns404Client::new(&base_url, rest_runs_path.clone());
        let version_response = client
            .execute(HttpRequest {
                method: HttpMethod::Get,
                path: "/api/v1/version".to_string(),
                query: Vec::new(),
                headers: vec![("Accept".to_string(), "application/json".to_string())],
                body: None,
            })
            .await
            .expect("Forgejo version request sends");
        assert_eq!(version_response.status, 200);
        let version: Value =
            serde_json::from_str(&version_response.body).expect("version response is JSON");
        assert!(
            version["version"].as_str().is_some_and(
                |version| version.starts_with(bench_forgejo::download::FORGEJO_VERSION)
            ),
            "the live contract must run against the pinned Forgejo fixture"
        );

        let fallback = ForgejoForge::with_client(
            ForgejoConfig::new(&base_url, REST_TOKEN_SENTINEL)
                .with_default_repo(ADMIN_USER, REPO)
                .with_web_ui_credentials(ADMIN_USER, ADMIN_PASSWORD),
            client.clone(),
        );
        let jobs = fallback
            .list_ci_jobs(&world.repo_id, CiJobQuery::default())
            .await
            .expect("password-authenticated CI fallback reads the real job");
        assert_eq!(
            jobs.len(),
            1,
            "the fixture workflow contains one runner job"
        );
        let job = &jobs[0];
        assert_eq!(job.name, "build");
        assert_eq!(job.status, CiJobStatus::Completed);
        assert_eq!(job.conclusion, Some(CiJobConclusion::Success));

        let exchanges = client.exchanges();
        let rest_runs = exchange(&exchanges, HttpMethod::Get, &rest_runs_path);
        assert_eq!(rest_runs.response.status, 404);
        assert_eq!(
            rest_runs.request.query,
            vec![("limit".to_string(), "200".to_string())]
        );
        assert_eq!(
            exchanges
                .iter()
                .filter(|exchange| exchange.response.status == 404)
                .count(),
            1,
            "only REST Actions-run discovery is forced to 404"
        );

        let login_get = exchange(&exchanges, HttpMethod::Get, "/user/login");
        assert_eq!(login_get.response.status, 200);
        let login_post = exchange(&exchanges, HttpMethod::Post, "/user/login");
        assert_eq!(login_post.response.status, 303);
        assert_eq!(login_post.response.header("location"), Some("/"));
        let login_body = login_post
            .request
            .body
            .as_deref()
            .expect("login POST carries a form body");
        assert!(login_body.contains("user_name=liveadmin"));
        assert!(login_body.contains("remember=on"));
        assert!(!login_body.contains("_csrf="));
        assert!(!login_get.response.body.contains("name=\"_csrf\""));
        assert_eq!(
            request_header(&login_post.request, "cookie")
                .and_then(|header| cookie_value(header, "_csrf")),
            None
        );

        let actions_path = format!("/{ADMIN_USER}/{REPO}/actions");
        let actions = exchange(&exchanges, HttpMethod::Get, &actions_path);
        assert_eq!(actions.response.status, 200);
        assert_eq!(actions.response.header("location"), None);

        let live_prefix = format!("/{ADMIN_USER}/{REPO}/actions/runs/");
        let live = exchanges
            .iter()
            .find(|exchange| {
                exchange.request.method == HttpMethod::Post
                    && exchange.request.path.starts_with(&live_prefix)
                    && exchange.request.path.ends_with("/jobs/0/attempt/1")
            })
            .cloned()
            .expect("live-view POST was delegated to Forgejo");
        let coordinate = live
            .request
            .path
            .strip_prefix(&live_prefix)
            .expect("live-view route prefix");
        let mut coordinate = coordinate.split('/');
        let run: u64 = coordinate
            .next()
            .expect("run coordinate")
            .parse()
            .expect("numeric run coordinate");
        assert_eq!(coordinate.next(), Some("jobs"));
        assert_eq!(coordinate.next(), Some("0"));
        assert_eq!(coordinate.next(), Some("attempt"));
        assert_eq!(coordinate.next(), Some("1"));
        assert_eq!(coordinate.next(), None);
        assert_eq!(
            job.id.as_str(),
            format!("forgejo:{ADMIN_USER}/{REPO}:actions:{run}:0:{run}")
        );
        assert_eq!(live.request.body.as_deref(), Some("{\"logCursors\":[]}"));
        assert_eq!(live.response.status, 200);
        assert_eq!(live.response.header("location"), None);
        assert!(
            live.response
                .header("content-type")
                .is_some_and(|value| value.starts_with("application/json"))
        );

        let cookie = request_header(&live.request, "cookie").expect("live-view cookie header");
        assert!(cookie_value(cookie, "persistent").is_some());
        assert!(cookie_value(cookie, "session").is_some());
        assert_eq!(cookie_value(cookie, "_csrf"), None);
        assert_eq!(request_header(&live.request, "x-csrf-token"), None);
        assert_eq!(request_header(&live.request, "authorization"), None);

        let live_json: Value =
            serde_json::from_str(&live.response.body).expect("live view is JSON");
        let live_run = &live_json["state"]["run"];
        assert_eq!(live_run["status"], "success");
        assert_eq!(live_run["jobs"].as_array().map(Vec::len), Some(1));
        assert_eq!(live_run["jobs"][0]["name"], "build");
        assert_eq!(live_run["jobs"][0]["status"], "success");
        let short_sha = live_run["commit"]["shortSHA"]
            .as_str()
            .expect("live view carries commit.shortSHA");
        assert_eq!(live_run["commit"]["branch"]["name"], "main");
        assert!(live_json["logs"].is_object());
        assert_eq!(job.commit_sha, short_sha);

        // The deployed-failure diagnostic used this HTML route for the same
        // run/job coordinate. Forward it with the authenticated cookie and
        // prove it remains a healthy page alongside the live-view JSON.
        let attempt_path = live.request.path.clone();
        let attempt_response = client
            .execute(HttpRequest {
                method: HttpMethod::Get,
                path: attempt_path.clone(),
                query: Vec::new(),
                headers: vec![
                    ("Accept".to_string(), "text/html".to_string()),
                    ("Cookie".to_string(), cookie.to_string()),
                ],
                body: None,
            })
            .await
            .expect("healthy attempt page request sends");
        assert_eq!(attempt_response.status, live.response.status);
        assert_eq!(attempt_response.status, 200);
        assert_eq!(attempt_response.header("location"), None);
        assert!(
            attempt_response
                .header("content-type")
                .is_some_and(|value| value.starts_with("text/html"))
        );
        assert!(
            attempt_response
                .body
                .contains("data-initial-post-response=")
        );
        assert!(
            attempt_response
                .body
                .contains("&#34;name&#34;:&#34;build&#34;")
        );
        assert!(
            attempt_response
                .body
                .contains("&#34;status&#34;:&#34;success&#34;")
        );
        assert!(
            attempt_response
                .body
                .contains(&format!("&#34;shortSHA&#34;:&#34;{short_sha}&#34;"))
        );
        let attempt = exchange(&client.exchanges(), HttpMethod::Get, &attempt_path);
        assert_eq!(request_header(&attempt.request, "cookie"), Some(cookie));

        // Portable output and provider responses must not reveal either web-UI
        // credentials or the REST-token sentinel. (The login request itself
        // necessarily carries the password over the local HTTP fixture.)
        let portable = format!("{job:?}");
        for secret in [ADMIN_PASSWORD, REST_TOKEN_SENTINEL] {
            assert!(!portable.contains(secret));
            assert!(!live.response.body.contains(secret));
            assert!(!attempt_response.body.contains(secret));
            assert!(
                exchanges
                    .iter()
                    .all(|exchange| !exchange.response.body.contains(secret))
            );
        }
        for web_path in [
            "/user/login",
            actions_path.as_str(),
            live.request.path.as_str(),
        ] {
            for observed in exchanges
                .iter()
                .filter(|exchange| exchange.request.path == web_path)
            {
                assert_eq!(request_header(&observed.request, "authorization"), None);
            }
        }
    });
}

async fn boot_world() -> LiveWorld {
    let state = ForgejoState::new(json!({
        "kind": "forgejo-backend-live-smoke",
        "version": 2,
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
                put_workflow_file(&base, &admin_token).await;
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
    let client = temper_engine_io::http::build_http_client();
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
        format!("{base}/api/v1/repos/{ADMIN_USER}/{REPO}/contents/{CI_WORKFLOW_PATH}"),
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

async fn candidate_label_filter_is_any_of(world: &LiveWorld) {
    for name in ["candidate-ready", "candidate-queued"] {
        world
            .forge
            .upsert_label(
                &world.repo_id,
                UpsertLabel {
                    name: name.to_string(),
                    color: Some("1d76db".to_string()),
                    description: None,
                },
            )
            .await
            .expect("candidate label should be created");
    }
    let created = world
        .forge
        .create_issue(
            &world.repo_id,
            CreateIssue {
                title: "Forgejo any-label candidate".to_string(),
                body: "Carries only one of the requested candidate labels.".to_string(),
                labels: vec!["candidate-ready".to_string()],
                assignees: Vec::new(),
            },
        )
        .await
        .expect("candidate issue should be created");

    let candidates = world
        .forge
        .list_issue_candidates(
            &world.repo_id,
            IssueCandidateQuery {
                lifecycle: CandidateLifecycle::Open,
                labels: CandidateLabelSelection::AnyOf(vec![
                    "candidate-ready".to_string(),
                    "candidate-queued".to_string(),
                ]),
                ..IssueCandidateQuery::default()
            },
        )
        .await
        .expect("candidate issue list should succeed");
    assert_eq!(
        candidates
            .iter()
            .map(|candidate| candidate.number)
            .collect::<Vec<_>>(),
        vec![created.number],
        "Forgejo candidate discovery must retain any-label semantics"
    );
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

async fn wait_for_ci_success(cx: &temper_engine_io::Cx, world: &LiveWorld) {
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
        temper_engine_io::runtime::sleep_for(cx, Duration::from_secs(2)).await;
    }
}
