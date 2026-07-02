// SPDX-License-Identifier: MPL-2.0

//! The `ResultApplier` seam and its transport-default implementations.
//!
//! Concrete Forge-backed application lives in [`crate::forge_applier`]; lease
//! gating lives in [`crate::lease_applier`]. This module holds only the trait
//! and the two appliers that need no Forge: the no-op default and the
//! role-routing dispatcher.

use std::{collections::BTreeMap, sync::Arc};

use temper_protocol_worker::{JobResult, PullRequestFreshness, PullRequestFreshnessResponse};

use crate::InFlightJob;

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
    async fn claim(&self, job: InFlightJob) {
        let _ = job;
    }

    async fn apply(&self, job: InFlightJob, result: JobResult);

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
    async fn apply(&self, _job: InFlightJob, _result: JobResult) {}
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
    async fn claim(&self, job: InFlightJob) {
        match self.routes.get(&job.role) {
            Some(applier) => applier.claim(job).await,
            None => self.default.claim(job).await,
        }
    }

    async fn apply(&self, job: InFlightJob, result: JobResult) {
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
