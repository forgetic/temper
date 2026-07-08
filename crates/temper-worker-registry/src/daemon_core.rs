// SPDX-License-Identifier: MPL-2.0

//! Pure daemon-side Worker Protocol handling.
//!
//! `DaemonCore` maps already-received worker protocol DTOs to the in-memory
//! dispatch coordinator and returns response DTOs. It intentionally performs no
//! networking, async work, I/O, clock reads, sleeps, or transport-level
//! long-poll waiting; callers are responsible for transport behavior.

use std::collections::{BTreeMap, BTreeSet};

use temper_protocol_worker::{
    Artifact, Assign, ErrorCode, Heartbeat, JobResult, LeaseAck, Poll, ProtocolError, Register,
    Release, ReleaseDisposition, WORKER_PROTOCOL_VERSION, WorkerAuth, WorkerProtocolMessage,
};

use crate::{DispatchCoordinator, WorkItem, WorkerPoolAuthConfig};

#[derive(Debug, Clone, PartialEq)]
pub struct QueuedJob {
    pub job_id: String,
    pub role: String,
    pub repo: String,
    pub artifact: Artifact,
    pub job_payload: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct InFlightJob {
    pub job_id: String,
    pub role: String,
    pub repo: String,
    pub artifact: Artifact,
    pub job_payload: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerAuthError {
    message: String,
}

impl WorkerAuthError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

#[derive(Debug, Default)]
pub struct DaemonCore {
    coordinator: DispatchCoordinator,
    job_context: BTreeMap<String, (Artifact, serde_json::Value)>,
    worker_auth: WorkerPoolAuthConfig,
    authenticated_workers: BTreeMap<String, String>,
}

impl DaemonCore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn coordinator(&self) -> &DispatchCoordinator {
        &self.coordinator
    }

    pub fn coordinator_mut(&mut self) -> &mut DispatchCoordinator {
        &mut self.coordinator
    }

    pub fn configure_worker_pool_auth(&mut self, config: WorkerPoolAuthConfig) {
        self.worker_auth = config;
        self.authenticated_workers.clear();
    }

    pub fn worker_pool_auth(&self) -> &WorkerPoolAuthConfig {
        &self.worker_auth
    }

    pub fn enqueue_job(
        &mut self,
        job_id: impl Into<String>,
        role: impl Into<String>,
        repo: impl Into<String>,
        artifact: Artifact,
        job_payload: serde_json::Value,
    ) {
        let job_id = job_id.into();
        let role = role.into();
        let repo = repo.into();
        let repos = manifest_repos(&job_payload, &repo);

        self.coordinator.enqueue(WorkItem {
            job_id: job_id.clone(),
            role,
            coordination_key: payload_coordination_key(&job_payload)
                .map(str::trim)
                .filter(|key| !key.is_empty())
                .map(str::to_string),
            repo,
            repos,
        });
        self.job_context.insert(job_id, (artifact, job_payload));
    }

    pub fn queued_jobs(&self) -> Vec<QueuedJob> {
        self.coordinator
            .pending()
            .iter()
            .filter_map(|item| {
                let (artifact, job_payload) = self.job_context.get(&item.job_id)?.clone();
                Some(QueuedJob {
                    job_id: item.job_id.clone(),
                    role: item.role.clone(),
                    repo: item.repo.clone(),
                    artifact,
                    job_payload,
                })
            })
            .collect()
    }

    /// Reconcile pending daemon jobs for one `(repo, role)` scan scope.
    ///
    /// Only pending jobs are pruned. Assigned/in-flight jobs remain in the
    /// dispatch coordinator and keep their job context so result handling can
    /// complete normally.
    pub fn retain_pending_jobs_for_scope(
        &mut self,
        repo: &str,
        role: &str,
        current_job_ids: &BTreeSet<String>,
    ) -> Vec<String> {
        let removed = self
            .coordinator
            .retain_pending_by_scope(repo, role, current_job_ids);
        let mut job_ids = Vec::with_capacity(removed.len());
        for item in removed {
            self.job_context.remove(&item.job_id);
            job_ids.push(item.job_id);
        }
        job_ids
    }

    /// Reports whether a role is saturated and, if so, what is queued behind it.
    ///
    /// A role is *saturated* when it has at least one in-flight (assigned) job
    /// and at least one job still pending in the same role's queue — i.e. work
    /// is waiting because the role's worker(s) are busy. The returned vector
    /// holds the `(repo, artifact)` coordinates of the pending same-role jobs in
    /// queue order, which the observability layer renders into the §7
    /// `role.saturated` wait list. An empty result means the role is not
    /// saturated (idle, or pending work but no in-flight holder).
    ///
    /// This is a pure read over the dispatch coordinator's pending/assigned
    /// sets; the caller owns the structured-event emission (this crate has no
    /// logging dependency).
    pub fn role_saturation(&self, role: &str) -> Vec<(String, Artifact)> {
        let coordinator = &self.coordinator;
        let role_busy = coordinator
            .assigned_work_items()
            .any(|item| item.role == role);
        if !role_busy {
            return Vec::new();
        }
        coordinator
            .pending()
            .iter()
            .filter(|item| item.role == role)
            .filter_map(|item| {
                let (artifact, _payload) = self.job_context.get(&item.job_id)?;
                Some((item.repo.clone(), artifact.clone()))
            })
            .collect()
    }

    /// Number of in-flight (assigned, not yet completed) jobs for a role.
    ///
    /// For the single-slot standalone roles this is the role's effective
    /// concurrency limit while it is busy (the `(concurrency=N)` figure of the
    /// §7 `role.saturated` line).
    pub fn in_flight_role_count(&self, role: &str) -> usize {
        self.coordinator
            .assigned_work_items()
            .filter(|item| item.role == role)
            .count()
    }

    /// Full context of a currently in-flight (assigned, not yet completed) job,
    /// recoverable until `handle(Result)` completes it. `None` if the job is
    /// pending (not yet dispatched), unknown, or already completed.
    pub fn in_flight_job(&self, job_id: &str) -> Option<InFlightJob> {
        let item = self.coordinator.assigned_work_item(job_id)?;
        let (artifact, job_payload) = self.job_context.get(job_id)?.clone();
        Some(InFlightJob {
            job_id: job_id.to_string(),
            role: item.role.clone(),
            repo: item.repo.clone(),
            artifact,
            job_payload,
        })
    }

    /// Whether any pending or assigned job is still known for this workspace
    /// correlation key.
    pub fn workstream_active_by_correlation_key(&self, correlation_key: &str) -> bool {
        let correlation_key = correlation_key.trim();
        if correlation_key.is_empty() {
            return false;
        }
        self.job_context.iter().any(|(_, (_, payload))| {
            payload_coordination_key(payload)
                .map(str::trim)
                .filter(|key| !key.is_empty())
                == Some(correlation_key)
        })
    }

    pub fn handle(&mut self, msg: WorkerProtocolMessage) -> Option<WorkerProtocolMessage> {
        match self.handle_authenticated(msg, None) {
            Ok(response) => response,
            Err(error) => Some(auth_error(error.message())),
        }
    }

    pub fn handle_authenticated(
        &mut self,
        msg: WorkerProtocolMessage,
        auth: Option<&WorkerAuth>,
    ) -> Result<Option<WorkerProtocolMessage>, WorkerAuthError> {
        if protocol_version(&msg) != WORKER_PROTOCOL_VERSION {
            return Ok(Some(error(
                ErrorCode::ProtocolVersionMismatch,
                "unsupported protocol_version",
                None,
            )));
        }

        match msg {
            WorkerProtocolMessage::Register(register) => self.handle_register(register, auth),
            WorkerProtocolMessage::Poll(poll) => {
                self.authenticate_registered_worker(&poll.worker_id, None, auth)?;
                Ok(Some(self.handle_poll(poll)))
            }
            WorkerProtocolMessage::Assign(_) | WorkerProtocolMessage::Release(_) => {
                Ok(Some(error(
                    ErrorCode::MalformedMessage,
                    "daemon-to-worker message received inbound",
                    None,
                )))
            }
            WorkerProtocolMessage::Heartbeat(heartbeat) => {
                self.authenticate_registered_worker(
                    &heartbeat.worker_id,
                    heartbeat.worker_pool.as_deref(),
                    auth,
                )?;
                Ok(self.handle_heartbeat(heartbeat))
            }
            WorkerProtocolMessage::Result(result) => {
                self.authenticate_registered_worker(&result.worker_id, None, auth)?;
                Ok(Some(self.handle_result(result)))
            }
            WorkerProtocolMessage::LeaseAck(lease_ack) => Ok(self.handle_lease_ack(lease_ack)),
            WorkerProtocolMessage::Error(_) => Ok(None),
        }
    }

    fn handle_register(
        &mut self,
        register: Register,
        auth: Option<&WorkerAuth>,
    ) -> Result<Option<WorkerProtocolMessage>, WorkerAuthError> {
        let pool = self.authenticate_register(&register, auth)?;
        self.coordinator.register(&register);
        if let Some(pool) = pool {
            self.authenticated_workers
                .insert(register.worker_id.clone(), pool);
        }
        Ok(None)
    }

    fn authenticate_register(
        &self,
        register: &Register,
        auth: Option<&WorkerAuth>,
    ) -> Result<Option<String>, WorkerAuthError> {
        if !self.worker_auth.is_enabled() {
            return Ok(None);
        }
        let pool = register_pool(register).ok_or_else(|| {
            WorkerAuthError::new(
                "worker pool authentication is configured; register message must declare a worker pool",
            )
        })?;
        self.authenticate_pool_credential(&pool, auth)?;
        Ok(Some(pool))
    }

    fn authenticate_registered_worker(
        &self,
        worker_id: &str,
        message_pool: Option<&str>,
        auth: Option<&WorkerAuth>,
    ) -> Result<(), WorkerAuthError> {
        if !self.worker_auth.is_enabled() {
            return Ok(());
        }
        let pool = self.authenticated_workers.get(worker_id).ok_or_else(|| {
            WorkerAuthError::new(format!(
                "worker `{worker_id}` is not authenticated to a registered worker pool"
            ))
        })?;
        if let Some(message_pool) = message_pool.map(str::trim).filter(|pool| !pool.is_empty()) {
            if message_pool != pool {
                return Err(WorkerAuthError::new(format!(
                    "worker `{worker_id}` sent worker_pool `{message_pool}` but registered to `{pool}`"
                )));
            }
        }
        self.authenticate_pool_credential(pool, auth)
    }

    fn authenticate_pool_credential(
        &self,
        pool: &str,
        auth: Option<&WorkerAuth>,
    ) -> Result<(), WorkerAuthError> {
        let expected = self.worker_auth.pool_token(pool).ok_or_else(|| {
            WorkerAuthError::new(format!("worker pool `{pool}` is not configured"))
        })?;
        match (expected.as_ref(), auth) {
            (Some(expected), Some(presented)) if expected.matches(presented) => Ok(()),
            (Some(_), None) => Err(WorkerAuthError::new(format!(
                "worker pool `{pool}` requires worker_token authentication"
            ))),
            (Some(_), Some(_)) => Err(WorkerAuthError::new(format!(
                "worker pool `{pool}` worker_token authentication failed"
            ))),
            (None, None) => Ok(()),
            (None, Some(_)) => Err(WorkerAuthError::new(format!(
                "worker pool `{pool}` does not accept worker_token authentication"
            ))),
        }
    }

    fn handle_poll(&mut self, poll: Poll) -> WorkerProtocolMessage {
        if !self.coordinator.registry().is_healthy(&poll.worker_id) {
            return error(ErrorCode::UnknownWorker, "unknown worker", None);
        }

        let Some(assignment) = self.coordinator.dispatch_for_worker(&poll.worker_id) else {
            return error(ErrorCode::PollTimeout, "no work available", None);
        };

        let Some((artifact, job_payload)) = self.job_context.get(&assignment.job_id).cloned()
        else {
            let _ = self.coordinator.complete(&assignment.job_id);
            return error(
                ErrorCode::MalformedMessage,
                "assigned job missing daemon job context",
                Some(assignment.job_id),
            );
        };

        WorkerProtocolMessage::Assign(Assign {
            protocol_version: WORKER_PROTOCOL_VERSION,
            job_id: assignment.job_id,
            role: assignment.role,
            repo: assignment.repo,
            artifact,
            job_payload,
        })
    }

    fn handle_heartbeat(&mut self, heartbeat: Heartbeat) -> Option<WorkerProtocolMessage> {
        match self
            .coordinator
            .registry_mut()
            .heartbeat(&heartbeat.worker_id)
        {
            Ok(()) => None,
            Err(_) => Some(error(ErrorCode::UnknownWorker, "unknown worker", None)),
        }
    }

    fn handle_result(&mut self, result: JobResult) -> WorkerProtocolMessage {
        let _ = self.coordinator.complete(&result.job_id);
        self.job_context.remove(&result.job_id);

        WorkerProtocolMessage::Release(Release {
            protocol_version: WORKER_PROTOCOL_VERSION,
            worker_id: result.worker_id,
            job_id: result.job_id,
            disposition: ReleaseDisposition::Accepted,
            message: None,
        })
    }

    fn handle_lease_ack(&mut self, _lease_ack: LeaseAck) -> Option<WorkerProtocolMessage> {
        None
    }
}

/// Every repository the assigned worker must be capable of: the manifest's
/// `workspace.repos[*].repo` (ADR 0023), falling back to just the primary repo
/// when the payload carries no manifest. Non-empty by construction.
fn manifest_repos(job_payload: &serde_json::Value, primary: &str) -> Vec<String> {
    let repos = job_payload
        .get("workspace")
        .and_then(|workspace| workspace.get("repos"))
        .and_then(serde_json::Value::as_array)
        .map(|repos| {
            repos
                .iter()
                .filter_map(|repo| repo.get("repo").and_then(serde_json::Value::as_str))
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    if repos.is_empty() {
        vec![primary.to_string()]
    } else {
        repos
    }
}

/// The job's coordination key (`workspace.coordination_key`).
fn payload_coordination_key(job_payload: &serde_json::Value) -> Option<&str> {
    job_payload
        .get("workspace")
        .and_then(|workspace| workspace.get("coordination_key"))
        .and_then(serde_json::Value::as_str)
}

fn protocol_version(msg: &WorkerProtocolMessage) -> u32 {
    match msg {
        WorkerProtocolMessage::Register(msg) => msg.protocol_version,
        WorkerProtocolMessage::Poll(msg) => msg.protocol_version,
        WorkerProtocolMessage::Assign(msg) => msg.protocol_version,
        WorkerProtocolMessage::Heartbeat(msg) => msg.protocol_version,
        WorkerProtocolMessage::Result(msg) => msg.protocol_version,
        WorkerProtocolMessage::Release(msg) => msg.protocol_version,
        WorkerProtocolMessage::LeaseAck(msg) => msg.protocol_version,
        WorkerProtocolMessage::Error(msg) => msg.protocol_version,
    }
}

fn register_pool(register: &Register) -> Option<String> {
    register
        .labels
        .as_deref()?
        .iter()
        .filter_map(|label| label.trim().strip_prefix("pool:"))
        .map(str::trim)
        .find(|pool| !pool.is_empty())
        .map(str::to_string)
}

fn auth_error(message: &str) -> WorkerProtocolMessage {
    error(ErrorCode::Unauthorized, message, None)
}

fn error(code: ErrorCode, message: &str, job_id: Option<String>) -> WorkerProtocolMessage {
    WorkerProtocolMessage::Error(ProtocolError {
        protocol_version: WORKER_PROTOCOL_VERSION,
        code,
        message: message.to_string(),
        retry_after_ms: None,
        job_id,
    })
}

#[cfg(test)]
#[path = "daemon_core_tests.rs"]
mod daemon_core_tests;
