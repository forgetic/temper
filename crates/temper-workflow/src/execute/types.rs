//! Result types returned by the [`Executor`](super::Executor).
//!
//! Split from the parent `execute` module to keep its `mod.rs` a thin facade.

use crate::classify::ArtifactSource;
use crate::ids::{RoleId, TransitionId};
use crate::plan::WorkflowEffect;

/// Outcome of an idempotent ensure-create operation.
///
/// Distinguishes whether the executor found an existing artifact with the
/// requested correlation key or created a fresh one, so callers and tests can
/// assert that a retry did not duplicate the artifact.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EnsureOutcome<T> {
    /// A matching artifact already existed; no new artifact was created.
    Existing(T),
    /// No matching artifact existed, so a new one was created.
    Created(T),
}

impl<T> EnsureOutcome<T> {
    /// Borrows the resolved artifact, whether found or created.
    pub fn artifact(&self) -> &T {
        match self {
            EnsureOutcome::Existing(artifact) | EnsureOutcome::Created(artifact) => artifact,
        }
    }

    /// Returns `true` when a new artifact was created.
    pub fn was_created(&self) -> bool {
        matches!(self, EnsureOutcome::Created(_))
    }

    /// Consumes the outcome and returns the resolved artifact.
    pub fn into_artifact(self) -> T {
        match self {
            EnsureOutcome::Existing(artifact) | EnsureOutcome::Created(artifact) => artifact,
        }
    }
}

/// A successful transition execution.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutionReport {
    /// Transition that was executed.
    pub transition: TransitionId,
    /// Role the execution was authorized for.
    pub role: RoleId,
    /// Forge artifact the effects were applied to.
    pub target: ArtifactSource,
    /// Effects applied to the artifact, in plan order.
    pub applied: Vec<WorkflowEffect>,
}
