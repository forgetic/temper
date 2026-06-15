// SPDX-License-Identifier: MPL-2.0

//! The `ResultApplier` seam and its transport-default implementations.
//!
//! Concrete Forge-backed application lives in [`crate::forge_applier`]; lease
//! gating lives in [`crate::lease_applier`]. This module holds only the trait
//! and the two appliers that need no Forge: the no-op default and the
//! role-routing dispatcher.

use std::{collections::BTreeMap, sync::Arc};

use temper_worker_protocol::{JobProgress, JobResult};

use crate::InFlightJob;

/// Pluggable seam invoked when the daemon accepts a worker `result`.
///
/// The default implementation is a no-op. Use [`crate::LeaseApplier`] to compose
/// a lease-gated Forge decorator around a concrete role-authored applier.
/// Implementations are invoked off the serial core task, so they may perform
/// async I/O without blocking the single-owner `DaemonCore` loop.
#[async_trait::async_trait]
pub trait ResultApplier: Send + Sync {
    async fn apply(&self, job: InFlightJob, result: JobResult);

    /// Applies one agent step-progress checkpoint for an in-flight job.
    ///
    /// Default: no-op. Implementations must be **idempotent keyed by
    /// `(correlation_key, step, state)`** — workers fire-and-forget and may
    /// re-deliver after retry or daemon restart.
    async fn apply_progress(&self, job: InFlightJob, progress: JobProgress) {
        let _ = (job, progress);
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
    async fn apply(&self, job: InFlightJob, result: JobResult) {
        match self.routes.get(&job.role) {
            Some(applier) => applier.apply(job, result).await,
            None => self.default.apply(job, result).await,
        }
    }

    async fn apply_progress(&self, job: InFlightJob, progress: JobProgress) {
        match self.routes.get(&job.role) {
            Some(applier) => applier.apply_progress(job, progress).await,
            None => self.default.apply_progress(job, progress).await,
        }
    }
}
