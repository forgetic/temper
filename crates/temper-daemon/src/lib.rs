// SPDX-License-Identifier: MPL-2.0

//! Standalone async daemon transport for the Worker/Daemon wire protocol.

use std::{collections::BTreeMap, net::SocketAddr, sync::Arc, time::Duration};

use axum::{
    body::Bytes,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use temper_forge::{Forge, ForgeError, ItemNumber, RepositoryId, RepositoryPath};
use temper_runner::{scan_role, scan_role_wake, ScanError, WorkItem};
use temper_worker_protocol::{Artifact, ErrorCode, JobResult, Poll, WorkerProtocolMessage};
#[cfg(test)]
use temper_worker_registry::daemon_core::QueuedJob;
use temper_worker_registry::{DaemonCore, InFlightJob};
use temper_workflow::{
    ArtifactSource, CompiledWorkflow, LeaseError, LeaseManager, LeasePolicy, RoleId,
    ValidatedWorkflow,
};
use tokio::{
    sync::{mpsc, oneshot},
    time::{sleep_until, Instant as TokioInstant},
};

pub const DEFAULT_MAX_POLL_WAIT_MS: u64 = 30_000;

/// Which read-only scan the daemon feed runs for a role.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RoleFeedMode {
    /// Steady-state active-queue scan (`scan_role`). This is the default mode.
    #[default]
    Normal,
    /// Wake-triggered scan (`scan_role_wake`).
    Wake,
}

/// Pluggable seam invoked when the daemon accepts a worker `result`.
///
/// The default implementation is a no-op. Use [`LeaseApplier`] to compose a
/// lease-gated Forge decorator around a concrete role-authored applier.
/// Implementations are invoked off the serial core task, so they may perform
/// async I/O without blocking the single-owner `DaemonCore` loop.
#[async_trait::async_trait]
pub trait ResultApplier: Send + Sync {
    async fn apply(&self, job: InFlightJob, result: JobResult);
}

/// Default applier that preserves existing daemon transport behavior.
#[derive(Debug, Default)]
pub struct NoopApplier;

#[async_trait::async_trait]
impl ResultApplier for NoopApplier {
    async fn apply(&self, _job: InFlightJob, _result: JobResult) {}
}

/// Lease-gated [`ResultApplier`] decorator for daemon-owned result application.
///
/// The decorator resolves the completed worker job's Forge artifact, acquires
/// the workflow lease for that `(artifact, role)` as the daemon owner, invokes
/// the inner applier only while that lease is held, and then releases the lease
/// best-effort. Duplicate or double-dispatched results that lose the lease race
/// no-op without disturbing the peer's live lease.
pub struct LeaseApplier<F: Forge> {
    forge: Arc<F>,
    policy: LeasePolicy,
    owner: String,
    inner: Arc<dyn ResultApplier>,
}

impl<F: Forge> LeaseApplier<F> {
    pub fn new(
        forge: Arc<F>,
        policy: LeasePolicy,
        owner: impl Into<String>,
        inner: Arc<dyn ResultApplier>,
    ) -> Self {
        Self {
            forge,
            policy,
            owner: owner.into(),
            inner,
        }
    }
}

#[async_trait::async_trait]
impl<F: Forge + 'static> ResultApplier for LeaseApplier<F> {
    async fn apply(&self, job: InFlightJob, result: JobResult) {
        let Some((repo_id, target)) = resolve_target(self.forge.as_ref(), &job).await else {
            eprintln!(
                "temper-daemon: lease applier could not resolve target for job_id={} repo={} artifact.kind={} artifact.item={}",
                job.job_id, job.repo, job.artifact.kind, job.artifact.item
            );
            return;
        };

        let manager = LeaseManager::new(self.forge.as_ref(), self.policy);
        match manager
            .acquire(
                &repo_id,
                target,
                RoleId::new(job.role.clone()),
                self.owner.clone(),
                Utc::now(),
            )
            .await
        {
            Ok(_) => {}
            Err(LeaseError::Conflict(_) | LeaseError::Contended { .. }) => return,
            Err(error) => {
                eprintln!(
                    "temper-daemon: lease applier could not acquire lease for job_id={} repo={} artifact.kind={} artifact.item={}: {error}",
                    job.job_id, job.repo, job.artifact.kind, job.artifact.item
                );
                return;
            }
        }

        self.inner.apply(job.clone(), result).await;

        if let Err(error) = manager.release(&repo_id, target, &self.owner).await {
            eprintln!(
                "temper-daemon: lease applier could not release lease for job_id={} repo={} artifact.kind={} artifact.item={}: {error}",
                job.job_id, job.repo, job.artifact.kind, job.artifact.item
            );
        }
    }
}

async fn resolve_target<F: Forge + ?Sized>(
    forge: &F,
    job: &InFlightJob,
) -> Option<(RepositoryId, ArtifactSource)> {
    let (owner, name) = job.repo.split_once('/')?;

    let repository = match forge
        .get_repository_by_path(&RepositoryPath::new(owner, name))
        .await
    {
        Ok(Some(repository)) => repository,
        Ok(None) => return None,
        Err(error) => {
            eprintln!(
                "temper-daemon: lease applier repository lookup failed for job_id={} repo={}: {error}",
                job.job_id, job.repo
            );
            return None;
        }
    };

    let number = job.artifact.item.as_u64().map(ItemNumber::new)?;
    let target = match job.artifact.kind.as_str() {
        "issue" => ArtifactSource::Issue { number },
        "pull_request" => ArtifactSource::PullRequest { number },
        _ => return None,
    };

    Some((repository.id, target))
}

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
    cmd_tx: mpsc::UnboundedSender<DaemonCommand>,
    max_poll_wait_ms: u64,
}

#[derive(Clone)]
pub struct Daemon {
    cmd_tx: mpsc::UnboundedSender<DaemonCommand>,
    max_poll_wait_ms: u64,
}

enum DaemonCommand {
    Message {
        msg: WorkerProtocolMessage,
        reply: oneshot::Sender<Option<WorkerProtocolMessage>>,
    },
    Result {
        result: JobResult,
        reply: oneshot::Sender<Option<WorkerProtocolMessage>>,
    },
    Poll {
        poll: Poll,
        deadline: TokioInstant,
        reply: oneshot::Sender<WorkerProtocolMessage>,
    },
    ExpirePoll {
        id: u64,
    },
    EnqueueJob {
        job_id: String,
        role: String,
        repo: String,
        artifact: Artifact,
        job_payload: serde_json::Value,
    },
    #[cfg(test)]
    QueuedJobs {
        reply: oneshot::Sender<Vec<QueuedJob>>,
    },
}

struct PollWaiter {
    poll: Poll,
    reply: oneshot::Sender<WorkerProtocolMessage>,
}

async fn run_core(
    mut rx: mpsc::UnboundedReceiver<DaemonCommand>,
    cmd_tx: mpsc::UnboundedSender<DaemonCommand>,
    applier: Arc<dyn ResultApplier>,
) {
    let mut core = DaemonCore::new();
    let mut waiters = BTreeMap::new();
    let mut next_id = 0_u64;

    while let Some(command) = rx.recv().await {
        match command {
            DaemonCommand::Message { msg, reply } => {
                let _ = reply.send(core.handle(msg));
            }
            DaemonCommand::Result { result, reply } => {
                // Capture full job context before the core completes and forgets the job.
                let in_flight = core.in_flight_job(&result.job_id);
                let response = core.handle(WorkerProtocolMessage::Result(result.clone()));

                // Apply only when the core accepted/completed the in-flight job. Unknown,
                // never-assigned, version-mismatched, and double-sent results must not run
                // the applier.
                if let (Some(job), Some(WorkerProtocolMessage::Release(_))) =
                    (in_flight, response.as_ref())
                {
                    let applier = applier.clone();
                    tokio::spawn(async move {
                        applier.apply(job, result).await;
                    });
                }

                let _ = reply.send(response);
            }
            DaemonCommand::Poll {
                poll,
                deadline,
                reply,
            } => {
                let response = core
                    .handle(WorkerProtocolMessage::Poll(poll.clone()))
                    .expect("poll messages produce a response");

                if is_poll_timeout(&response) {
                    let id = next_id;
                    next_id = next_id.wrapping_add(1);
                    waiters.insert(id, PollWaiter { poll, reply });

                    let timer_tx = cmd_tx.clone();
                    tokio::spawn(async move {
                        sleep_until(deadline).await;
                        let _ = timer_tx.send(DaemonCommand::ExpirePoll { id });
                    });
                } else {
                    let _ = reply.send(response);
                }
            }
            DaemonCommand::ExpirePoll { id } => {
                if let Some(waiter) = waiters.remove(&id) {
                    let response = core
                        .handle(WorkerProtocolMessage::Poll(waiter.poll.clone()))
                        .expect("poll messages produce a response");
                    let _ = waiter.reply.send(response);
                }
            }
            DaemonCommand::EnqueueJob {
                job_id,
                role,
                repo,
                artifact,
                job_payload,
            } => {
                core.enqueue_job(job_id, role, repo, artifact, job_payload);
                fulfil_waiters(&mut core, &mut waiters);
            }
            #[cfg(test)]
            DaemonCommand::QueuedJobs { reply } => {
                let _ = reply.send(core.queued_jobs());
            }
        }
    }
}

fn fulfil_waiters(core: &mut DaemonCore, waiters: &mut BTreeMap<u64, PollWaiter>) {
    let ids = waiters.keys().copied().collect::<Vec<_>>();

    for id in ids {
        let Some(waiter) = waiters.get(&id) else {
            continue;
        };

        if waiter.reply.is_closed() {
            waiters.remove(&id);
            continue;
        }

        let response = core
            .handle(WorkerProtocolMessage::Poll(waiter.poll.clone()))
            .expect("poll messages produce a response");

        if is_poll_timeout(&response) {
            continue;
        }

        let waiter = waiters
            .remove(&id)
            .expect("waiter exists after successful poll response");
        let _ = waiter.reply.send(response);
    }
}

fn is_poll_timeout(message: &WorkerProtocolMessage) -> bool {
    matches!(
        message,
        WorkerProtocolMessage::Error(error) if error.code == ErrorCode::PollTimeout
    )
}

impl Daemon {
    pub fn new() -> Self {
        Self::with_applier(Arc::new(NoopApplier))
    }

    pub fn with_applier(applier: Arc<dyn ResultApplier>) -> Self {
        let (cmd_tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(run_core(rx, cmd_tx.clone(), applier));

        Self {
            cmd_tx,
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
        let _ = self.cmd_tx.send(DaemonCommand::EnqueueJob {
            job_id: job_id.into(),
            role: role.into(),
            repo: repo.into(),
            artifact,
            job_payload,
        });
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

    /// Scans `repo` for `role`'s active queue work and enqueues each resulting
    /// `WorkItem` into the daemon for dispatch. Returns the number of scanned
    /// items; the daemon/registry dedupes already-pending or in-flight jobs by
    /// `job_id`, so repeated feeds for an unchanged ready artifact do not
    /// double-dispatch.
    ///
    /// The protocol `repo` label is the artifact repository's `owner/name` path,
    /// matching worker registered capability `repo` values.
    #[allow(clippy::too_many_arguments)]
    pub async fn enqueue_scanned_role_work<F: Forge + ?Sized>(
        &self,
        forge: &F,
        repo: &RepositoryId,
        workflow: &ValidatedWorkflow,
        compiled: &CompiledWorkflow,
        now: DateTime<Utc>,
        role: &RoleId,
        mode: RoleFeedMode,
    ) -> Result<usize, ScanError> {
        let repo_label = repo_label(forge, repo).await?;
        let items: Vec<WorkItem> = match mode {
            RoleFeedMode::Normal => scan_role(forge, repo, workflow, compiled, now, role).await?,
            RoleFeedMode::Wake => {
                scan_role_wake(forge, repo, workflow, compiled, now, role).await?
            }
        };
        for item in &items {
            self.enqueue_work_item(&repo_label, item).await;
        }
        Ok(items.len())
    }

    #[cfg(test)]
    async fn queued_jobs(&self) -> Vec<WorkItemJob> {
        let (reply, rx) = oneshot::channel();
        self.cmd_tx
            .send(DaemonCommand::QueuedJobs { reply })
            .expect("daemon core task is running");

        rx.await
            .expect("daemon core task replies with queued jobs")
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
            cmd_tx: self.cmd_tx.clone(),
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

/// Resolves the `owner/name` protocol repo label for a scanned `RepositoryId`.
async fn repo_label<F: Forge + ?Sized>(
    forge: &F,
    repo: &RepositoryId,
) -> Result<String, ScanError> {
    let repository = forge
        .get_repository(repo)
        .await?
        .ok_or_else(|| ScanError::Forge(ForgeError::NotFound(format!("repository {repo}"))))?;
    Ok(format!("{}/{}", repository.owner, repository.name))
}

async fn handle_message(State(state): State<DaemonState>, body: Bytes) -> Response {
    let Ok(msg) = serde_json::from_slice::<WorkerProtocolMessage>(&body) else {
        return StatusCode::BAD_REQUEST.into_response();
    };

    match msg {
        WorkerProtocolMessage::Poll(poll) => {
            let requested = poll.max_wait_ms.unwrap_or(state.max_poll_wait_ms);
            let wait_ms = requested.min(state.max_poll_wait_ms);
            let deadline = TokioInstant::now() + Duration::from_millis(wait_ms);
            let (reply, rx) = oneshot::channel();

            if state
                .cmd_tx
                .send(DaemonCommand::Poll {
                    poll,
                    deadline,
                    reply,
                })
                .is_err()
            {
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }

            match rx.await {
                Ok(reply) => (StatusCode::OK, Json(reply)).into_response(),
                Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
            }
        }
        WorkerProtocolMessage::Result(result) => {
            let (reply, rx) = oneshot::channel();

            if state
                .cmd_tx
                .send(DaemonCommand::Result { result, reply })
                .is_err()
            {
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }

            match rx.await {
                Ok(Some(reply)) => (StatusCode::OK, Json(reply)).into_response(),
                Ok(None) => StatusCode::NO_CONTENT.into_response(),
                Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
            }
        }
        other => {
            let (reply, rx) = oneshot::channel();

            if state
                .cmd_tx
                .send(DaemonCommand::Message { msg: other, reply })
                .is_err()
            {
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }

            match rx.await {
                Ok(Some(reply)) => (StatusCode::OK, Json(reply)).into_response(),
                Ok(None) => StatusCode::NO_CONTENT.into_response(),
                Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
            }
        }
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
