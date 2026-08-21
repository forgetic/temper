// SPDX-License-Identifier: MPL-2.0

use async_trait::async_trait;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use temper_forge::{Repository, RepositoryId, RepositoryPath};
use temper_forge_forgejo::{
    ForgejoConfig, ForgejoForge, HttpClient, HttpError, HttpMethod, HttpRequest,
    HttpRequestProvenanceSnapshot, HttpResponse,
};
use temper_runner::RepositoryTarget;
use temper_workflow::{RawWorkflowSpec, ValidatedWorkflow};

pub(super) const HEAD: &str = "exacthead123456789";
const PAGE_LIMIT: usize = 50;
const PROVENANCE_CAPACITY: usize = 256;

const LANDING_WORKFLOW: &str = r#"
{
  "name": "paged-forgejo-landing",
  "roles": [
    { "id": "engineer", "queues": ["failed"] },
    { "id": "mechanical" }
  ],
  "labels": [
    { "id": "implementation" },
    { "id": "landing" }
  ],
  "artifact_kinds": [
    { "id": "implementation_pr", "target": "pull_request", "identifying_labels": ["implementation"] }
  ],
  "queues": [
    {
      "id": "failed",
      "artifact": "implementation_pr",
      "condition": { "kind": "ci_failed" }
    },
    {
      "id": "landing",
      "artifact": "implementation_pr",
      "labels": ["landing"],
      "condition": { "kind": "ci_passed" },
      "automation": { "actor": "mechanical", "transition": "land_pr" }
    }
  ],
  "transitions": [
    {
      "id": "land_pr",
      "artifact": "implementation_pr",
      "roles": ["mechanical"],
      "effects": [{ "kind": "merge_pull_request" }]
    }
  ]
}
"#;

#[derive(Clone, Copy, Debug)]
pub(super) enum InventoryMode {
    GreenOnLaterPage,
    PageCeiling,
    NonAdvancing,
    Status,
    Decode,
    TransportCap,
}

impl InventoryMode {
    pub(super) const FAILURES: [Self; 5] = [
        Self::PageCeiling,
        Self::NonAdvancing,
        Self::Status,
        Self::Decode,
        Self::TransportCap,
    ];
}

#[derive(Debug)]
struct FixtureState {
    mode: InventoryMode,
    requests: Vec<HttpRequest>,
    merged: bool,
}

#[derive(Clone, Debug)]
pub(super) struct ForgejoFixtureClient {
    state: Arc<Mutex<FixtureState>>,
}

impl ForgejoFixtureClient {
    fn new(mode: InventoryMode) -> Self {
        Self {
            state: Arc::new(Mutex::new(FixtureState {
                mode,
                requests: Vec::new(),
                merged: false,
            })),
        }
    }

    pub(super) fn requests(&self) -> Vec<HttpRequest> {
        self.state.lock().expect("fixture state").requests.clone()
    }

    pub(super) fn merged(&self) -> bool {
        self.state.lock().expect("fixture state").merged
    }

    fn respond(&self, request: &HttpRequest) -> Result<HttpResponse, HttpError> {
        let mut state = self.state.lock().expect("fixture state");
        if request.path == "/api/v1/repos/acme/widgets" && request.method == HttpMethod::Get {
            return Ok(ok(repository_json()));
        }
        if request.path == "/api/v1/repos/acme/widgets/issues" && request.method == HttpMethod::Get
        {
            return Ok(ok(json!([pull_issue()])));
        }
        if request.path == "/api/v1/repos/acme/widgets/pulls/7" && request.method == HttpMethod::Get
        {
            return Ok(ok(pull(state.merged)));
        }
        if request.path == "/api/v1/repos/acme/widgets/issues/7/dependencies"
            && request.method == HttpMethod::Get
        {
            return Ok(ok(json!([])));
        }
        if request.path == "/api/v1/repos/acme/widgets/actions/runs"
            && request.method == HttpMethod::Get
        {
            return inventory_response(state.mode, request);
        }
        if request.path == "/api/v1/repos/acme/widgets/actions/runs/901/jobs"
            && request.method == HttpMethod::Get
        {
            return Ok(ok(json!([job(32, 901, 2, 42, "test")])));
        }
        if request.path == "/api/v1/repos/acme/widgets/actions/runs/900/jobs"
            && request.method == HttpMethod::Get
        {
            return Ok(ok(json!([job(31, 900, 1, 41, "build")])));
        }
        if request.path == "/api/v1/repos/acme/widgets/pulls/7/merge"
            && request.method == HttpMethod::Post
        {
            state.merged = true;
            return Ok(HttpResponse::new(200, ""));
        }
        panic!(
            "unexpected Forgejo fixture request: {} {} {:?}",
            request.method, request.path, request.query
        );
    }
}

#[async_trait]
impl HttpClient for ForgejoFixtureClient {
    async fn execute(&self, request: HttpRequest) -> Result<HttpResponse, HttpError> {
        self.state
            .lock()
            .expect("fixture state")
            .requests
            .push(request.clone());
        self.respond(&request)
    }
}

fn inventory_response(
    mode: InventoryMode,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpError> {
    let page = request
        .query
        .iter()
        .find(|(key, _)| key == "page")
        .and_then(|(_, value)| value.parse::<u32>().ok())
        .expect("production inventory request has a page");
    assert_eq!(
        request
            .query
            .iter()
            .find(|(key, _)| key == "limit")
            .map(|(_, value)| value.as_str()),
        Some("50")
    );
    assert_eq!(request.query.len(), 2);

    match mode {
        InventoryMode::GreenOnLaterPage if page == 1 => Ok(runs(noise_runs(10_000, PAGE_LIMIT))),
        InventoryMode::GreenOnLaterPage => Ok(runs(vec![
            action_run(900, HEAD, "#7", "2026-07-21T11:59:00Z"),
            action_run(901, HEAD, "#7", "2026-07-21T12:00:00Z"),
        ])),
        InventoryMode::PageCeiling => Ok(runs(noise_runs(
            100_000 + u64::from(page) * PAGE_LIMIT as u64,
            PAGE_LIMIT,
        ))),
        InventoryMode::NonAdvancing => Ok(runs(noise_runs(20_000, PAGE_LIMIT))),
        InventoryMode::Status => Ok(HttpResponse::new(
            503,
            "STATUS-RESPONSE-BODY-MUST-NOT-BECOME-MISSING-CI",
        )),
        InventoryMode::Decode => Ok(HttpResponse::new(
            200,
            r#"{"workflow_runs":["DECODE-RESPONSE-BODY-SENTINEL"]}"#,
        )),
        InventoryMode::TransportCap => Err(HttpError::Transport(
            "response exceeded transport body cap; CREDENTIAL-SENTINEL".to_string(),
        )),
    }
}

fn ok(body: Value) -> HttpResponse {
    HttpResponse::new(200, body.to_string())
}

fn runs(rows: Vec<Value>) -> HttpResponse {
    ok(json!({ "workflow_runs": rows }))
}

fn noise_runs(first_id: u64, count: usize) -> Vec<Value> {
    (0..count)
        .map(|offset| {
            action_run(
                first_id + offset as u64,
                "historical0000000",
                "main",
                "2026-07-20T12:00:00Z",
            )
        })
        .collect()
}

fn action_run(id: u64, head: &str, prettyref: &str, updated_at: &str) -> Value {
    json!({
        "id": id,
        "status": "success",
        "prettyref": prettyref,
        "head_branch": "feature",
        "head_sha": head,
        "html_url": format!("https://forge.invalid/acme/widgets/actions/runs/{id}"),
        "created_at": "2026-07-21T11:58:00Z",
        "updated_at": updated_at
    })
}

fn job(id: u64, run_id: u64, attempt: u64, task_id: u64, name: &str) -> Value {
    json!({
        "id": id,
        "run_id": run_id,
        "attempt": attempt,
        "task_id": task_id,
        "name": name,
        "status": "success"
    })
}

fn labels() -> Value {
    json!([
        { "id": 1, "name": "implementation" },
        { "id": 2, "name": "landing" }
    ])
}

fn pull_issue() -> Value {
    json!({
        "number": 7,
        "title": "Paged exact-head CI",
        "body": "",
        "state": "open",
        "user": { "login": "author" },
        "labels": labels(),
        "created_at": "2026-07-21T11:00:00Z",
        "updated_at": "2026-07-21T12:00:00Z",
        "repository": { "name": "widgets", "full_name": "acme/widgets" },
        "pull_request": { "url": "https://forge.invalid/acme/widgets/pulls/7" }
    })
}

fn pull(merged: bool) -> Value {
    json!({
        "number": 7,
        "title": "Paged exact-head CI",
        "body": "",
        "state": if merged { "closed" } else { "open" },
        "merged": merged,
        "merge_commit_sha": merged.then_some("merged123456789"),
        "merged_by": merged.then(|| json!({ "login": "temper" })),
        "merged_at": merged.then_some("2026-07-21T12:01:00Z"),
        "user": { "login": "author" },
        "head": { "ref": "feature", "sha": HEAD },
        "base": { "ref": "main", "sha": "base123456789" },
        "labels": labels(),
        "created_at": "2026-07-21T11:00:00Z",
        "updated_at": "2026-07-21T12:00:00Z",
        "closed_at": merged.then_some("2026-07-21T12:01:00Z")
    })
}

fn repository_json() -> Value {
    json!({
        "owner": { "login": "acme" },
        "name": "widgets",
        "full_name": "acme/widgets",
        "default_branch": "main",
        "description": null,
        "created_at": "2026-07-21T10:00:00Z",
        "updated_at": "2026-07-21T12:00:00Z"
    })
}

pub(super) fn build_forge(
    mode: InventoryMode,
) -> (ForgejoForge<ForgejoFixtureClient>, ForgejoFixtureClient) {
    let client = ForgejoFixtureClient::new(mode);
    let forge = ForgejoForge::with_client(
        ForgejoConfig::new("https://forge.invalid", "AUTHENTICATION-SENTINEL"),
        client.clone(),
    )
    .with_request_provenance(PROVENANCE_CAPACITY);
    (forge, client)
}

pub(super) fn repository() -> Repository {
    Repository {
        id: RepositoryId::new("forgejo:acme/widgets"),
        owner: "acme".to_string(),
        name: "widgets".to_string(),
        default_branch: "main".to_string(),
        description: None,
        created_at: "2026-07-21T10:00:00Z".parse().expect("fixture timestamp"),
        updated_at: "2026-07-21T12:00:00Z".parse().expect("fixture timestamp"),
    }
}

pub(super) fn repository_target() -> RepositoryTarget {
    RepositoryTarget::new(repository().id, RepositoryPath::new("acme", "widgets"))
}

pub(super) fn landing_workflow() -> ValidatedWorkflow {
    serde_json::from_str::<RawWorkflowSpec>(LANDING_WORKFLOW)
        .expect("landing workflow parses")
        .validate()
        .expect("landing workflow validates")
}

fn provenance(forge: &ForgejoForge<ForgejoFixtureClient>) -> HttpRequestProvenanceSnapshot {
    forge
        .request_provenance()
        .expect("request provenance is enabled")
}

pub(super) fn assert_inventory_provenance(forge: &ForgejoForge<ForgejoFixtureClient>) {
    let provenance = provenance(forge);
    assert_eq!(provenance.dropped, 0);
    assert_eq!(
        provenance.requests.len(),
        forge.provider_request_count() as usize
    );
    let inventory = provenance
        .requests
        .iter()
        .filter(|request| request.path.ends_with("/actions/runs"))
        .collect::<Vec<_>>();
    assert!(!inventory.is_empty());
    assert!(inventory.iter().all(|request| {
        request.method == HttpMethod::Get
            && request.path == "/api/v1/repos/acme/widgets/actions/runs"
            && request.query_keys == ["page".to_string(), "limit".to_string()]
            && request.authentication_present
            && request.accepts_json
    }));
    assert!(provenance.requests.iter().all(|request| {
        !request.path.contains("/actions/tasks")
            && !request.path.contains("/user/login")
            && !request.path.ends_with("/acme/widgets/actions")
    }));
}
