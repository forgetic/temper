use crate::classify::{ArtifactSource, ClassificationDiagnostic};
use crate::ids::{StateDimensionId, StateId, TransitionId};
use crate::journal::{CommandId, CommandState, JournalError};
use crate::metadata::Lease;
use crate::plan::{Postcondition, WorkflowEffect};
use std::error::Error;
use std::fmt;
use temper_forge_model::ForgeError;

/// A single problem reconciliation found in durable state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReconcileFinding {
    /// An artifact's lease has expired and its holder is presumed gone.
    ExpiredLease {
        target: ArtifactSource,
        lease: Lease,
    },
    /// An exclusive state dimension holds several states at once.
    ImpossibleState {
        target: ArtifactSource,
        dimension: StateDimensionId,
        states: Vec<StateId>,
    },
    /// Classification failed for a reason other than an impossible state.
    ClassificationDrift {
        target: ArtifactSource,
        diagnostics: Vec<ClassificationDiagnostic>,
    },
    /// A dependency-gated blocked artifact has no dependency relations, so the
    /// reconciler intentionally cannot produce a mechanical unblock.
    BlockedWithoutDependencies {
        target: ArtifactSource,
        transition: TransitionId,
        dependency_count: usize,
        relation_count: usize,
    },
    /// A journaled command's intended effects are not all realized yet.
    PartialTransition {
        command: CommandId,
        target: ArtifactSource,
        pending: Vec<Postcondition>,
    },
    /// A journaled command is incomplete but its effects already landed (or its
    /// target is gone), so only the journal status lags.
    StaleCommand {
        command: CommandId,
        target: ArtifactSource,
        state: CommandState,
    },
    /// A blocked artifact's `dependency` relations have all landed, so its
    /// dependency-gated unblock `transition` can be applied mechanically.
    DependenciesResolved {
        target: ArtifactSource,
        transition: TransitionId,
    },
}

/// What the policy decided to do about a [`ReconcileFinding`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RecoveryAction {
    /// Clear the lease so the artifact returns to its queue.
    RequeueLease { target: ArtifactSource },
    /// Route the artifact to an owner or operator for human judgement.
    Escalate {
        target: ArtifactSource,
        reason: String,
    },
    /// Re-apply the still-pending effects of an interrupted transition.
    Repair {
        target: ArtifactSource,
        effects: Vec<WorkflowEffect>,
    },
    /// Mark a journaled command [`Reconciled`](CommandState::Reconciled).
    MarkReconciled { command: CommandId },
    /// Mechanically apply a dependency-gated unblock transition's effects (clear
    /// the blocked label, mark the work ready) now that prerequisites landed.
    Unblock {
        target: ArtifactSource,
        effects: Vec<WorkflowEffect>,
    },
    /// Record a diagnostic for review without an automated change.
    Diagnose {
        target: ArtifactSource,
        message: String,
    },
}

/// Policy hooks deciding the recovery action for each finding class.
///
/// Every method has a safe default (see [`DefaultRecoveryPolicy`]), so a custom
/// policy overrides only the hooks it wants to change.
pub trait RecoveryPolicy {
    /// Decides what to do with an expired lease. Default: requeue the artifact.
    fn on_expired_lease(&self, target: ArtifactSource, _lease: &Lease) -> RecoveryAction {
        RecoveryAction::RequeueLease { target }
    }

    /// Decides what to do with an impossible exclusive state. Default: escalate.
    fn on_impossible_state(
        &self,
        target: ArtifactSource,
        dimension: &StateDimensionId,
        states: &[StateId],
    ) -> RecoveryAction {
        RecoveryAction::Escalate {
            target,
            reason: format!(
                "exclusive dimension `{dimension}` holds conflicting states: {}",
                join_states(states)
            ),
        }
    }

    /// Decides what to do with non-impossible classification drift. Default:
    /// escalate for human review.
    fn on_classification_drift(
        &self,
        target: ArtifactSource,
        diagnostics: &[ClassificationDiagnostic],
    ) -> RecoveryAction {
        RecoveryAction::Escalate {
            target,
            reason: format!("classification drift: {}", join_diagnostics(diagnostics)),
        }
    }

    /// Decides what to do with a blocked artifact that has no dependency
    /// relations. Default: record a named diagnostic and do not mutate state.
    fn on_blocked_without_dependencies(
        &self,
        target: ArtifactSource,
        transition: &TransitionId,
        dependency_count: usize,
        relation_count: usize,
    ) -> RecoveryAction {
        RecoveryAction::Diagnose {
            target,
            message: format!(
                "blocked_artifact_without_dependencies: dependency-gated unblocking for transition `{transition}` intentionally cannot proceed without at least one recorded dependency (dependency_count={dependency_count}, relation_count={relation_count})"
            ),
        }
    }

    /// Decides what to do with a partially applied transition. Default: re-apply
    /// the pending effects.
    fn on_partial_transition(
        &self,
        _command: &CommandId,
        target: ArtifactSource,
        pending: &[WorkflowEffect],
    ) -> RecoveryAction {
        RecoveryAction::Repair {
            target,
            effects: pending.to_vec(),
        }
    }

    /// Decides what to do with an incomplete command whose effects already
    /// landed (or whose target is gone). Default: mark it reconciled.
    fn on_stale_command(
        &self,
        command: &CommandId,
        _target: ArtifactSource,
        _state: CommandState,
    ) -> RecoveryAction {
        RecoveryAction::MarkReconciled {
            command: command.clone(),
        }
    }

    /// Decides what to do when a blocked artifact's dependencies have all
    /// landed. Default: mechanically apply the unblock transition's effects.
    fn on_resolved_dependencies(
        &self,
        target: ArtifactSource,
        _transition: &TransitionId,
        effects: &[WorkflowEffect],
    ) -> RecoveryAction {
        RecoveryAction::Unblock {
            target,
            effects: effects.to_vec(),
        }
    }
}

/// The built-in recovery policy with safe defaults.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DefaultRecoveryPolicy;

impl RecoveryPolicy for DefaultRecoveryPolicy {}

/// The deterministic result of a reconciliation scan.
///
/// `findings` and `actions` are parallel: `actions[i]` is the policy's decision
/// for `findings[i]`. Both follow a stable order — snapshots first (lease, then
/// classification), then journal entries — so the report is safe for
/// snapshot-style assertions.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ReconcileReport {
    /// Number of deduplicated artifact snapshots scanned by the loading path.
    /// Pure [`Reconciler::scan`](crate::reconcile::Reconciler::scan) reports `0`.
    pub snapshot_count: usize,
    pub findings: Vec<ReconcileFinding>,
    pub actions: Vec<RecoveryAction>,
}

impl ReconcileReport {
    /// Returns `true` when nothing needed reconciliation.
    pub fn is_clean(&self) -> bool {
        self.findings.is_empty()
    }

    pub(crate) fn push(&mut self, finding: ReconcileFinding, action: RecoveryAction) {
        self.findings.push(finding);
        self.actions.push(action);
    }
}

/// Why an end-to-end reconciliation could not load durable state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReconcileError {
    /// A Forge operation failed.
    Backend { message: String },
    /// A journal operation failed.
    Journal(JournalError),
}

impl fmt::Display for ReconcileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReconcileError::Backend { message } => write!(formatter, "backend error: {message}"),
            ReconcileError::Journal(error) => write!(formatter, "journal error: {error}"),
        }
    }
}

impl Error for ReconcileError {}

impl From<ForgeError> for ReconcileError {
    fn from(error: ForgeError) -> Self {
        ReconcileError::Backend {
            message: error.to_string(),
        }
    }
}

impl From<JournalError> for ReconcileError {
    fn from(error: JournalError) -> Self {
        ReconcileError::Journal(error)
    }
}

fn join_states(states: &[StateId]) -> String {
    states
        .iter()
        .map(|state| format!("`{state}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn join_diagnostics(diagnostics: &[ClassificationDiagnostic]) -> String {
    diagnostics
        .iter()
        .map(|diagnostic| diagnostic.to_string())
        .collect::<Vec<_>>()
        .join("; ")
}
