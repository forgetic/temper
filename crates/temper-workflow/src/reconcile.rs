//! Reconciliation of Forge artifacts and command journals (Phase 7).
//!
//! The runtime cannot assume its own commands always finish: a worker may crash
//! mid-transition, a lease may expire, or a human may edit labels into an
//! impossible combination. The reconciler is the authority that periodically
//! inspects durable state — Forge artifacts plus the [command
//! journal](crate::journal) — and decides what to repair or escalate.
//!
//! # Decide, then apply
//!
//! [`Reconciler::scan`] is pure and deterministic over snapshots, journal
//! records, dependency status, and time (see the [`scan`] submodule).
//! [`Reconciler::reconcile`] is the bounded loader for exact journal targets plus
//! workflow-labelled candidates, and [`Reconciler::reconcile_deep_audit`] is the
//! explicit all-history loader (both in the [`load`] submodule).
//!
//! Applying the chosen actions is the job of
//! [`recover::Applier`](crate::recover::Applier), which routes each action
//! through the existing [`Executor`](crate::execute::Executor),
//! [`LeaseManager`](crate::lease), and [`CommandJournal`](crate::journal)
//! runtime layers. The reconciler itself only decides, so a caller can still
//! review or filter actions before handing the report to the applier.
//!
//! # Recovery policy hooks
//!
//! Durable assignment recovery is fixed to full convergence so it cannot be
//! downgraded to lease-only clearing. [`RecoveryPolicy`] has defaulted hooks for
//! the remaining configurable finding classes, so a workflow can override how
//! it handles expired legacy leases, partial transitions, impossible states,
//! classification drift, stale commands, or resolved dependencies by
//! implementing only the hooks it cares about. [`DefaultRecoveryPolicy`] uses the safe defaults: fully converge expired
//! assignments, requeue expired legacy leases, escalate ambiguous drift, repair
//! partial transitions, mark already-realized commands reconciled, and
//! mechanically unblock dependency-gated work once its prerequisites land.

mod candidate;
mod detail_cache;
mod finding;
mod load;
mod scan;

use crate::classify::ArtifactSource;
use crate::validated::ValidatedWorkflow;
use temper_forge::{Issue, ItemNumber, PullRequest};

pub use candidate::{ReconciliationCandidateQueryPlan, reconciliation_candidate_query_plan};
pub use detail_cache::{
    ReconciliationDetailCache, ReconciliationDetailCachePolicy, ReconciliationDetailCacheStats,
};
pub use finding::{
    DefaultRecoveryPolicy, ReconcileError, ReconcileFinding, ReconcileReport, RecoveryAction,
    RecoveryPolicy,
};

/// A point-in-time view of a Forge artifact used for reconciliation.
///
/// Holds only what reconciliation reads — the source, raw labels, and body — so
/// the pure [`Reconciler::scan`] needs no backend handles.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtifactSnapshot {
    /// Where the artifact lives in the Forge.
    pub source: ArtifactSource,
    /// Raw Forge labels present on the artifact.
    pub labels: Vec<String>,
    /// The artifact body, which may carry a workflow metadata block.
    pub body: String,
    /// Native dependency links read from the Forge artifact record.
    pub dependencies: Vec<ItemNumber>,
}

impl ArtifactSnapshot {
    /// Builds a snapshot from a Forge issue.
    pub fn from_issue(issue: &Issue) -> Self {
        Self {
            source: ArtifactSource::Issue {
                number: issue.number,
            },
            labels: issue.labels.clone(),
            body: issue.body.clone(),
            dependencies: issue.dependencies.clone(),
        }
    }

    /// Builds a snapshot from a Forge pull request.
    pub fn from_pull_request(pull_request: &PullRequest) -> Self {
        Self {
            source: ArtifactSource::PullRequest {
                number: pull_request.number,
            },
            labels: pull_request.labels.clone(),
            body: pull_request.body.clone(),
            dependencies: pull_request.dependencies.clone(),
        }
    }
}

/// How a runtime loads reconciliation snapshots before calling [`Reconciler::scan`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReconciliationMode {
    /// Load bounded runtime inputs.
    Bounded,
    /// Load every visible issue and pull request with full details. This is for
    /// rare operator audits and compatibility tests, not normal hot-path ticks.
    DeepAudit,
}

/// Scans Forge artifacts and the command journal for recovery work.
pub struct Reconciler<'a, P: RecoveryPolicy> {
    workflow: &'a ValidatedWorkflow,
    policy: &'a P,
}

impl<'a, P: RecoveryPolicy> Reconciler<'a, P> {
    /// Creates a reconciler bound to a validated workflow and a policy.
    pub fn new(workflow: &'a ValidatedWorkflow, policy: &'a P) -> Self {
        Self { workflow, policy }
    }
}

impl ValidatedWorkflow {
    /// Returns a [`Reconciler`] bound to this workflow and a recovery policy.
    pub fn reconciler<'a, P: RecoveryPolicy>(&'a self, policy: &'a P) -> Reconciler<'a, P> {
        Reconciler::new(self, policy)
    }
}
