//! Transition-planning diagnostics and the collecting [`PlanError`].
//!
//! Split from the planning root so the diagnostic taxonomy and its `Display`
//! rendering stay separate from the [`Planner`](super::Planner) that produces
//! them.

use crate::ids::{
    ArtifactKindId, GateId, LabelId, RoleId, StateDimensionId, StateId, TransitionId,
};
use std::error::Error;
use std::fmt;

/// A single reason a transition cannot be planned.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PlanDiagnostic {
    /// The workflow declares no transition with this id.
    UnknownTransition { transition: TransitionId },
    /// The role is not authorized to perform the transition.
    Unauthorized {
        transition: TransitionId,
        role: RoleId,
    },
    /// The artifact's kind differs from the kind the transition acts on.
    ArtifactKindMismatch {
        transition: TransitionId,
        expected: ArtifactKindId,
        actual: ArtifactKindId,
    },
    /// A label a remove-effect targets is already absent: the source state is
    /// stale, so the transition would do nothing meaningful.
    StalePrecondition {
        transition: TransitionId,
        label: LabelId,
    },
    /// A label an add-effect targets is already present: the transition has
    /// already been applied or contradicts the artifact's current state.
    ContradictedPrecondition {
        transition: TransitionId,
        label: LabelId,
    },
    /// A required gate is not satisfied by the artifact's current labels.
    GateNotSatisfied {
        transition: TransitionId,
        gate: GateId,
    },
    /// Applying the effects would leave an exclusive dimension in several
    /// states at once, or put an artifact into a state not legal for its kind.
    /// Diagnosed before planning so the impossible state never reaches a Forge
    /// backend.
    ImpossibleState {
        transition: TransitionId,
        dimension: StateDimensionId,
        states: Vec<StateId>,
    },
}

impl fmt::Display for PlanDiagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PlanDiagnostic::UnknownTransition { transition } => {
                write!(formatter, "no transition `{transition}` is declared")
            }
            PlanDiagnostic::Unauthorized { transition, role } => write!(
                formatter,
                "role `{role}` is not authorized for transition `{transition}`"
            ),
            PlanDiagnostic::ArtifactKindMismatch {
                transition,
                expected,
                actual,
            } => write!(
                formatter,
                "transition `{transition}` acts on `{expected}` but the artifact is `{actual}`"
            ),
            PlanDiagnostic::StalePrecondition { transition, label } => write!(
                formatter,
                "transition `{transition}` removes label `{label}` but it is already absent"
            ),
            PlanDiagnostic::ContradictedPrecondition { transition, label } => write!(
                formatter,
                "transition `{transition}` adds label `{label}` but it is already present"
            ),
            PlanDiagnostic::GateNotSatisfied { transition, gate } => write!(
                formatter,
                "transition `{transition}` requires gate `{gate}`, which is not satisfied"
            ),
            PlanDiagnostic::ImpossibleState {
                transition,
                dimension,
                states,
            } => write!(
                formatter,
                "transition `{transition}` would put exclusive dimension `{dimension}` into states: {}",
                join_states(states)
            ),
        }
    }
}

fn join_states(ids: &[StateId]) -> String {
    ids.iter()
        .map(|id| format!("`{id}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Error returned when a transition cannot be planned.
///
/// Carries every diagnostic found so a caller sees all problems at once,
/// matching the diagnostic-collecting style of validation and classification.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlanError {
    diagnostics: Vec<PlanDiagnostic>,
}

impl PlanError {
    /// Builds an error from the collected diagnostics.
    pub(super) fn new(diagnostics: Vec<PlanDiagnostic>) -> Self {
        Self { diagnostics }
    }

    /// Returns the collected diagnostics.
    pub fn diagnostics(&self) -> &[PlanDiagnostic] {
        &self.diagnostics
    }
}

impl fmt::Display for PlanError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "transition planning failed with {} diagnostic(s)",
            self.diagnostics.len()
        )?;
        for diagnostic in &self.diagnostics {
            write!(formatter, "\n  - {diagnostic}")?;
        }
        Ok(())
    }
}

impl Error for PlanError {}
