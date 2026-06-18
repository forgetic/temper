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

use std::io::{BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::board::BoardEvent;
use crate::config::WebConfig;
use crate::feeds::logtail::{LogLineSource, LogTailAdapter};
use crate::feeds::snapshot_source::SnapshotSource;
use crate::project::lanes::LaneMap;
use crate::project::snapshot::project_snapshot;
use crate::readmodel::ReadModel;
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
}

/// Shared server state: the read-model and the SSE fan-out hub.
pub struct AppState {
    model: Mutex<ReadModel>,
    broadcaster: Broadcaster,
    ui_dir: std::path::PathBuf,
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
        }
    }

    /// The SSE hub (shared with PR E's conversation proxy).
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

/// Handle one connection: route a buffered request, or — for `/events` — upgrade
/// to a long-lived SSE stream (snapshot greeting, then live frames + keep-alive).
fn handle_connection(
    mut stream: TcpStream,
    state: &AppState,
    keep_alive: Duration,
) -> std::io::Result<()> {
    let request = {
        let mut reader =
            BufReader::new(stream.try_clone().expect("clone stream for buffered read"));
        request::read_request_line(&mut reader)
    };
    let Some(request) = request else {
        return Ok(()); // closed/empty connection
    };

    if request.method == "GET" && request.path_only() == "/events" {
        return serve_events(stream, state, keep_alive);
    }

    let response = route(state, &request.method, request.path_only());
    request::write_response(
        &mut stream,
        response.status,
        response.reason,
        response.content_type,
        &response.body,
    )
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

#[cfg(test)]
#[path = "server_tests.rs"]
mod server_tests;
