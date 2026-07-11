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

use crate::{
    DispatchCoordinator, WorkItem, WorkerPoolAuthConfig, WorkerPoolPolicies, WorkerPoolPolicy,
};

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

#[derive(Debug, Clone, PartialEq)]
pub struct RoleSaturation {
    /// The configured finite concurrency limit that is currently saturated.
    pub concurrency: u32,
    /// Pending `(repo, Artifact)` entries for the role in queue order.
    pub pending: Vec<(String, Artifact)>,
}

#[derive(Debug, Default)]
pub struct DaemonCore {
    coordinator: DispatchCoordinator,
    job_context: BTreeMap<String, (Artifact, serde_json::Value)>,
    worker_auth: WorkerPoolAuthConfig,
    authenticated_workers: BTreeMap<String, String>,
    pool_policies: WorkerPoolPolicies,
}

impl DaemonCore {
    /// Construct a core with no finite role limits or worker-pool policies.
    pub fn new() -> Self {
        Self::default()
    }

    /// Construct a core with authoritative finite per-role limits.
    pub fn with_role_limits(role_limits: BTreeMap<String, u32>) -> Self {
        Self {
            coordinator: DispatchCoordinator::with_role_limits(role_limits),
            ..Self::default()
        }
    }

    /// Construct a core with worker-pool policies and unlimited roles.
    pub fn with_pool_policies(policies: Vec<WorkerPoolPolicy>) -> Self {
        Self::with_pool_policies_and_role_limits(policies, BTreeMap::new())
    }

    /// Construct a core with both worker-pool policies and authoritative finite
    /// per-role limits.
    pub fn with_pool_policies_and_role_limits(
        policies: Vec<WorkerPoolPolicy>,
        role_limits: BTreeMap<String, u32>,
    ) -> Self {
        Self {
            coordinator: DispatchCoordinator::with_role_limits(role_limits),
            pool_policies: WorkerPoolPolicies::from(policies),
            ..Self::default()
        }
    }

    /// All configured finite role limits. An absent role is unlimited.
    pub fn configured_role_limits(&self) -> &BTreeMap<String, u32> {
        self.coordinator.configured_role_limits()
    }

    /// The configured finite limit for `role`, or `None` when it is unlimited.
    pub fn configured_role_limit(&self, role: &str) -> Option<u32> {
        self.coordinator.configured_role_limit(role)
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

    /// Reports finite configured saturation for `role` and the work waiting
    /// behind that limit.
    ///
    /// Saturation exists only when the role has a configured finite limit, its
    /// assigned count is at least that limit, and same-role work remains
    /// pending. This definition intentionally handles a zero limit with no
    /// in-flight holder and never infers saturation for unlimited roles from
    /// worker exhaustion.
    pub fn role_saturation(&self, role: &str) -> Option<RoleSaturation> {
        let concurrency = self.configured_role_limit(role)?;
        let assigned = self
            .coordinator
            .assigned_work_items()
            .filter(|item| item.role == role)
            .count();
        if assigned < concurrency as usize {
            return None;
        }

        let pending = self
            .coordinator
            .pending()
            .iter()
            .filter(|item| item.role == role)
            .filter_map(|item| {
                let (artifact, _payload) = self.job_context.get(&item.job_id)?;
                Some((item.repo.clone(), artifact.clone()))
            })
            .collect::<Vec<_>>();
        (!pending.is_empty()).then_some(RoleSaturation {
            concurrency,
            pending,
        })
    }

    /// Number of in-flight (assigned, not yet completed) jobs for a role.
    ///
    /// This is an observed count only; configured concurrency is exposed by
    /// [`Self::configured_role_limit`] and must not be inferred from this value.
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
            return Ok(Some(error_response(
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
                Ok(Some(error_response(
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
        match self
            .coordinator
            .registry_mut()
            .register_with_policies(&register, &self.pool_policies)
        {
            Ok(()) => {
                if let Some(pool) = pool {
                    self.authenticated_workers
                        .insert(register.worker_id.clone(), pool);
                }
                Ok(None)
            }
            Err(error) => Ok(Some(error_response(
                ErrorCode::RegistrationRejected,
                &error.to_string(),
                None,
            ))),
        }
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
            return error_response(ErrorCode::UnknownWorker, "unknown worker", None);
        }

        let Some(assignment) = self.coordinator.dispatch_for_worker(&poll.worker_id) else {
            return error_response(ErrorCode::PollTimeout, "no work available", None);
        };

        let Some((artifact, job_payload)) = self.job_context.get(&assignment.job_id).cloned()
        else {
            let _ = self.coordinator.complete(&assignment.job_id);
            return error_response(
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
            Err(_) => Some(error_response(
                ErrorCode::UnknownWorker,
                "unknown worker",
                None,
            )),
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
        .worker_pool
        .as_deref()
        .map(str::trim)
        .filter(|pool| !pool.is_empty())
        .map(str::to_string)
        .or_else(|| {
            register
                .labels
                .as_deref()?
                .iter()
                .filter_map(|label| label.trim().strip_prefix("pool:"))
                .map(str::trim)
                .find(|pool| !pool.is_empty())
                .map(str::to_string)
        })
}

fn auth_error(message: &str) -> WorkerProtocolMessage {
    error_response(ErrorCode::Unauthorized, message, None)
}

fn error_response(code: ErrorCode, message: &str, job_id: Option<String>) -> WorkerProtocolMessage {
    WorkerProtocolMessage::Error(ProtocolError {
        protocol_version: WORKER_PROTOCOL_VERSION,
        code,
        message: message.to_string(),
        retry_after_ms: None,
        job_id,
    })
}

#[cfg(test)]
#[path = "daemon_core_auth_tests.rs"]
mod daemon_core_auth_tests;

#[cfg(test)]
#[path = "daemon_core_tests.rs"]
mod daemon_core_tests;
