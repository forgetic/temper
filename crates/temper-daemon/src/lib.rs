// SPDX-License-Identifier: MPL-2.0

//! Standalone async daemon transport for the Worker/Daemon wire protocol.

use std::{
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{
    body::Bytes,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use temper_runner::WorkItem;
use temper_worker_protocol::{Artifact, ErrorCode, Poll, WorkerProtocolMessage};
use temper_worker_registry::DaemonCore;
use temper_workflow::ArtifactSource;
use tokio::sync::{Mutex, Notify};

pub const DEFAULT_MAX_POLL_WAIT_MS: u64 = 30_000;

/// Daemon-owned role-decision context serialized into a worker assignment.
/// This starts minimal for Phase 2f-i and can be extended later when the real
/// worker executor consumes richer role-decision inputs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobContext {
    pub role: String,
    pub repo: String,
    pub queue: String,
    pub artifact_kind: String,
}

/// A daemon job derived from a scanned `WorkItem`: exactly the arguments
/// `Daemon::enqueue_job` consumes.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkItemJob {
    pub job_id: String,
    pub role: String,
    pub repo: String,
    pub artifact: Artifact,
    pub job_payload: serde_json::Value,
}

/// Pure translation of a scanned `WorkItem` into a daemon job. No I/O.
pub fn job_from_work_item(repo: &str, item: &WorkItem) -> WorkItemJob {
    let (number, forge_kind) = match item.target {
        ArtifactSource::Issue { number } => (number.get(), "issue"),
        ArtifactSource::PullRequest { number } => (number.get(), "pull_request"),
    };

    let role = item.role.as_str().to_string();
    let queue = item.queue.as_str().to_string();

    let context = JobContext {
        role: role.clone(),
        repo: repo.to_string(),
        queue: queue.clone(),
        artifact_kind: item.kind.as_str().to_string(),
    };

    WorkItemJob {
        job_id: format!("{repo}/{forge_kind}-{number}/{role}/{queue}"),
        role,
        repo: repo.to_string(),
        artifact: Artifact {
            item: json!(number),
            kind: forge_kind.to_string(),
        },
        job_payload: serde_json::to_value(&context).expect("JobContext serializes"),
    }
}

#[derive(Clone)]
struct DaemonState {
    core: Arc<Mutex<DaemonCore>>,
    notify: Arc<Notify>,
    max_poll_wait_ms: u64,
}

#[derive(Clone)]
pub struct Daemon {
    core: Arc<Mutex<DaemonCore>>,
    notify: Arc<Notify>,
    max_poll_wait_ms: u64,
}

impl Daemon {
    pub fn new() -> Self {
        Self {
            core: Arc::new(Mutex::new(DaemonCore::new())),
            notify: Arc::new(Notify::new()),
            max_poll_wait_ms: DEFAULT_MAX_POLL_WAIT_MS,
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

    /// Map a scanned `WorkItem` to a job and enqueue it.
    pub async fn enqueue_work_item(&self, repo: &str, item: &WorkItem) {
        let job = job_from_work_item(repo, item);
        self.enqueue_job(
            job.job_id,
            job.role,
            job.repo,
            job.artifact,
            job.job_payload,
        )
        .await;
    }

    #[cfg(test)]
    async fn queued_jobs(&self) -> Vec<WorkItemJob> {
        let core = self.core.lock().await;
        core.queued_jobs()
            .into_iter()
            .map(|job| WorkItemJob {
                job_id: job.job_id,
                role: job.role,
                repo: job.repo,
                artifact: job.artifact,
                job_payload: job.job_payload,
            })
            .collect()
    }

    pub fn router(&self) -> Router {
        let state = DaemonState {
            core: Arc::clone(&self.core),
            notify: Arc::clone(&self.notify),
            max_poll_wait_ms: self.max_poll_wait_ms,
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

    match msg {
        WorkerProtocolMessage::Poll(poll) => long_poll(&state, poll).await,
        other => {
            let reply = {
                let mut core = state.core.lock().await;
                core.handle(other)
            };

            match reply {
                Some(reply) => (StatusCode::OK, Json(reply)).into_response(),
                None => StatusCode::NO_CONTENT.into_response(),
            }
        }
    }
}

async fn long_poll(state: &DaemonState, poll: Poll) -> Response {
    let requested = poll.max_wait_ms.unwrap_or(state.max_poll_wait_ms);
    let wait_ms = requested.min(state.max_poll_wait_ms);
    let deadline = Instant::now() + Duration::from_millis(wait_ms);

    loop {
        let notified = state.notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();

        let reply = {
            let mut core = state.core.lock().await;
            core.handle(WorkerProtocolMessage::Poll(poll.clone()))
        };

        match reply {
            Some(WorkerProtocolMessage::Error(error)) if error.code == ErrorCode::PollTimeout => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return (StatusCode::OK, Json(WorkerProtocolMessage::Error(error)))
                        .into_response();
                }

                tokio::select! {
                    _ = notified => {}
                    _ = tokio::time::sleep(remaining) => {
                        return (StatusCode::OK, Json(WorkerProtocolMessage::Error(error)))
                            .into_response();
                    }
                }
            }
            Some(reply) => return (StatusCode::OK, Json(reply)).into_response(),
            None => return StatusCode::NO_CONTENT.into_response(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use temper_forge::ItemNumber;
    use temper_workflow::{ArtifactKindId, QueueId, RoleId};

    fn work_item(target: ArtifactSource) -> WorkItem {
        WorkItem {
            queue: QueueId::new("code_ready"),
            role: RoleId::new("engineer"),
            target,
            kind: ArtifactKindId::new("code"),
        }
    }

    #[test]
    fn maps_issue_work_item_to_daemon_job() {
        let item = work_item(ArtifactSource::Issue {
            number: ItemNumber::new(103),
        });

        let job = job_from_work_item("ai/temper", &item);

        assert_eq!(job.job_id, "ai/temper/issue-103/engineer/code_ready");
        assert_eq!(job.role, "engineer");
        assert_eq!(job.repo, "ai/temper");
        assert_eq!(
            job.artifact,
            Artifact {
                item: json!(103),
                kind: "issue".to_string(),
            }
        );
        assert_eq!(
            serde_json::from_value::<JobContext>(job.job_payload).expect("valid JobContext"),
            JobContext {
                role: "engineer".to_string(),
                repo: "ai/temper".to_string(),
                queue: "code_ready".to_string(),
                artifact_kind: "code".to_string(),
            }
        );
    }

    #[test]
    fn maps_pull_request_work_item_to_daemon_job() {
        let item = work_item(ArtifactSource::PullRequest {
            number: ItemNumber::new(42),
        });

        let job = job_from_work_item("ai/temper", &item);

        assert_eq!(job.artifact.kind, "pull_request");
        assert!(job.job_id.contains("/pull_request-42/"));
        assert_eq!(job.artifact.item, json!(42));
    }

    #[test]
    fn work_item_job_mapping_is_deterministic() {
        let item = work_item(ArtifactSource::Issue {
            number: ItemNumber::new(103),
        });

        assert_eq!(
            job_from_work_item("ai/temper", &item),
            job_from_work_item("ai/temper", &item)
        );
    }

    #[tokio::test]
    async fn enqueue_work_item_stores_mapped_job() {
        let daemon = Daemon::new();
        let item = work_item(ArtifactSource::Issue {
            number: ItemNumber::new(103),
        });
        let expected = job_from_work_item("ai/temper", &item);

        daemon.enqueue_work_item("ai/temper", &item).await;

        assert_eq!(daemon.queued_jobs().await, vec![expected]);
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
