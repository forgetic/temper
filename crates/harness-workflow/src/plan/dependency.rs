//! Dependency-gate inputs and outputs for the planner.
//!
//! `dependency_gate` evaluation needs to know which prerequisites have landed.
//! Runtime layers derive that signal from Forge state, so it lives in a small
//! data type ([`DependencyStatus`]) rather than being derived inside the pure planner.
//! [`MechanicalPlan`] is the actor-less plan the reconciler applies to unblock
//! dependency-gated work.

use super::{Postcondition, WorkflowEffect};
use crate::artifact::ArtifactRef;
use crate::classify::ArtifactSource;
use crate::ids::TransitionId;
use std::collections::HashSet;

/// Runtime-supplied resolution status of dependency relation targets.
///
/// `dependency_gate` asks whether an artifact's prerequisite work has landed.
/// Like the CI signal behind `ci_gate`, "has it landed" is decided by runtime
/// Forge reads — never inside the pure planner — and supplied here as the set of
/// repo-qualified artifact references whose work has landed. The planner stays
/// pure: it only checks set membership against the artifact's typed
/// `dependency` relations.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DependencyStatus {
    landed: HashSet<ArtifactRef>,
}

impl DependencyStatus {
    /// An empty status: nothing has landed yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Builds a status from the artifact references whose work has landed.
    /// Bare [`harness_forge::ItemNumber`] inputs remain same-repository
    /// references for backward-compatible call sites.
    pub fn landed<I, T>(items: I) -> Self
    where
        I: IntoIterator<Item = T>,
        T: Into<ArtifactRef>,
    {
        Self {
            landed: items.into_iter().map(Into::into).collect(),
        }
    }

    /// Marks one artifact's work as landed.
    pub fn mark_landed(&mut self, item: impl Into<ArtifactRef>) {
        self.landed.insert(item.into());
    }

    /// Returns whether the given artifact's work has landed.
    pub fn is_landed(&self, item: impl Into<ArtifactRef>) -> bool {
        self.landed.contains(&item.into())
    }
}

/// A mechanical (actor-less) transition the planner authorizes purely on
/// preconditions, gates, and dependency status.
///
/// Produced by [`Planner::dependency_unblocks`](super::Planner::dependency_unblocks)
/// for the reconciler's mechanical dependency unblock: it carries no role
/// because no actor performs it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MechanicalPlan {
    /// Transition whose effects the mechanical apply would run.
    pub transition: TransitionId,
    /// Forge artifact the effects target.
    pub target: ArtifactSource,
    /// Typed effects to apply, in declaration order.
    pub effects: Vec<WorkflowEffect>,
    /// Conditions that must hold once the effects are applied.
    pub postconditions: Vec<Postcondition>,
}
