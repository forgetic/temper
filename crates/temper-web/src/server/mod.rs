// SPDX-License-Identifier: MPL-2.0

//! The temper-web HTTP + SSE server.
//!
//! ## Why its own blocking server (not skein H1)
//!
//! The daemon's skein `Http1Listener` is a one-shot request→buffered-response
//! model: it cannot hold a connection open to push SSE frames over time. SSE
//! needs exactly that. So temper-web runs its OWN server on a plain blocking
//! `std::net::TcpListener` with a thread per connection — the same shape as the
//! workspace's other non-daemon servers (interaction-service, trigger-forgejo),
//! but extended to keep `/events` connections open and stream `text/event-stream`
//! frames. This keeps temper-web a dependency-light depgraph leaf: no skein, no
//! async runtime — just `std` + `serde_json` + the temper-log/temper-workflow
//! types it consumes.
//!
//! ## Shape
//!
//! - [`AppState`] holds the read-model (behind a mutex) and the SSE
//!   [`Broadcaster`]. The feed pump applies deltas and fans the resulting
//!   [`BoardEvent`]s out; `/events` subscribers re-snap then resume from the
//!   cursor.
//! - [`route`] maps a request to a [`Response`] for the buffered endpoints
//!   (`/api/state`, `/healthz`, static files); `/events` is handled specially so
//!   the socket stays open.
//! - PR E reuses [`sse::Broadcaster`] and the framing helpers for its
//!   conversation SSE proxy.

pub mod request;
pub mod sse;
pub mod static_files;
mod trace_proxy;

pub use trace_proxy::route_trace_get;
#[cfg(test)]
use trace_proxy::{parse_event_query, parse_trace_query};
use trace_proxy::{parse_trace_stream_target, serve_trace_events};

use std::io::{BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::board::BoardEvent;
use crate::config::WebConfig;
use crate::conversation::ConversationProxy;
use crate::feeds::logtail::{LogLineSource, LogTailAdapter};
use crate::feeds::snapshot_source::SnapshotSource;
use crate::project::lanes::LaneMap;
use crate::project::snapshot::project_snapshot;
use crate::readmodel::{Delta, ReadModel};
use crate::trace::TraceApiClient;
use sse::Broadcaster;

/// A buffered HTTP response for the non-SSE endpoints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub reason: &'static str,
    pub content_type: &'static str,
    pub body: Vec<u8>,
}

impl Response {
    fn json(body: Vec<u8>) -> Self {
        Self {
            status: 200,
            reason: "OK",
            content_type: "application/json",
            body,
        }
    }

    fn not_found() -> Self {
        Self {
            status: 404,
            reason: "Not Found",
            content_type: "text/plain; charset=utf-8",
            body: b"not found".to_vec(),
        }
    }

    /// A 503 used when the conversation proxy is disabled (no `interaction_url`).
    fn service_unavailable(message: &'static str) -> Self {
        Self {
            status: 503,
            reason: "Service Unavailable",
            content_type: "text/plain; charset=utf-8",
            body: message.as_bytes().to_vec(),
        }
    }

    fn bad_request(message: &'static str) -> Self {
        Self {
            status: 400,
            reason: "Bad Request",
            content_type: "text/plain; charset=utf-8",
            body: message.as_bytes().to_vec(),
        }
    }

    fn trace_bad_gateway() -> Self {
        Self {
            status: 502,
            reason: "Bad Gateway",
            content_type: "text/plain; charset=utf-8",
            body: b"engine trace API unavailable".to_vec(),
        }
    }

    /// A 502 used when the upstream interaction-service is unreachable.
    fn bad_gateway() -> Self {
        Self {
            status: 502,
            reason: "Bad Gateway",
            content_type: "text/plain; charset=utf-8",
            body: b"interaction-service unreachable".to_vec(),
        }
    }

    /// Pass an upstream interaction-service JSON response through with its status.
    fn passthrough(status: u16, body: Vec<u8>) -> Self {
        Self {
            status,
            // The status line reason is cosmetic; the body carries the truth.
            reason: "OK",
            content_type: "application/json",
            body,
        }
    }
}

/// Shared server state: the read-model, the board SSE fan-out hub, and the
/// optional conversation proxy (feed 2). The conversation proxy is `None` when no
/// `interaction_url` is configured — the chat routes then report a clear 503 but
/// the server still boots and the board feed works.
pub struct AppState {
    model: Mutex<ReadModel>,
    broadcaster: Broadcaster,
    ui_dir: std::path::PathBuf,
    conversation: Option<Arc<ConversationProxy>>,
    trace_client: Option<Arc<dyn TraceApiClient>>,
}

impl AppState {
    /// Build app state with a cold-start read-model seeded from a snapshot
    /// source (the live daemon, a fixture, or empty) projected via `lanes`.
    #[must_use]
    pub fn new(
        snapshot: &dyn SnapshotSource,
        lanes: &LaneMap,
        now: i64,
        ui_dir: std::path::PathBuf,
    ) -> Self {
        let mut model = ReadModel::new();
        if let Some(raw) = snapshot.fetch() {
            model.load_snapshot(project_snapshot(&raw, lanes, now));
        }
        Self {
            model: Mutex::new(model),
            broadcaster: Broadcaster::new(),
            ui_dir,
            conversation: None,
            trace_client: None,
        }
    }

    /// Attach the conversation proxy (feed 2). Called at the binary entry when an
    /// `interaction_url` is configured; absent, the chat routes report 503.
    #[must_use]
    pub fn with_conversation_proxy(mut self, proxy: Arc<ConversationProxy>) -> Self {
        self.conversation = Some(proxy);
        self
    }

    /// The conversation proxy, if one is attached.
    #[must_use]
    pub fn conversation(&self) -> Option<&Arc<ConversationProxy>> {
        self.conversation.as_ref()
    }

    /// Attach the server-side engine trace client. Its credential remains
    /// encapsulated by the client and is never serialized into board state.
    #[must_use]
    pub fn with_trace_client(mut self, client: Arc<dyn TraceApiClient>) -> Self {
        self.trace_client = Some(client);
        self
    }

    #[must_use]
    pub fn trace_client(&self) -> Option<&Arc<dyn TraceApiClient>> {
        self.trace_client.as_ref()
    }

    /// Distinct artifacts currently represented by cards. Global trace polling
    /// queries only these filters, avoiding journal scans and transcript reads
    /// for unrelated or already-retained runs.
    #[must_use]
    pub(crate) fn trace_artifact_refs(&self) -> Vec<String> {
        let mut artifacts = self
            .model
            .lock()
            .expect("read-model mutex not poisoned")
            .cards()
            .values()
            .map(|card| card.artifact_ref.clone())
            .collect::<Vec<_>>();
        artifacts.sort();
        artifacts.dedup();
        artifacts
    }

    /// Project one low-rate canonical boundary onto a real card. Unknown or
    /// already-removed artifacts are ignored without consuming board sequence
    /// space, preventing irrelevant trace history from reaching subscribers.
    pub fn ingest_trace_activity(
        &self,
        artifact_ref: &str,
        event: crate::board::StreamEvent,
    ) -> bool {
        let id = {
            let model = self.model.lock().expect("read-model mutex not poisoned");
            model
                .cards()
                .values()
                .find(|card| card.artifact_ref == artifact_ref)
                .map(|card| card.id.clone())
        };
        let Some(id) = id else { return false };
        self.ingest(Delta::PushStream { id, event });
        true
    }

    /// The board SSE hub (the `/events` board feed; the conversation proxy owns
    /// its own hub for `/conversations/events`).
    #[must_use]
    pub fn broadcaster(&self) -> Broadcaster {
        self.broadcaster.clone()
    }

    /// Apply a board delta from a feed and fan the resulting event out to SSE
    /// subscribers. Called by the feed pump.
    pub fn ingest(&self, delta: crate::readmodel::Delta) {
        let event = {
            let mut model = self.model.lock().expect("read-model mutex not poisoned");
            model.apply(delta)
        };
        if let Some(event) = event {
            self.broadcast_event(&event);
        }
    }

    /// Merge a freshly-projected snapshot into the read-model and fan the
    /// resulting events out to SSE subscribers. Called by the snapshot re-poll
    /// pump ([`pump_snapshot_source`]).
    pub fn reconcile(&self, fresh: crate::board::SnapshotState, now: i64) {
        let events = {
            let mut model = self.model.lock().expect("read-model mutex not poisoned");
            model.reconcile_snapshot(fresh, now)
        };
        for event in &events {
            self.broadcast_event(event);
        }
    }

    fn broadcast_event(&self, event: &BoardEvent) {
        if let Ok(json) = serde_json::to_string(event) {
            self.broadcaster.broadcast(&sse::data_frame(&json));
        }
    }

    /// The current cold-start snapshot event (for `/api/state` and the SSE
    /// greeting). Carries the live cursor so the client resumes with no gap.
    #[must_use]
    pub fn snapshot_event(&self) -> BoardEvent {
        self.model
            .lock()
            .expect("read-model mutex not poisoned")
            .snapshot_event()
    }
}

/// Route a buffered (non-SSE) request to a [`Response`]. `/events` is handled by
/// the connection loop directly (it streams), so it is not routed here.
#[must_use]
pub fn route(state: &AppState, method: &str, path_only: &str) -> Response {
    match (method, path_only) {
        ("GET", "/healthz") => Response {
            status: 200,
            reason: "OK",
            content_type: "text/plain; charset=utf-8",
            body: crate::healthz().as_bytes().to_vec(),
        },
        ("GET", "/api/state") => match serde_json::to_vec(&state.snapshot_event()) {
            Ok(body) => Response::json(body),
            Err(_) => Response {
                status: 500,
                reason: "Internal Server Error",
                content_type: "text/plain; charset=utf-8",
                body: b"snapshot serialization failed".to_vec(),
            },
        },
        ("GET", _) => match static_files::resolve(&state.ui_dir, path_only) {
            Ok(file) => Response {
                status: 200,
                reason: "OK",
                content_type: file.content_type,
                body: file.bytes,
            },
            Err(_) => Response::not_found(),
        },
        _ => Response::not_found(),
    }
}

/// Pump a log-line source into the read-model on a background thread: drain new
/// lines through the adapter and `ingest` each delta. Designed so a future
/// in-process broadcast source swaps in behind [`LogLineSource`] unchanged.
///
/// The source's `next_line` returns `None` when momentarily drained; the pump
/// then sleeps `idle` and retries, also emitting an SSE keep-alive so an idle
/// pipe still proves liveness.
pub fn pump_log_source<S: LogLineSource + Send + 'static>(
    state: Arc<AppState>,
    mut adapter: LogTailAdapter,
    mut source: S,
    idle: Duration,
    clock: impl Fn() -> i64 + Send + 'static,
) {
    std::thread::spawn(move || {
        loop {
            adapter.set_now(clock());
            match source.next_line() {
                Some(line) => {
                    for delta in adapter.deltas_for_line(&line) {
                        state.ingest(delta);
                    }
                }
                None => {
                    state.broadcaster.broadcast(&sse::keep_alive());
                    std::thread::sleep(idle);
                }
            }
        }
    });
}

/// Pump the daemon snapshot source into the read-model on a background thread:
/// re-fetch on `interval`, project via [`project_snapshot`], reconcile-merge into
/// the model (see [`ReadModel::reconcile_snapshot`]), and fan each resulting
/// [`BoardEvent`] out to SSE subscribers.
///
/// This is the liveness fix for the one-shot cold-start snapshot: a job that goes
/// in-flight after startup appears within one interval, with no reload or restart.
/// The reconcile **merges** rather than clobbers, so log-tail-refined lifecycle
/// state survives the re-poll.
///
/// The source is shared (`Arc`) with cold start; `lanes` is cloned in so the pump
/// projects identically. A fetch that yields `None` (daemon momentarily
/// unreachable) is skipped — the model keeps its last good state.
pub fn pump_snapshot_source(
    state: Arc<AppState>,
    source: Arc<dyn SnapshotSource>,
    lanes: LaneMap,
    interval: Duration,
    clock: impl Fn() -> i64 + Send + 'static,
) {
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(interval);
            let Some(raw) = source.fetch() else { continue };
            let now = clock();
            let fresh = project_snapshot(&raw, &lanes, now);
            state.reconcile(fresh, now);
        }
    });
}

/// Serve forever on the configured bind address. Each connection runs on its own
/// thread; `/events` connections stay open and stream frames, all others get a
/// single buffered response.
pub fn serve(state: Arc<AppState>, config: &WebConfig) -> std::io::Result<()> {
    let listener = TcpListener::bind(config.bind)?;
    tracing::info!(target: "temper_web", bind = %config.bind, "temper-web: serving");
    let keep_alive = Duration::from_millis(config.keep_alive_ms);
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let state = Arc::clone(&state);
        std::thread::spawn(move || {
            if let Err(error) = handle_connection(stream, &state, keep_alive) {
                tracing::debug!(target: "temper_web", %error, "connection ended");
            }
        });
    }
    Ok(())
}

/// Handle one connection: route a buffered request, or — for the board `/events`
/// and conversation `/conversations/events` paths — upgrade to a long-lived SSE
/// stream. POST conversation routes read the body and forward it upstream.
fn handle_connection(
    mut stream: TcpStream,
    state: &AppState,
    keep_alive: Duration,
) -> std::io::Result<()> {
    let request = {
        let mut reader =
            BufReader::new(stream.try_clone().expect("clone stream for buffered read"));
        request::read_request(&mut reader)
    };
    let Some(request) = request else {
        return Ok(()); // closed/empty connection
    };
    let method = request.line.method.as_str();
    let target = request.line.path.as_str();
    let path_only = request.line.path_only();

    if method == "GET" && path_only == "/events" {
        return serve_events(stream, state, keep_alive);
    }
    if method == "GET" && path_only == "/conversations/events" {
        return serve_conversation_events(stream, state, keep_alive);
    }
    if method == "GET" {
        match parse_trace_stream_target(target) {
            Ok(Some((run_id, after_seq))) => {
                let last_event_id = request
                    .headers
                    .get("last-event-id")
                    .and_then(|value| value.parse::<u64>().ok())
                    .unwrap_or(0);
                if state.trace_client().is_none() {
                    let response = Response::service_unavailable("agent trace API disabled");
                    return request::write_response(
                        &mut stream,
                        response.status,
                        response.reason,
                        response.content_type,
                        &response.body,
                    );
                }
                return serve_trace_events(
                    stream,
                    state,
                    &run_id,
                    after_seq.max(last_event_id),
                    keep_alive
                        .min(Duration::from_secs(1))
                        .max(Duration::from_millis(50)),
                );
            }
            Err(message) => {
                let response = Response::bad_request(message);
                return request::write_response(
                    &mut stream,
                    response.status,
                    response.reason,
                    response.content_type,
                    &response.body,
                );
            }
            Ok(None) => {}
        }
    }

    let response = if method == "GET" && path_only.starts_with("/api/agent-runs") {
        route_trace_get(state, target)
    } else if method == "POST" && path_only.starts_with("/conversations") {
        route_conversation_post(state, path_only, &request.body)
    } else {
        route(state, method, path_only)
    };
    request::write_response(
        &mut stream,
        response.status,
        response.reason,
        response.content_type,
        &response.body,
    )
}

/// Route a `POST /conversations…` proxy request to the interaction-service via
/// the injected client, passing its response through. Returns 503 when the proxy
/// is disabled (no `interaction_url`) and 502 when the upstream is unreachable.
fn route_conversation_post(state: &AppState, path_only: &str, body: &[u8]) -> Response {
    let Some(proxy) = state.conversation() else {
        return Response::service_unavailable("conversation proxy disabled (no interaction_url)");
    };
    let body = String::from_utf8_lossy(body);
    let segments: Vec<&str> = path_only
        .trim_matches('/')
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect();
    // Mirror the interaction-service's own route shapes (verified in its http.rs).
    let result = match segments.as_slice() {
        ["conversations"] => proxy.forward_new_conversation(&body),
        ["conversations", id, "turns"] => proxy.forward_turn(id, &body),
        ["conversations", id, "proposals", proposal_id, "accept"] => {
            proxy.forward_accept(id, proposal_id)
        }
        _ => return Response::not_found(),
    };
    match result {
        Ok(upstream) => Response::passthrough(upstream.status, upstream.body.into_bytes()),
        Err(error) => {
            tracing::debug!(target: "temper_web", %error, "conversation forward failed");
            Response::bad_gateway()
        }
    }
}

/// Stream the board SSE feed on this connection: send the SSE head, the
/// cold-start snapshot frame (so the client re-snaps and resumes from the
/// cursor), then live frames as they arrive, with periodic keep-alives.
fn serve_events(
    mut stream: TcpStream,
    state: &AppState,
    keep_alive: Duration,
) -> std::io::Result<()> {
    stream.write_all(sse::sse_response_head().as_bytes())?;

    // Subscribe BEFORE snapshotting so no event is missed between the two.
    let subscription = state.broadcaster.subscribe();
    let snapshot = state.snapshot_event();
    if let Ok(json) = serde_json::to_string(&snapshot) {
        stream.write_all(sse::data_frame(&json).as_bytes())?;
        stream.flush()?;
    }

    pump_subscription(&mut stream, &subscription, keep_alive)
}

/// Stream the conversation SSE feed (feed 2) on this connection: send the SSE
/// head, then live `ConversationEvent` frames as the poller produces them, with
/// periodic keep-alives. The chat page seeds its own transcript (from
/// `GET /conversations`), so there is no cold-start greeting frame here.
///
/// When the proxy is disabled (no `interaction_url`), the stream still opens and
/// emits only keep-alives — the chat page connects cleanly and shows an idle, live
/// pipe rather than a failed `EventSource` (matching how the board degrades).
fn serve_conversation_events(
    mut stream: TcpStream,
    state: &AppState,
    keep_alive: Duration,
) -> std::io::Result<()> {
    stream.write_all(sse::sse_response_head().as_bytes())?;
    stream.flush()?;

    let Some(proxy) = state.conversation() else {
        return keep_alive_only(&mut stream, keep_alive);
    };
    let subscription = proxy.broadcaster().subscribe();
    pump_subscription(&mut stream, &subscription, keep_alive)
}

/// Drive an SSE subscription to a socket: write each broadcast frame, and on a
/// quiet interval send a keep-alive so the client's pulse can tell idle from dead.
/// A write/flush error (client closed) ends the loop by propagating.
fn pump_subscription(
    stream: &mut TcpStream,
    subscription: &sse::Subscription,
    keep_alive: Duration,
) -> std::io::Result<()> {
    loop {
        match subscription.recv_timeout(keep_alive) {
            Ok(frame) => stream.write_all(frame.as_bytes())?,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                stream.write_all(sse::keep_alive().as_bytes())?;
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
        // A write error here means the client closed; propagate to drop the conn.
        stream.flush()?;
    }
    Ok(())
}

/// Emit only keep-alives forever (the disabled-proxy idle stream).
fn keep_alive_only(stream: &mut TcpStream, keep_alive: Duration) -> std::io::Result<()> {
    loop {
        std::thread::sleep(keep_alive);
        stream.write_all(sse::keep_alive().as_bytes())?;
        stream.flush()?;
    }
}

#[cfg(test)]
#[path = "server_tests.rs"]
mod server_tests;
