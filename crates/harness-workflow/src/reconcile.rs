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
//! Reconciliation is split so its judgement is deterministic and testable:
//!
//! - [`Reconciler::scan`] is pure. Given artifact [`ArtifactSnapshot`]s, journal
//!   [`CommandRecord`](crate::journal::CommandRecord)s, and the current time, it
//!   produces a [`ReconcileReport`] of [`ReconcileFinding`]s paired with the
//!   [`RecoveryAction`]s a [`RecoveryPolicy`] chose. It touches no backend.
//! - [`Reconciler::reconcile`] is the async convenience that loads snapshots and
//!   journal entries from a [`Forge`] and a [`CommandJournal`], then calls
//!   `scan`.
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
//! [`RecoveryPolicy`] has one defaulted hook per finding class, so a workflow
//! can override how it handles expired leases, partial transitions, impossible
//! states, classification drift, stale commands, or resolved dependencies by
//! implementing only the hooks it cares about. [`DefaultRecoveryPolicy`] uses
//! the safe defaults: requeue expired leases, escalate ambiguous drift, repair
//! partial transitions, mark already-realized commands reconciled, and
//! mechanically unblock dependency-gated work once its prerequisites land.

use crate::classify::{
    ArtifactSource, ClassificationDiagnostic, ClassificationError, ClassifiedArtifact, Classifier,
};
use crate::ids::{StateDimensionId, StateId, TransitionId};
use crate::journal::{CommandId, CommandJournal, CommandRecord, CommandState, JournalError};
use crate::metadata::{parse_metadata_block, Lease};
use crate::plan::{DependencyStatus, Planner, Postcondition, WorkflowEffect};
use crate::validated::ValidatedWorkflow;
use harness_forge::{
    Forge, ForgeError, Issue, IssueQuery, PullRequest, PullRequestQuery, RepositoryId,
};
use std::collections::HashSet;
use std::error::Error;
use std::fmt;

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
        }
    }
}

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
    pub findings: Vec<ReconcileFinding>,
    pub actions: Vec<RecoveryAction>,
}

impl ReconcileReport {
    /// Returns `true` when nothing needed reconciliation.
    pub fn is_clean(&self) -> bool {
        self.findings.is_empty()
    }

    fn push(&mut self, finding: ReconcileFinding, action: RecoveryAction) {
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

    /// Deterministically scans snapshots and journal entries for recovery work.
    ///
    /// Produces one (finding, action) pair per detected problem, in a stable
    /// order: for each snapshot in order, its expired lease then either its
    /// classification problems (when it fails to classify) or its mechanical
    /// dependency unblocks (when it classifies cleanly), followed by each
    /// incomplete journal command in journal order. `deps` carries which
    /// prerequisite item numbers have landed (see [`DependencyStatus`]); it is
    /// supplied by the runtime, like the CI signal behind `ci_gate`. Pure and
    /// backend-free.
    pub fn scan(
        &self,
        snapshots: &[ArtifactSnapshot],
        journal: &[CommandRecord],
        deps: &DependencyStatus,
        now: chrono::DateTime<chrono::Utc>,
    ) -> ReconcileReport {
        let mut report = ReconcileReport::default();
        let classifier = Classifier::new(self.workflow);

        for snapshot in snapshots {
            self.scan_lease(snapshot, now, &mut report);
            match classifier.classify_snapshot(snapshot.source, &snapshot.labels, &snapshot.body) {
                Ok(artifact) => self.scan_dependency_unblocks(&artifact, deps, &mut report),
                Err(error) => self.scan_classification(snapshot.source, &error, &mut report),
            }
        }

        for record in journal.iter().filter(|record| record.state.is_incomplete()) {
            self.scan_command(record, snapshots, &mut report);
        }

        report
    }

    /// Detects an expired lease on a single snapshot.
    fn scan_lease(
        &self,
        snapshot: &ArtifactSnapshot,
        now: chrono::DateTime<chrono::Utc>,
        report: &mut ReconcileReport,
    ) {
        let Some(lease) = parse_metadata_block(&snapshot.body)
            .ok()
            .flatten()
            .and_then(|metadata| metadata.lease)
        else {
            return;
        };
        if lease.is_expired(now) {
            let action = self.policy.on_expired_lease(snapshot.source, &lease);
            report.push(
                ReconcileFinding::ExpiredLease {
                    target: snapshot.source,
                    lease,
                },
                action,
            );
        }
    }

    /// Detects impossible states and other classification drift for a snapshot
    /// that failed to classify.
    fn scan_classification(
        &self,
        source: ArtifactSource,
        error: &ClassificationError,
        report: &mut ReconcileReport,
    ) {
        let mut drift = Vec::new();
        for diagnostic in error.diagnostics() {
            match diagnostic {
                ClassificationDiagnostic::ExclusiveStateConflict { dimension, states } => {
                    let action = self.policy.on_impossible_state(source, dimension, states);
                    report.push(
                        ReconcileFinding::ImpossibleState {
                            target: source,
                            dimension: dimension.clone(),
                            states: states.clone(),
                        },
                        action,
                    );
                }
                other => drift.push(other.clone()),
            }
        }

        if !drift.is_empty() {
            let action = self.policy.on_classification_drift(source, &drift);
            report.push(
                ReconcileFinding::ClassificationDrift {
                    target: source,
                    diagnostics: drift,
                },
                action,
            );
        }
    }

    /// Detects mechanical dependency unblocks available for a classified
    /// artifact under the supplied dependency status.
    fn scan_dependency_unblocks(
        &self,
        artifact: &ClassifiedArtifact,
        deps: &DependencyStatus,
        report: &mut ReconcileReport,
    ) {
        let planner = Planner::new(self.workflow);
        for unblock in planner.dependency_unblocks(artifact, deps) {
            let action = self.policy.on_resolved_dependencies(
                artifact.source,
                &unblock.transition,
                &unblock.effects,
            );
            report.push(
                ReconcileFinding::DependenciesResolved {
                    target: artifact.source,
                    transition: unblock.transition,
                },
                action,
            );
        }
    }

    /// Classifies an incomplete journal command against current artifact state.
    fn scan_command(
        &self,
        record: &CommandRecord,
        snapshots: &[ArtifactSnapshot],
        report: &mut ReconcileReport,
    ) {
        let snapshot = snapshots
            .iter()
            .find(|snapshot| snapshot.source == record.target);

        // No snapshot means the target vanished, so nothing can be re-applied.
        let pending: Vec<WorkflowEffect> = match snapshot {
            None => Vec::new(),
            Some(snapshot) => {
                let labels: HashSet<&str> = snapshot.labels.iter().map(String::as_str).collect();
                record
                    .effects
                    .iter()
                    .filter(|effect| is_pending(effect, &labels))
                    .cloned()
                    .collect()
            }
        };

        if pending.is_empty() {
            // Either the effects already landed or the target is gone; only the
            // journal status lags behind reality.
            let action = self
                .policy
                .on_stale_command(&record.id, record.target, record.state);
            report.push(
                ReconcileFinding::StaleCommand {
                    command: record.id.clone(),
                    target: record.target,
                    state: record.state,
                },
                action,
            );
        } else {
            let action = self
                .policy
                .on_partial_transition(&record.id, record.target, &pending);
            let postconditions = pending.iter().filter_map(label_postcondition).collect();
            report.push(
                ReconcileFinding::PartialTransition {
                    command: record.id.clone(),
                    target: record.target,
                    pending: postconditions,
                },
                action,
            );
        }
    }

    /// Loads snapshots and journal entries from a backend, then scans them.
    ///
    /// `deps` is supplied by the caller (the runtime/adapter that knows which
    /// prerequisites have landed), mirroring the external CI signal behind
    /// `ci_gate`; it is threaded straight into [`Reconciler::scan`].
    pub async fn reconcile<F, J>(
        &self,
        forge: &F,
        repo_id: &RepositoryId,
        journal: &J,
        deps: &DependencyStatus,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<ReconcileReport, ReconcileError>
    where
        F: Forge + ?Sized,
        J: CommandJournal,
    {
        let issues = forge.list_issues(repo_id, IssueQuery::default()).await?;
        let pull_requests = forge
            .list_pull_requests(repo_id, PullRequestQuery::default())
            .await?;
        let mut snapshots: Vec<ArtifactSnapshot> =
            issues.iter().map(ArtifactSnapshot::from_issue).collect();
        snapshots.extend(
            pull_requests
                .iter()
                .map(ArtifactSnapshot::from_pull_request),
        );

        let entries = journal.list().await?;
        Ok(self.scan(&snapshots, &entries, deps, now))
    }
}

/// Returns `true` when an effect's result is not yet visible in `labels`.
///
/// Only label effects are verifiable today; any other effect variant is treated
/// as not pending because the reconciler cannot yet confirm it from labels.
fn is_pending(effect: &WorkflowEffect, labels: &HashSet<&str>) -> bool {
    match effect {
        WorkflowEffect::AddLabel(label) => !labels.contains(label.as_str()),
        WorkflowEffect::RemoveLabel(label) => labels.contains(label.as_str()),
        _ => false,
    }
}

/// Derives the postcondition a pending label effect implies, if any.
fn label_postcondition(effect: &WorkflowEffect) -> Option<Postcondition> {
    match effect {
        WorkflowEffect::AddLabel(label) => Some(Postcondition::LabelPresent(label.clone())),
        WorkflowEffect::RemoveLabel(label) => Some(Postcondition::LabelAbsent(label.clone())),
        _ => None,
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

impl ValidatedWorkflow {
    /// Returns a [`Reconciler`] bound to this workflow and a recovery policy.
    pub fn reconciler<'a, P: RecoveryPolicy>(&'a self, policy: &'a P) -> Reconciler<'a, P> {
        Reconciler::new(self, policy)
    }
}
