// SPDX-License-Identifier: MPL-2.0

//! Standalone async daemon transport for the Worker/Daemon wire protocol.

use std::{net::SocketAddr, sync::Arc};

use axum::{
    body::Bytes,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use temper_worker_protocol::{Artifact, WorkerProtocolMessage};
use temper_worker_registry::DaemonCore;
use tokio::sync::{Mutex, Notify};

#[derive(Clone)]
struct DaemonState {
    core: Arc<Mutex<DaemonCore>>,
    notify: Arc<Notify>,
}

#[derive(Clone)]
pub struct Daemon {
    core: Arc<Mutex<DaemonCore>>,
    notify: Arc<Notify>,
}

impl Daemon {
    pub fn new() -> Self {
        Self {
            core: Arc::new(Mutex::new(DaemonCore::new())),
            notify: Arc::new(Notify::new()),
        }
    }

    pub async fn enqueue_job(
        &self,
        job_id: impl Into<String>,
        role: impl Into<String>,
        repo: impl Into<String>,
        artifact: Artifact,
        job_payload: serde_json::Value,
    ) {
        {
            let mut core = self.core.lock().await;
            core.enqueue_job(job_id, role, repo, artifact, job_payload);
        }
        self.notify.notify_waiters();
    }

    pub fn router(&self) -> Router {
        let state = DaemonState {
            core: Arc::clone(&self.core),
            notify: Arc::clone(&self.notify),
        };

        Router::new()
            .route("/v1/message", post(handle_message))
            .with_state(state)
    }
}

impl Default for Daemon {
    fn default() -> Self {
        Self::new()
    }
}

async fn handle_message(State(state): State<DaemonState>, body: Bytes) -> Response {
    let Ok(msg) = serde_json::from_slice::<WorkerProtocolMessage>(&body) else {
        return StatusCode::BAD_REQUEST.into_response();
    };

    let _notify = Arc::clone(&state.notify);
    let reply = {
        let mut core = state.core.lock().await;
        core.handle(msg)
    };

    match reply {
        Some(reply) => (StatusCode::OK, Json(reply)).into_response(),
        None => StatusCode::NO_CONTENT.into_response(),
    }
}

pub async fn serve(daemon: &Daemon, bind: SocketAddr) -> std::io::Result<()> {
    let listener = tokio::net::TcpListener::bind(bind).await?;
    eprintln!("temper-daemon: serving on {}", listener.local_addr()?);

    axum::serve(listener, daemon.router())
        .with_graceful_shutdown(async {
            if let Err(error) = tokio::signal::ctrl_c().await {
                eprintln!("temper-daemon: failed to listen for shutdown signal: {error}");
            }
        })
        .await
}
