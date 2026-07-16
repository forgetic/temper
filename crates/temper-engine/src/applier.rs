// SPDX-License-Identifier: MPL-2.0

//! The `ResultApplier` seam and its transport-default implementations.
//!
//! Concrete Forge-backed application lives in [`crate::forge_applier`]; lease
//! gating lives in [`crate::lease_applier`]. This module holds only the trait
//! and the two appliers that need no Forge: the no-op default and the
//! role-routing dispatcher.

use std::{collections::BTreeMap, sync::Arc};

use temper_protocol_worker::{
    FailureClass, JobResult, PullRequestFreshness, PullRequestFreshnessResponse,
};

use temper_workflow::AssignmentMutation;

use crate::InFlightJob;

/// Identity supplied by the daemon for one assignment claim attempt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClaimContext {
    pub worker_id: String,
    pub daemon_boot_id: String,
}

/// Typed result of attempting to durably claim work before publication.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ClaimOutcome {
    Claimed,
    Contended { reason: String },
    Stale { reason: String },
    Retryable { reason: String },
}

/// Typed result of applying an accepted worker result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ApplyOutcome {
    Applied,
    /// Retry bookkeeping and exact claim release completed successfully.
    RetryReleased,
    Stale,
    Retryable {
        reason: String,
    },
    Rejected {
        class: FailureClass,
        reason: String,
    },
}

/// Pluggable seam invoked when the daemon assigns work and accepts worker
/// results.
///
/// The default implementation is a no-op. Use [`crate::LeaseApplier`] to compose
/// a lease-gated Forge decorator around a concrete role-authored applier.
/// Implementations are invoked off the serial core task, so they may perform
/// async I/O without blocking the single-owner `DaemonCore` loop.
#[async_trait::async_trait]
pub trait ResultApplier: Send + Sync {
    /// Applies assignment-time source-artifact claim signals before the worker
    /// receives its `Assign` response. Implementations that do not manage Forge
    /// workflow state can leave the default no-op in place.
    async fn claim(&self, job: InFlightJob, context: ClaimContext) -> ClaimOutcome {
        let _ = (job, context);
        ClaimOutcome::Claimed
    }

    /// Computes lifecycle fields that must be committed in the same Forge CAS
    /// as the durable assignment metadata.
    async fn assignment_mutation(&self, job: &InFlightJob) -> AssignmentMutation {
        let _ = job;
        AssignmentMutation::default()
    }

    async fn release_claim(&self, job: InFlightJob, context: ClaimContext) {
        let _ = (job, context);
    }

    /// Reattaches and refreshes a recovered assignment after the recorded worker
    /// proves ownership by reporting the exact job id in a heartbeat.
    async fn heartbeat(&self, job: InFlightJob, context: ClaimContext) {
        let _ = (job, context);
    }

    /// Applies a matching result for an assignment reconstructed during daemon
    /// startup. Lease-backed implementations override this to reattach the
    /// exact durable claim before performing any result mutation. The default
    /// keeps non-durable appliers source compatible.
    async fn apply_recovered(
        &self,
        job: InFlightJob,
        result: JobResult,
        context: ClaimContext,
    ) -> ApplyOutcome {
        self.heartbeat(job.clone(), context).await;
        self.apply(job, result).await
    }

    async fn apply(&self, job: InFlightJob, result: JobResult) -> ApplyOutcome;

    /// Validates whether a PR-targeted in-flight job may still publish work.
    async fn check_pull_request_freshness(
        &self,
        check: PullRequestFreshness,
    ) -> PullRequestFreshnessResponse {
        let _ = check;
        PullRequestFreshnessResponse::unavailable("pull request freshness checks are unavailable")
    }
}

/// Default applier that preserves existing daemon transport behavior.
#[derive(Debug, Default)]
pub struct NoopApplier;

#[async_trait::async_trait]
impl ResultApplier for NoopApplier {
    async fn apply(&self, _job: InFlightJob, _result: JobResult) -> ApplyOutcome {
        ApplyOutcome::Applied
    }
}

/// Routes each applied result to the applier registered for the job's role,
/// falling back to the default applier for unknown roles.
pub struct RoleRoutingApplier {
    routes: BTreeMap<String, Arc<dyn ResultApplier>>,
    default: Arc<dyn ResultApplier>,
}

impl RoleRoutingApplier {
    pub fn new(default: Arc<dyn ResultApplier>) -> Self {
        Self {
            routes: BTreeMap::new(),
            default,
        }
    }

    pub fn with_route(mut self, role: impl Into<String>, applier: Arc<dyn ResultApplier>) -> Self {
        self.routes.insert(role.into(), applier);
        self
    }
}

#[async_trait::async_trait]
impl ResultApplier for RoleRoutingApplier {
    async fn claim(&self, job: InFlightJob, context: ClaimContext) -> ClaimOutcome {
        match self.routes.get(&job.role) {
            Some(applier) => applier.claim(job, context).await,
            None => self.default.claim(job, context).await,
        }
    }

    async fn assignment_mutation(&self, job: &InFlightJob) -> AssignmentMutation {
        match self.routes.get(&job.role) {
            Some(applier) => applier.assignment_mutation(job).await,
            None => self.default.assignment_mutation(job).await,
        }
    }

    async fn release_claim(&self, job: InFlightJob, context: ClaimContext) {
        match self.routes.get(&job.role) {
            Some(applier) => applier.release_claim(job, context).await,
            None => self.default.release_claim(job, context).await,
        }
    }

    async fn heartbeat(&self, job: InFlightJob, context: ClaimContext) {
        match self.routes.get(&job.role) {
            Some(applier) => applier.heartbeat(job, context).await,
            None => self.default.heartbeat(job, context).await,
        }
    }

    async fn apply_recovered(
        &self,
        job: InFlightJob,
        result: JobResult,
        context: ClaimContext,
    ) -> ApplyOutcome {
        match self.routes.get(&job.role) {
            Some(applier) => applier.apply_recovered(job, result, context).await,
            None => self.default.apply_recovered(job, result, context).await,
        }
    }

    async fn apply(&self, job: InFlightJob, result: JobResult) -> ApplyOutcome {
        match self.routes.get(&job.role) {
            Some(applier) => applier.apply(job, result).await,
            None => self.default.apply(job, result).await,
        }
    }

    async fn check_pull_request_freshness(
        &self,
        check: PullRequestFreshness,
    ) -> PullRequestFreshnessResponse {
        match self.routes.get(&check.role) {
            Some(applier) => applier.check_pull_request_freshness(check).await,
            None => self.default.check_pull_request_freshness(check).await,
        }
    }
}
