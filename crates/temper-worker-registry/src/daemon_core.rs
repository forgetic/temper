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
    Assignment, DispatchCoordinator, RegistryError, WorkItem, WorkerPoolAuthConfig,
    WorkerPoolPolicies, WorkerPoolPolicy,
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

#[derive(Debug, Clone, PartialEq)]
pub struct RecoveredJob {
    pub job_id: String,
    pub worker_id: String,
    pub role: String,
    pub repo: String,
    pub artifact: Artifact,
    pub job_payload: serde_json::Value,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HeartbeatRecovery {
    /// Durable jobs that match this worker. Includes jobs reattached by this
    /// heartbeat and already-reattached jobs refreshed by later heartbeats.
    pub matched_job_ids: Vec<String>,
    /// Job ids reported by the worker that were unknown or belonged to another
    /// worker. These ids must never extend a durable lease.
    pub rejected_job_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StagedRecovery {
    worker_id: String,
    item: WorkItem,
}

#[derive(Debug, Default)]
pub struct DaemonCore {
    coordinator: DispatchCoordinator,
    job_context: BTreeMap<String, (Artifact, serde_json::Value)>,
    staged_recovery: BTreeMap<String, StagedRecovery>,
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

    /// Stages one assignment reconstructed from durable Forge metadata.
    ///
    /// A staged job is intentionally not dispatchable. It becomes in-flight
    /// only when its recorded worker reports the same job id in a heartbeat;
    /// this is the prior-boot ownership proof used during the bounded startup
    /// grace period.
    pub fn stage_recovered_job(&mut self, recovered: RecoveredJob) -> Result<(), RegistryError> {
        let repos = manifest_repos(&recovered.job_payload, &recovered.repo);
        let item = WorkItem {
            job_id: recovered.job_id.clone(),
            role: recovered.role,
            coordination_key: payload_coordination_key(&recovered.job_payload)
                .map(str::trim)
                .filter(|key| !key.is_empty())
                .map(str::to_string),
            repo: recovered.repo,
            repos,
        };
        let staged = StagedRecovery {
            worker_id: recovered.worker_id,
            item,
        };
        if let Some(current) = self.staged_recovery.get(&recovered.job_id) {
            return if current == &staged {
                Ok(())
            } else {
                Err(RegistryError::DuplicateJob(recovered.job_id))
            };
        }
        if self
            .coordinator
            .assigned_work_item(&recovered.job_id)
            .is_some()
            || self.job_context.contains_key(&recovered.job_id)
        {
            return Err(RegistryError::DuplicateJob(recovered.job_id));
        }
        self.job_context.insert(
            recovered.job_id.clone(),
            (recovered.artifact, recovered.job_payload),
        );
        self.staged_recovery.insert(recovered.job_id, staged);
        Ok(())
    }

    /// Drops every staged job that failed to prove prior-boot ownership. The
    /// returned contexts are used by startup recovery to clear their durable
    /// claims and converge them to a safe workflow state.
    pub fn take_unreattached_recovered_jobs(&mut self) -> Vec<RecoveredJob> {
        let staged = std::mem::take(&mut self.staged_recovery);
        staged
            .into_iter()
            .filter_map(|(job_id, staged)| {
                let (artifact, job_payload) = self.job_context.remove(&job_id)?;
                Some(RecoveredJob {
                    job_id,
                    worker_id: staged.worker_id,
                    role: staged.item.role,
                    repo: staged.item.repo,
                    artifact,
                    job_payload,
                })
            })
            .collect()
    }

    pub fn staged_recovery_len(&self) -> usize {
        self.staged_recovery.len()
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

    /// Authenticates a worker and proves that `job_id` is its currently active
    /// assignment. Pending, staged, completed, released, unknown, and
    /// wrong-worker jobs all return `None` after authentication.
    pub fn authorize_context_read(
        &self,
        worker_id: &str,
        job_id: &str,
        auth: Option<&WorkerAuth>,
    ) -> Result<Option<InFlightJob>, WorkerAuthError> {
        self.authenticate_registered_worker(worker_id, None, auth)?;
        if self.coordinator.assigned_worker(job_id) != Some(worker_id) {
            return Ok(None);
        }
        Ok(self.in_flight_job(job_id))
    }

    /// Reserve one job for a poller without making it externally in-flight.
    pub fn reserve_authenticated_poll(
        &mut self,
        poll: Poll,
        auth: Option<&WorkerAuth>,
    ) -> Result<WorkerProtocolMessage, WorkerAuthError> {
        self.authenticate_registered_worker(&poll.worker_id, None, auth)?;
        Ok(self.reserve_poll(poll))
    }

    /// Commit a durable-claim-backed reservation as the visible assignment.
    pub fn commit_assignment(&mut self, job_id: &str) -> Result<(), RegistryError> {
        self.coordinator.commit_reservation(job_id).map(|_| ())
    }

    /// Restore a failed or canceled claim to the front of the pending queue.
    pub fn rollback_assignment(&mut self, job_id: &str) -> bool {
        self.coordinator.rollback_reservation(job_id)
    }

    pub fn rollback_committed_assignment(&mut self, job_id: &str) -> bool {
        self.coordinator.rollback_committed(job_id)
    }

    /// Full context for an assignment reservation or committed assignment.
    pub fn job_context(&self, job_id: &str) -> Option<(Artifact, serde_json::Value)> {
        self.job_context.get(job_id).cloned()
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

    pub fn in_flight_jobs(&self) -> Vec<InFlightJob> {
        self.coordinator
            .assigned_work_items()
            .filter_map(|item| self.in_flight_job(&item.job_id))
            .collect()
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
            WorkerProtocolMessage::Heartbeat(heartbeat) => self
                .handle_authenticated_heartbeat(heartbeat, auth)
                .map(|(response, _recovery)| response),
            WorkerProtocolMessage::Result(result) => {
                self.authenticate_registered_worker(&result.worker_id, None, auth)?;
                Ok(Some(self.handle_result(result)))
            }
            WorkerProtocolMessage::LeaseAck(lease_ack) => Ok(self.handle_lease_ack(lease_ack)),
            WorkerProtocolMessage::FetchContext(_) | WorkerProtocolMessage::ContextResponse(_) => {
                Ok(Some(error_response(
                    ErrorCode::MalformedMessage,
                    "context messages are handled by the daemon transport",
                    None,
                )))
            }
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

    fn reserve_poll(&mut self, poll: Poll) -> WorkerProtocolMessage {
        if !self.coordinator.registry().is_healthy(&poll.worker_id) {
            return error_response(ErrorCode::UnknownWorker, "unknown worker", None);
        }

        let Some(assignment) = self.coordinator.reserve_for_worker(&poll.worker_id) else {
            return error_response(ErrorCode::PollTimeout, "no work available", None);
        };

        let Some((artifact, job_payload)) = self.job_context.get(&assignment.job_id).cloned()
        else {
            self.coordinator.rollback_reservation(&assignment.job_id);
            return error_response(
                ErrorCode::MalformedMessage,
                "reserved job missing daemon job context",
                Some(assignment.job_id),
            );
        };
        assignment_message(assignment, artifact, job_payload)
    }

    fn handle_poll(&mut self, poll: Poll) -> WorkerProtocolMessage {
        let response = self.reserve_poll(poll);
        let WorkerProtocolMessage::Assign(assign) = &response else {
            return response;
        };
        if self.coordinator.commit_reservation(&assign.job_id).is_err() {
            return error_response(
                ErrorCode::CapacityExceeded,
                "assignment reservation could not be committed",
                Some(assign.job_id.clone()),
            );
        }
        response
    }

    /// Authenticates and applies a heartbeat while exposing exact durable-job
    /// matches to the daemon lease handler.
    pub fn handle_authenticated_heartbeat(
        &mut self,
        heartbeat: Heartbeat,
        auth: Option<&WorkerAuth>,
    ) -> Result<(Option<WorkerProtocolMessage>, HeartbeatRecovery), WorkerAuthError> {
        self.authenticate_registered_worker(
            &heartbeat.worker_id,
            heartbeat.worker_pool.as_deref(),
            auth,
        )?;
        Ok(self.handle_heartbeat(heartbeat))
    }

    fn handle_heartbeat(
        &mut self,
        heartbeat: Heartbeat,
    ) -> (Option<WorkerProtocolMessage>, HeartbeatRecovery) {
        if self
            .coordinator
            .registry_mut()
            .heartbeat(&heartbeat.worker_id)
            .is_err()
        {
            return (
                Some(error_response(
                    ErrorCode::UnknownWorker,
                    "unknown worker",
                    None,
                )),
                HeartbeatRecovery::default(),
            );
        }

        let mut recovery = HeartbeatRecovery::default();
        for reported in heartbeat.jobs {
            if self.coordinator.assigned_worker(&reported.job_id)
                == Some(heartbeat.worker_id.as_str())
            {
                recovery.matched_job_ids.push(reported.job_id);
                continue;
            }

            let Some(staged) = self.staged_recovery.get(&reported.job_id).cloned() else {
                recovery.rejected_job_ids.push(reported.job_id);
                continue;
            };
            if staged.worker_id != heartbeat.worker_id {
                recovery.rejected_job_ids.push(reported.job_id);
                continue;
            }
            match self
                .coordinator
                .restore_assignment(&staged.worker_id, staged.item)
            {
                Ok(_) => {
                    self.staged_recovery.remove(&reported.job_id);
                    recovery.matched_job_ids.push(reported.job_id);
                }
                Err(_) => recovery.rejected_job_ids.push(reported.job_id),
            }
        }
        (None, recovery)
    }

    fn handle_result(&mut self, result: JobResult) -> WorkerProtocolMessage {
        if self.coordinator.assigned_worker(&result.job_id) != Some(result.worker_id.as_str()) {
            return error_response(
                ErrorCode::MalformedMessage,
                "result does not match the assigned job and worker",
                Some(result.job_id),
            );
        }
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

fn assignment_message(
    assignment: Assignment,
    artifact: Artifact,
    job_payload: serde_json::Value,
) -> WorkerProtocolMessage {
    WorkerProtocolMessage::Assign(Assign {
        protocol_version: WORKER_PROTOCOL_VERSION,
        job_id: assignment.job_id,
        role: assignment.role,
        repo: assignment.repo,
        artifact,
        job_payload,
    })
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
        WorkerProtocolMessage::FetchContext(msg) => msg.protocol_version,
        WorkerProtocolMessage::ContextResponse(msg) => msg.protocol_version,
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
