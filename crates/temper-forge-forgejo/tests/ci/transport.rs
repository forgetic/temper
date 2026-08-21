// SPDX-License-Identifier: MPL-2.0
//! Real-transport regression for Forgejo 16's unpaged Actions behavior.

use serde_json::{Value, json};
use skein::http::h1::http_client::DEFAULT_MAX_BODY_SIZE as TRANSPORT_RESPONSE_CAP_BYTES;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use temper_forge_forgejo::{
    EngineHttpClient, ForgejoConfig, ForgejoForge, HttpClient, HttpError, HttpMethod, HttpRequest,
};
use temper_forge_model::{CiJobId, CiJobQuery, PullRequestId, RepositoryId};

const PAGE_LIMIT: usize = 50;
const INVENTORY_RUNS: usize = 401;
const INVENTORY_PAGES: usize = 9;
const RUN_PADDING_BYTES: usize = 42 * 1024;
const EXACT_RUN_ID: u64 = 900;
const OPAQUE_RUN_ID: u64 = 901;
const EXACT_HEAD: &str = "exacthead123456789";
const AUTHENTICATION_SENTINEL: &str = "TRANSPORT-AUTHENTICATION-SENTINEL";
const RESPONSE_PAYLOAD_SENTINEL: &str = "TRANSPORT-RESPONSE-PAYLOAD-SENTINEL";
const PRODUCTION_REQUEST_CEILING: u64 = (INVENTORY_PAGES * 2 + 3) as u64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RouteShape {
    Pull,
    RunInventory,
    RunJobs(u64),
    Other,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum QueryKeyShape {
    Page,
    Limit,
    Other,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RequestShape {
    route: RouteShape,
    query_keys: Vec<QueryKeyShape>,
    page: Option<usize>,
    limit: Option<usize>,
    authentication_present: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RequestFact {
    shape: RequestShape,
    response_bytes: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FixtureFailureClass {
    Read,
    MalformedRequest,
    Write,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct FixtureStats {
    requests: Vec<RequestFact>,
    largest_paged_response_bytes: usize,
    unpaged_response_bytes: Option<usize>,
    failures: Vec<FixtureFailureClass>,
}

struct FixtureResponses {
    inventory: Vec<Value>,
    pull: String,
    exact_jobs: String,
    opaque_jobs: String,
    not_found: String,
}

impl FixtureResponses {
    fn new() -> Self {
        let mut inventory = (0..INVENTORY_RUNS)
            .map(|offset| action_run(10_000 + offset as u64, "historical0000000", "main"))
            .collect::<Vec<_>>();
        inventory[PAGE_LIMIT + 5] = action_run(EXACT_RUN_ID, EXACT_HEAD, "#7");
        inventory[PAGE_LIMIT + 6] = action_run(OPAQUE_RUN_ID, "opaquehead123456", "main");
        Self {
            inventory,
            pull: json!({
                "number": 7,
                "state": "open",
                "user": { "login": "author" },
                "head": { "ref": "feature", "sha": EXACT_HEAD },
                "base": { "ref": "main" },
                "created_at": "2024-01-01T00:00:00Z",
                "updated_at": "2024-01-01T00:00:00Z"
            })
            .to_string(),
            exact_jobs: jobs(EXACT_RUN_ID, 31, 41, "exact-head"),
            opaque_jobs: jobs(OPAQUE_RUN_ID, 32, 42, "opaque"),
            not_found: json!({ "message": "not found" }).to_string(),
        }
    }

    fn response(&self, request: &RequestShape) -> (u16, String, bool) {
        match request.route {
            RouteShape::Pull => (200, self.pull.clone(), false),
            RouteShape::RunInventory => {
                if let Some(page) = request.page {
                    let limit = request.limit.unwrap_or(PAGE_LIMIT);
                    let start = page.saturating_sub(1).saturating_mul(limit);
                    let end = start.saturating_add(limit).min(self.inventory.len());
                    let rows = self.inventory.get(start..end).unwrap_or(&[]);
                    (200, json!({ "workflow_runs": rows }).to_string(), false)
                } else {
                    // Forgejo 16.0.1 ignores `limit` unless `page` is present.
                    (
                        200,
                        json!({ "workflow_runs": &self.inventory }).to_string(),
                        true,
                    )
                }
            }
            RouteShape::RunJobs(EXACT_RUN_ID) => (200, self.exact_jobs.clone(), false),
            RouteShape::RunJobs(OPAQUE_RUN_ID) => (200, self.opaque_jobs.clone(), false),
            RouteShape::RunJobs(_) | RouteShape::Other => (404, self.not_found.clone(), false),
        }
    }
}

struct Forgejo16InventoryServer {
    address: SocketAddr,
    shutdown: Arc<AtomicBool>,
    stats: Arc<Mutex<FixtureStats>>,
    thread: Option<JoinHandle<()>>,
}

impl Forgejo16InventoryServer {
    fn start() -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind loopback fixture");
        listener
            .set_nonblocking(true)
            .expect("configure loopback fixture");
        let address = listener.local_addr().expect("read fixture address");
        let shutdown = Arc::new(AtomicBool::new(false));
        let stats = Arc::new(Mutex::new(FixtureStats::default()));
        let thread_shutdown = Arc::clone(&shutdown);
        let thread_stats = Arc::clone(&stats);
        let thread = thread::spawn(move || {
            serve(
                listener,
                &thread_shutdown,
                &thread_stats,
                FixtureResponses::new(),
            );
        });
        Self {
            address,
            shutdown,
            stats,
            thread: Some(thread),
        }
    }

    fn base_url(&self) -> String {
        format!("http://{}", self.address)
    }

    fn stats(&self) -> FixtureStats {
        self.stats
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

impl Drop for Forgejo16InventoryServer {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        let _ = TcpStream::connect(self.address);
        if let Some(thread) = self.thread.take() {
            let joined = thread.join();
            if !std::thread::panicking() {
                joined.expect("join loopback fixture");
            }
        }
    }
}

fn action_run(id: u64, sha: &str, prettyref: &str) -> Value {
    json!({
        "id": id,
        "status": "success",
        "prettyref": prettyref,
        "head_branch": "feature",
        "head_sha": sha,
        "html_url": format!("https://forge.invalid/actions/runs/{id}"),
        "created_at": "2024-01-02T00:00:00Z",
        "updated_at": "2024-01-02T00:05:00Z",
        "fixture_padding": format!(
            "{RESPONSE_PAYLOAD_SENTINEL}{}",
            "x".repeat(RUN_PADDING_BYTES)
        )
    })
}

fn jobs(run_id: u64, job_id: u64, task_id: u64, name: &str) -> String {
    json!([{
        "id": job_id,
        "run_id": run_id,
        "attempt": 1,
        "task_id": task_id,
        "name": name,
        "status": "success"
    }])
    .to_string()
}

fn serve(
    listener: TcpListener,
    shutdown: &AtomicBool,
    stats: &Mutex<FixtureStats>,
    responses: FixtureResponses,
) {
    while !shutdown.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((mut stream, _)) => serve_one(&mut stream, stats, &responses),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(1));
            }
            Err(_) => {
                record_failure(stats, FixtureFailureClass::Read);
                return;
            }
        }
    }
}

fn serve_one(stream: &mut TcpStream, stats: &Mutex<FixtureStats>, responses: &FixtureResponses) {
    let request = match read_request_shape(stream) {
        Ok(Some(request)) => request,
        Ok(None) => return,
        Err(failure) => {
            record_failure(stats, failure);
            return;
        }
    };
    let (status, body, legacy_unpaged) = responses.response(&request);
    let body_bytes = body.len();
    {
        let mut stats = stats
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if request.route == RouteShape::RunInventory {
            if request.page.is_some() {
                stats.largest_paged_response_bytes =
                    stats.largest_paged_response_bytes.max(body_bytes);
            } else {
                stats.unpaged_response_bytes = Some(body_bytes);
            }
        }
        stats.requests.push(RequestFact {
            shape: request,
            response_bytes: body_bytes,
        });
    }

    let reason = if status == 200 { "OK" } else { "Not Found" };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\n\
         Content-Length: {body_bytes}\r\nConnection: close\r\n\r\n"
    );
    if stream.write_all(head.as_bytes()).is_err() {
        record_failure(stats, FixtureFailureClass::Write);
        return;
    }
    if stream.write_all(body.as_bytes()).is_err() && !legacy_unpaged {
        record_failure(stats, FixtureFailureClass::Write);
    }
}

fn record_failure(stats: &Mutex<FixtureStats>, failure: FixtureFailureClass) {
    stats
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .failures
        .push(failure);
}

fn read_request_shape(stream: &mut TcpStream) -> Result<Option<RequestShape>, FixtureFailureClass> {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|_| FixtureFailureClass::Read)?;
    let mut request = Vec::with_capacity(1024);
    let mut buffer = [0_u8; 1024];
    loop {
        let read = stream
            .read(&mut buffer)
            .map_err(|_| FixtureFailureClass::Read)?;
        if read == 0 {
            return Ok(None);
        }
        request.extend_from_slice(&buffer[..read]);
        if request.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
        if request.len() > 64 * 1024 {
            return Err(FixtureFailureClass::MalformedRequest);
        }
    }

    let request =
        std::str::from_utf8(&request).map_err(|_| FixtureFailureClass::MalformedRequest)?;
    let mut lines = request.split("\r\n");
    let request_line = lines.next().ok_or(FixtureFailureClass::MalformedRequest)?;
    let mut request_parts = request_line.split_whitespace();
    if request_parts.next() != Some("GET") {
        return Err(FixtureFailureClass::MalformedRequest);
    }
    let target = request_parts
        .next()
        .ok_or(FixtureFailureClass::MalformedRequest)?;
    let authentication_present = lines.any(|line| {
        line.split_once(':')
            .is_some_and(|(name, _)| name.eq_ignore_ascii_case("authorization"))
    });
    let (path, query) = target.split_once('?').map_or((target, ""), |parts| parts);
    let route = route_shape(path);
    let mut query_keys = Vec::new();
    let mut page = None;
    let mut limit = None;
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        match key {
            "page" => {
                query_keys.push(QueryKeyShape::Page);
                page = value.parse().ok();
            }
            "limit" => {
                query_keys.push(QueryKeyShape::Limit);
                limit = value.parse().ok();
            }
            _ => query_keys.push(QueryKeyShape::Other),
        }
    }
    Ok(Some(RequestShape {
        route,
        query_keys,
        page,
        limit,
        authentication_present,
    }))
}

fn route_shape(path: &str) -> RouteShape {
    const ROOT: &str = "/api/v1/repos/acme/widgets";
    if path == format!("{ROOT}/pulls/7") {
        return RouteShape::Pull;
    }
    let runs = format!("{ROOT}/actions/runs");
    if path == runs {
        return RouteShape::RunInventory;
    }
    path.strip_prefix(&format!("{runs}/"))
        .and_then(|suffix| suffix.strip_suffix("/jobs"))
        .and_then(|run_id| run_id.parse().ok())
        .map_or(RouteShape::Other, RouteShape::RunJobs)
}

#[test]
fn production_stack_pages_oversized_forgejo_16_inventory() {
    let server = Forgejo16InventoryServer::start();
    let base_url = server.base_url();
    let (legacy_rejected, listing, opaque, request_count, provenance) =
        temper_engine_io::block_on(async move {
            let legacy_client = EngineHttpClient::new(&base_url);
            let legacy_rejected = matches!(
                legacy_client
                    .execute(HttpRequest {
                        method: HttpMethod::Get,
                        path: "/api/v1/repos/acme/widgets/actions/runs".to_string(),
                        query: vec![("limit".to_string(), PAGE_LIMIT.to_string())],
                        headers: vec![(
                            "Authorization".to_string(),
                            format!("token {AUTHENTICATION_SENTINEL}"),
                        )],
                        body: None,
                    })
                    .await,
                Err(HttpError::Transport(_))
            );

            let forge = ForgejoForge::new(ForgejoConfig::new(&base_url, AUTHENTICATION_SENTINEL))
                .with_request_provenance(PRODUCTION_REQUEST_CEILING as usize);
            let repo_id = RepositoryId::new("forgejo:acme/widgets");
            let listing = forge
                .list_ci_jobs_with_presence(
                    &repo_id,
                    CiJobQuery {
                        pull_request_id: Some(PullRequestId::new("forgejo:acme/widgets:pull:7")),
                        commit_sha: Some(EXACT_HEAD.to_string()),
                        ..Default::default()
                    },
                )
                .await
                .expect("exact-head inventory read remains below the transport cap");
            let opaque = forge
                .get_ci_job(&CiJobId::new("forgejo:acme/widgets:actions:901:32:1:42"))
                .await
                .expect("opaque inventory read remains below the transport cap");
            (
                legacy_rejected,
                listing,
                opaque,
                forge.provider_request_count(),
                forge
                    .request_provenance()
                    .expect("request provenance is enabled"),
            )
        });

    assert!(
        legacy_rejected,
        "the test-only unpaged control must hit the cap"
    );
    assert!(listing.matching_ci_present());
    assert_eq!(listing.jobs().len(), 1);
    assert_eq!(
        listing.jobs()[0].id.as_str(),
        "forgejo:acme/widgets:actions:900:31:1:41"
    );
    assert_eq!(
        opaque.expect("opaque job after page one").id.as_str(),
        "forgejo:acme/widgets:actions:901:32:1:42"
    );
    assert_eq!(request_count, PRODUCTION_REQUEST_CEILING);
    assert_eq!(provenance.dropped, 0);
    assert_eq!(provenance.requests.len(), request_count as usize);

    let run_reads = provenance
        .requests
        .iter()
        .filter(|request| request.path.ends_with("/actions/runs"))
        .collect::<Vec<_>>();
    assert_eq!(run_reads.len(), INVENTORY_PAGES * 2);
    assert!(run_reads.iter().all(|request| {
        request.query_keys == ["page".to_string(), "limit".to_string()]
            && request.authentication_present
    }));

    let stats = server.stats();
    assert!(stats.failures.is_empty(), "{stats:?}");
    assert_eq!(stats.requests.len(), request_count as usize + 1);
    assert!(
        stats
            .requests
            .iter()
            .all(|fact| fact.shape.authentication_present)
    );
    assert!(
        stats
            .unpaged_response_bytes
            .is_some_and(|bytes| bytes > TRANSPORT_RESPONSE_CAP_BYTES),
        "{stats:?}"
    );
    assert!(
        stats.largest_paged_response_bytes < TRANSPORT_RESPONSE_CAP_BYTES,
        "{stats:?}"
    );
    let inventory_reads = stats
        .requests
        .iter()
        .filter(|fact| fact.shape.route == RouteShape::RunInventory)
        .collect::<Vec<_>>();
    assert_eq!(inventory_reads.len(), INVENTORY_PAGES * 2 + 1);
    assert_eq!(inventory_reads[0].shape.page, None);
    assert_eq!(inventory_reads[0].shape.limit, Some(PAGE_LIMIT));
    assert_eq!(inventory_reads[0].shape.query_keys, [QueryKeyShape::Limit]);
    assert!(
        stats.requests[1..]
            .iter()
            .all(|fact| fact.response_bytes < TRANSPORT_RESPONSE_CAP_BYTES),
        "{stats:?}"
    );
    assert_eq!(
        inventory_reads[1..]
            .iter()
            .map(|fact| fact.shape.page)
            .collect::<Vec<_>>(),
        (1..=INVENTORY_PAGES)
            .chain(1..=INVENTORY_PAGES)
            .map(Some)
            .collect::<Vec<_>>()
    );
    assert!(inventory_reads[1..].iter().all(|fact| {
        fact.shape.limit == Some(PAGE_LIMIT)
            && fact.response_bytes < TRANSPORT_RESPONSE_CAP_BYTES
            && fact.shape.query_keys == [QueryKeyShape::Page, QueryKeyShape::Limit]
    }));

    let retained_diagnostics = format!("{stats:?} {provenance:?}");
    assert!(!retained_diagnostics.contains(RESPONSE_PAYLOAD_SENTINEL));
    assert!(!retained_diagnostics.contains(AUTHENTICATION_SENTINEL));
}
