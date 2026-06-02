//! Application of reconciler recovery actions.
//!
//! [`crate::reconcile`] deliberately *decides* what to repair or escalate
//! without mutating anything: [`Reconciler::scan`](crate::reconcile::Reconciler)
//! is pure and produces a [`ReconcileReport`] of parallel findings and
//! [`RecoveryAction`]s. This module is the runtime layer that *applies* those
//! decisions, routing each action through the existing component that already
//! owns the matching mutation path rather than re-implementing one:
//!
//! - [`RecoveryAction::RequeueLease`] clears the lease through
//!   [`LeaseManager::clear`](crate::lease::LeaseManager::clear) — the
//!   reconciler is the authority that force-clears a presumed-gone holder's
//!   lease, so it uses the post-CAS clear rather than the peer-checked release.
//! - [`RecoveryAction::Repair`] re-applies the still-pending label effects of an
//!   interrupted transition through the executor's idempotent label-apply path
//!   ([`Executor::apply_label_effects`](crate::execute::Executor)), then marks
//!   the originating journal command [`Reconciled`](CommandState::Reconciled).
//! - [`RecoveryAction::Unblock`] applies a mechanical dependency unblock's
//!   labels the same idempotent way, journaling a fresh command around the
//!   mutation so a crash mid-apply is recoverable.
//! - [`RecoveryAction::MarkReconciled`] moves a stale journal command to
//!   [`Reconciled`](CommandState::Reconciled).
//! - [`RecoveryAction::Escalate`] and [`RecoveryAction::Diagnose`] are
//!   human-facing. The applier records them as *advisory* and never silently
//!   mutates workflow state for them (see "Advisory actions").
//!
//! # Safety
//!
//! Every mutating action loads fresh state before it writes, applies at most
//! once, and is safe to re-run on the same report: clearing an already-clear
//! lease is a no-op, re-applying realized labels issues no backend update, and
//! a completed unblock is skipped on a second pass. Repairs and unblocks are
//! journaled, so a crash between the mutation and the terminal journal update
//! leaves the command incomplete for the next [`crate::reconcile`] pass to
//! re-derive. Running the scan→apply loop to a fixpoint therefore converges.
//!
//! # Advisory actions
//!
//! `Escalate` and `Diagnose` route work to a human; "applying" them is not a
//! workflow-state mutation in this layer. The applier records them in
//! [`ApplyOutcome::advisory`] and performs no Forge mutation, so an escalation
//! is never silently turned into a label or comment change. A workflow that
//! wants escalation to project a label or post a comment can do so in its own
//! adapter on top of the advisory list.

use crate::classify::ArtifactSource;
use crate::execute::{ExecutionError, Executor};
use crate::ids::TransitionId;
use crate::journal::{CommandId, CommandJournal, CommandRecord, CommandState, JournalError};
use crate::lease::{LeaseError, LeaseManager};
use crate::plan::WorkflowEffect;
use crate::reconcile::{ReconcileFinding, ReconcileReport, RecoveryAction};
use chrono::{DateTime, Utc};
use std::error::Error;
use std::fmt;
use temper_forge::{Forge, RepositoryId};

/// What an [`Applier::apply_report`] run did with each action.
///
/// `applied` and `advisory` are disjoint: an action is either carried through to
/// a mutation (or confirmed already satisfied) or deliberately left for a human.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ApplyOutcome {
    /// Actions the applier carried through to a mutation, in report order.
    pub applied: Vec<RecoveryAction>,
    /// Advisory actions (`Escalate`/`Diagnose`) the applier recorded without
    /// mutating workflow state, in report order.
    pub advisory: Vec<RecoveryAction>,
}

impl ApplyOutcome {
    /// Returns `true` when nothing was applied and nothing was advised.
    pub fn is_empty(&self) -> bool {
        self.applied.is_empty() && self.advisory.is_empty()
    }

    /// Returns `true` when no action needed a human (no advisory actions).
    pub fn is_fully_applied(&self) -> bool {
        self.advisory.is_empty()
    }
}

/// Why applying a recovery action failed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ApplyError {
    /// A label re-apply through the executor failed.
    Execution(ExecutionError),
    /// A lease clear through the lease manager failed.
    Lease(LeaseError),
    /// A journal write failed.
    Journal(JournalError),
    /// The report's `findings` and `actions` were not parallel, so an action
    /// could not be paired with its finding. A well-formed [`ReconcileReport`]
    /// never trips this.
    MalformedReport,
}

impl fmt::Display for ApplyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ApplyError::Execution(error) => write!(formatter, "apply failed: {error}"),
            ApplyError::Lease(error) => write!(formatter, "lease clear failed: {error}"),
            ApplyError::Journal(error) => write!(formatter, "journal update failed: {error}"),
            ApplyError::MalformedReport => {
                write!(
                    formatter,
                    "reconcile report findings and actions are not parallel"
                )
            }
        }
    }
}

impl Error for ApplyError {}

impl From<ExecutionError> for ApplyError {
    fn from(error: ExecutionError) -> Self {
        ApplyError::Execution(error)
    }
}

impl From<LeaseError> for ApplyError {
    fn from(error: LeaseError) -> Self {
        ApplyError::Lease(error)
    }
}

impl From<JournalError> for ApplyError {
    fn from(error: JournalError) -> Self {
        ApplyError::Journal(error)
    }
}

/// Applies a [`ReconcileReport`]'s actions through the runtime components.
///
/// Borrows the three components that own the mutation paths — an [`Executor`]
/// for idempotent label re-apply, a [`LeaseManager`] for force-clearing leases,
/// and a [`CommandJournal`] for journal-state transitions — and routes each
/// action to the right one. It owns no durable state, so one applier is safe to
/// reuse across many reports and across the scan→apply loop.
pub struct Applier<'a, F: Forge + ?Sized, J: CommandJournal> {
    executor: &'a Executor<'a, F>,
    leases: &'a LeaseManager<'a, F>,
    journal: &'a J,
}

impl<'a, F: Forge + ?Sized, J: CommandJournal> Applier<'a, F, J> {
    /// Creates an applier over the components that own each mutation path.
    pub fn new(
        executor: &'a Executor<'a, F>,
        leases: &'a LeaseManager<'a, F>,
        journal: &'a J,
    ) -> Self {
        Self {
            executor,
            leases,
            journal,
        }
    }

    /// Applies every action in `report`, in report order.
    ///
    /// Each mutating action loads fresh state and applies at most once, so
    /// re-running the same report is a no-op rather than a double-apply.
    /// `Escalate`/`Diagnose` are recorded as advisory and not applied. Returns
    /// an [`ApplyOutcome`] partitioning what was applied versus advised, or the
    /// first [`ApplyError`] encountered.
    pub async fn apply_report(
        &self,
        repo_id: &RepositoryId,
        report: &ReconcileReport,
        now: DateTime<Utc>,
    ) -> Result<ApplyOutcome, ApplyError> {
        if report.findings.len() != report.actions.len() {
            return Err(ApplyError::MalformedReport);
        }
        let mut outcome = ApplyOutcome::default();
        for (finding, action) in report.findings.iter().zip(report.actions.iter()) {
            self.apply_action(repo_id, finding, action, now, &mut outcome)
                .await?;
        }
        Ok(outcome)
    }

    /// Applies one (finding, action) pair, recording it in `outcome`.
    async fn apply_action(
        &self,
        repo_id: &RepositoryId,
        finding: &ReconcileFinding,
        action: &RecoveryAction,
        now: DateTime<Utc>,
        outcome: &mut ApplyOutcome,
    ) -> Result<(), ApplyError> {
        match action {
            RecoveryAction::RequeueLease { target } => {
                self.leases.clear(repo_id, *target).await?;
                outcome.applied.push(action.clone());
            }
            RecoveryAction::Repair { target, effects } => {
                self.executor
                    .apply_label_effects(repo_id, *target, effects)
                    .await?;
                // The originating command's intent is now realized, so resolve
                // it. A crash before this point leaves it incomplete for the
                // next scan to re-derive.
                if let ReconcileFinding::PartialTransition { command, .. } = finding {
                    self.reconcile_command(command, "repaired by reconciler", now)
                        .await?;
                }
                outcome.applied.push(action.clone());
            }
            RecoveryAction::Unblock { target, effects } => {
                self.apply_unblock(repo_id, finding, *target, effects, now)
                    .await?;
                outcome.applied.push(action.clone());
            }
            RecoveryAction::MarkReconciled { command } => {
                self.reconcile_command(command, "reconciled by reconciler", now)
                    .await?;
                outcome.applied.push(action.clone());
            }
            RecoveryAction::Escalate { .. } | RecoveryAction::Diagnose { .. } => {
                outcome.advisory.push(action.clone());
            }
        }
        Ok(())
    }

    /// Applies a mechanical dependency unblock, journaled for crash recovery.
    ///
    /// Unlike a `Repair`, an unblock has no pre-existing journal command, so the
    /// applier records its own: `Planned` → `Applying` → `Completed` around the
    /// idempotent label apply. The command id is derived deterministically from
    /// the target and transition, so a second pass finds the existing record;
    /// once it is terminal the unblock is skipped, and the label apply is a
    /// no-op regardless, so re-applying is safe.
    async fn apply_unblock(
        &self,
        repo_id: &RepositoryId,
        finding: &ReconcileFinding,
        target: ArtifactSource,
        effects: &[WorkflowEffect],
        now: DateTime<Utc>,
    ) -> Result<(), ApplyError> {
        let transition = match finding {
            ReconcileFinding::DependenciesResolved { transition, .. } => Some(transition.clone()),
            _ => None,
        };
        let id = unblock_command_id(target, transition.as_ref());
        if let Some(existing) = self.journal.get(&id).await? {
            if existing.state.is_terminal() {
                // A prior pass already completed this unblock; its labels are
                // durable, so there is nothing to redo.
                return Ok(());
            }
        }
        self.journal
            .append(CommandRecord {
                id: id.clone(),
                target,
                transition,
                role: None,
                effects: effects.to_vec(),
                state: CommandState::Planned,
                detail: Some("mechanical dependency unblock".to_string()),
                created_at: now,
                updated_at: now,
            })
            .await?;
        self.journal
            .transition_state(&id, CommandState::Applying, None, now)
            .await?;
        self.executor
            .apply_label_effects(repo_id, target, effects)
            .await?;
        self.journal
            .transition_state(&id, CommandState::Completed, None, now)
            .await?;
        Ok(())
    }

    /// Moves a journal command to [`Reconciled`](CommandState::Reconciled).
    async fn reconcile_command(
        &self,
        command: &CommandId,
        detail: &str,
        now: DateTime<Utc>,
    ) -> Result<(), ApplyError> {
        self.journal
            .transition_state(
                command,
                CommandState::Reconciled,
                Some(detail.to_string()),
                now,
            )
            .await?;
        Ok(())
    }
}

/// Builds the deterministic journal id for a mechanical unblock command.
///
/// Keyed on the target (and transition, when the finding supplies one) so a
/// re-applied report resolves to the same record and the append is idempotent.
fn unblock_command_id(target: ArtifactSource, transition: Option<&TransitionId>) -> CommandId {
    let target = source_token(target);
    match transition {
        Some(transition) => CommandId::new(format!("reconcile-unblock:{target}:{transition}")),
        None => CommandId::new(format!("reconcile-unblock:{target}")),
    }
}

/// Renders an [`ArtifactSource`] into a stable token for a command id.
fn source_token(source: ArtifactSource) -> String {
    match source {
        ArtifactSource::Issue { number } => format!("issue-{number}"),
        ArtifactSource::PullRequest { number } => format!("pr-{number}"),
    }
}
