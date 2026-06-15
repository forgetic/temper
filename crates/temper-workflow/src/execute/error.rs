//! Execution error type and plan-error classification for the [`Executor`].
//!
//! Split from the parent `execute` module to keep its `mod.rs` a thin facade.
//! [`ExecutionError`] separates the failure classes the runtime must
//! distinguish (request validation, state preconditions, and backend failures),
//! and [`classify_plan_error`] maps a pure [`PlanError`] into that taxonomy.

use crate::classify::{ArtifactSource, ClassificationError};
use crate::ids::{RoleId, TransitionId};
use crate::plan::{PlanDiagnostic, PlanError, Postcondition, WorkflowEffect};
use temper_forge_model::ForgeError;

/// Why a transition execution failed.
///
/// The variants deliberately separate the three failure classes the runtime
/// must distinguish: a [validation](ExecutionError::Validation) problem with the
/// request itself, a [precondition](ExecutionError::Precondition) problem with
/// the artifact's current state, and a [backend](ExecutionError::Backend)
/// failure from the Forge. Classification, missing/stale targets, routable
/// merge conflicts, unsupported effects, missing-create-context, and
/// postcondition failures are reported distinctly so callers never have to guess
/// which stage failed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExecutionError {
    /// The request is invalid regardless of artifact state: an undeclared
    /// transition, an unauthorized role, or an artifact-kind mismatch.
    Validation { diagnostics: Vec<PlanDiagnostic> },
    /// The artifact's fresh state forbids the transition: a stale or
    /// contradicted label precondition, an unsatisfied gate, or an impossible
    /// resulting state. No mutation is performed.
    Precondition { diagnostics: Vec<PlanDiagnostic> },
    /// Fresh Forge state could not be classified under the workflow.
    Classification(ClassificationError),
    /// The target artifact does not exist in the backend.
    TargetMissing { target: ArtifactSource },
    /// The target changed while a side effect was being applied, so the
    /// original transition should be treated as stale rather than routed.
    TargetStale {
        target: ArtifactSource,
        message: String,
    },
    /// A pull-request merge was rejected while the target remained open and
    /// unmerged, making it eligible for workflow-declared conflict routing.
    MergeConflict {
        target: ArtifactSource,
        message: String,
    },
    /// The planner produced an effect the executor cannot apply yet.
    UnsupportedEffect { effect: WorkflowEffect },
    /// An assignee effect named a role with no Forge user bound in the
    /// [`ExecutionContext`](crate::context::ExecutionContext). Reported before
    /// any mutation.
    UnresolvedAssignee { role: RoleId },
    /// A reviewer-request effect named a role with no Forge user bound.
    UnresolvedReviewer { role: RoleId },
    /// A `CreatePullRequest` effect omitted the correlation key needed for
    /// idempotent execution. Reported before any mutation.
    MissingCorrelationKey { effect: WorkflowEffect },
    /// A `CreatePullRequest` effect has no concrete create input bound in the
    /// [`ExecutionContext`](crate::context::ExecutionContext). Reported before
    /// any mutation.
    UnresolvedPullRequestCreate {
        transition: TransitionId,
        effect_index: usize,
    },
    /// A `SetBody` effect has no agent-authored body bound in the
    /// [`ExecutionContext`](crate::context::ExecutionContext). Reported before
    /// any mutation.
    UnresolvedSetBody {
        transition: TransitionId,
        effect_index: usize,
    },
    /// An `AttachReview` effect has no agent-authored review body bound in the
    /// [`ExecutionContext`](crate::context::ExecutionContext). Reported before
    /// any mutation.
    UnresolvedAttachReview {
        transition: TransitionId,
        effect_index: usize,
    },
    /// A `CreateIssues` effect has no workspace-authored children bound in the
    /// [`ExecutionContext`](crate::context::ExecutionContext). Reported before
    /// any mutation.
    UnresolvedCreateIssues {
        transition: TransitionId,
        effect_index: usize,
    },
    /// A `CreateIssues` child referenced a sibling dependency slug that no other
    /// bound child in the same effect declares. Reported before any mutation.
    UnknownCreateIssuesDependency {
        transition: TransitionId,
        effect_index: usize,
        slug: String,
        dependency: String,
    },
    /// A postcondition did not hold after the effects were applied.
    PostconditionFailed { postcondition: Postcondition },
    /// A backend operation failed.
    Backend { message: String },
}

impl std::fmt::Display for ExecutionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExecutionError::Validation { diagnostics } => {
                write!(formatter, "transition request is invalid:")?;
                write_diagnostics(formatter, diagnostics)
            }
            ExecutionError::Precondition { diagnostics } => {
                write!(formatter, "transition preconditions are not met:")?;
                write_diagnostics(formatter, diagnostics)
            }
            ExecutionError::Classification(error) => {
                write!(formatter, "could not classify fresh state: {error}")
            }
            ExecutionError::TargetMissing { target } => {
                write!(formatter, "target artifact {target:?} does not exist")
            }
            ExecutionError::TargetStale { target, message } => {
                write!(formatter, "target artifact {target:?} is stale: {message}")
            }
            ExecutionError::MergeConflict { target, message } => write!(
                formatter,
                "merge of target artifact {target:?} was rejected: {message}"
            ),
            ExecutionError::UnsupportedEffect { effect } => {
                write!(formatter, "executor cannot apply effect {effect:?}")
            }
            ExecutionError::UnresolvedAssignee { role } => {
                write!(
                    formatter,
                    "no Forge user is bound for assignee role `{role}`"
                )
            }
            ExecutionError::UnresolvedReviewer { role } => {
                write!(
                    formatter,
                    "no Forge user is bound for reviewer role `{role}`"
                )
            }
            ExecutionError::MissingCorrelationKey { effect } => {
                write!(formatter, "effect {effect:?} has no correlation key")
            }
            ExecutionError::UnresolvedPullRequestCreate {
                transition,
                effect_index,
            } => write!(
                formatter,
                "no pull-request create input is bound for transition `{transition}` create effect #{effect_index}"
            ),
            ExecutionError::UnresolvedSetBody {
                transition,
                effect_index,
            } => write!(
                formatter,
                "no authored body is bound for transition `{transition}` set-body effect #{effect_index}"
            ),
            ExecutionError::UnresolvedAttachReview {
                transition,
                effect_index,
            } => write!(
                formatter,
                "no authored review body is bound for transition `{transition}` attach-review effect #{effect_index}"
            ),
            ExecutionError::UnresolvedCreateIssues {
                transition,
                effect_index,
            } => write!(
                formatter,
                "no authored children are bound for transition `{transition}` create-issues effect #{effect_index}"
            ),
            ExecutionError::UnknownCreateIssuesDependency {
                transition,
                effect_index,
                slug,
                dependency,
            } => write!(
                formatter,
                "create-issues child `{slug}` in transition `{transition}` effect #{effect_index} depends on unknown sibling `{dependency}`"
            ),
            ExecutionError::PostconditionFailed { postcondition } => {
                write!(
                    formatter,
                    "postcondition not satisfied after applying effects: {postcondition:?}"
                )
            }
            ExecutionError::Backend { message } => {
                write!(formatter, "backend error: {message}")
            }
        }
    }
}

impl std::error::Error for ExecutionError {}

fn write_diagnostics(
    formatter: &mut std::fmt::Formatter<'_>,
    diagnostics: &[PlanDiagnostic],
) -> std::fmt::Result {
    for diagnostic in diagnostics {
        write!(formatter, "\n  - {diagnostic}")?;
    }
    Ok(())
}

impl From<ForgeError> for ExecutionError {
    fn from(error: ForgeError) -> Self {
        ExecutionError::Backend {
            message: error.to_string(),
        }
    }
}

/// Splits a [`PlanError`] into the matching [`ExecutionError`] class.
///
/// A request-level problem (unknown transition, unauthorized role, kind
/// mismatch) outranks a state-level problem, so a mixed error is reported as a
/// validation failure. Otherwise every diagnostic is state-level and the error
/// is a precondition failure.
pub(super) fn classify_plan_error(error: PlanError) -> ExecutionError {
    let diagnostics = error.diagnostics().to_vec();
    if diagnostics.iter().any(is_validation_diagnostic) {
        ExecutionError::Validation { diagnostics }
    } else {
        ExecutionError::Precondition { diagnostics }
    }
}

fn is_validation_diagnostic(diagnostic: &PlanDiagnostic) -> bool {
    matches!(
        diagnostic,
        PlanDiagnostic::UnknownTransition { .. }
            | PlanDiagnostic::Unauthorized { .. }
            | PlanDiagnostic::ArtifactKindMismatch { .. }
    )
}
