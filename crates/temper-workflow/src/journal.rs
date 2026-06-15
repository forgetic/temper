//! Command journaling (Phase 7).
//!
//! Workflow transitions are idempotent and re-checked against fresh state, but
//! a worker can still crash *between* deciding to mutate and finishing the
//! mutation. A command journal records the lifecycle of each runtime command so
//! that, after a restart, a reconciler can tell which commands were merely
//! planned, which were mid-flight, and which finished. Without this record a
//! crash mid-transition is indistinguishable from "nothing happened".
//!
//! # Lifecycle
//!
//! A command moves through [`CommandState`]:
//!
//! ```text
//! Planned ──▶ Applying ──▶ Completed
//!                     └──▶ Failed
//! ```
//!
//! [`CommandState::Reconciled`] is a terminal state the reconciler assigns when
//! it has resolved an interrupted command (for example, after confirming the
//! intended effects already landed). `Planned` and `Applying` are the
//! *incomplete* states a reconciler must investigate; the rest are terminal.
//!
//! # Storage abstraction
//!
//! [`CommandJournal`] is a trait so durable storage (a database or the
//! filesystem backend) can be added later without changing the runtime. It is
//! async to match the [`Forge`](temper_forge_model::Forge) interface. This phase ships
//! [`InMemoryJournal`], a shared-store implementation for deterministic tests;
//! cloning it shares the same underlying log, which lets a test simulate a
//! process restart by attaching a fresh handle to existing entries.

use crate::ArtifactSource;
use crate::execute::{ExecutionError, ExecutionReport, Executor};
use crate::ids::{RoleId, TransitionId};
use crate::plan::WorkflowEffect;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::error::Error;
use std::fmt;
use std::sync::{Arc, Mutex};
use temper_forge_model::{Forge, RepositoryId};

/// Stable identifier for a journaled command.
///
/// Callers choose the id; reusing one makes [`CommandJournal::append`]
/// idempotent, which keeps a retried command from being logged twice.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CommandId(String);

impl CommandId {
    /// Creates a command id from any string-like value.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Returns the id as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for CommandId {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<String> for CommandId {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl fmt::Display for CommandId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// The lifecycle state of a journaled command.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommandState {
    /// The command's effects were computed but no mutation was attempted.
    Planned,
    /// A mutation is in flight; the artifact may be partially updated.
    Applying,
    /// The command finished and its postconditions verified.
    Completed,
    /// The command failed before completing; the detail explains why.
    Failed,
    /// The reconciler resolved an interrupted command.
    Reconciled,
}

impl CommandState {
    /// Returns `true` for states a reconciler must still investigate.
    ///
    /// Only [`Planned`](CommandState::Planned) and
    /// [`Applying`](CommandState::Applying) are incomplete; the rest are
    /// terminal.
    pub fn is_incomplete(self) -> bool {
        matches!(self, CommandState::Planned | CommandState::Applying)
    }

    /// Returns `true` for states that need no further action.
    pub fn is_terminal(self) -> bool {
        !self.is_incomplete()
    }
}

impl fmt::Display for CommandState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            CommandState::Planned => "planned",
            CommandState::Applying => "applying",
            CommandState::Completed => "completed",
            CommandState::Failed => "failed",
            CommandState::Reconciled => "reconciled",
        };
        formatter.write_str(text)
    }
}

/// A journaled command: what was intended, against what, and how it ended.
///
/// The `effects` are the planned [`WorkflowEffect`]s, recorded *before* applying
/// so a reconciler can compare them to the artifact's fresh state and decide
/// whether an interrupted command's intent already landed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandRecord {
    /// Stable command id chosen by the caller.
    pub id: CommandId,
    /// Forge artifact the command targets.
    pub target: ArtifactSource,
    /// Transition the command applies, if it is a transition command.
    pub transition: Option<TransitionId>,
    /// Role the command was authorized for, if any.
    pub role: Option<RoleId>,
    /// Effects the command intended to apply, in plan order.
    pub effects: Vec<WorkflowEffect>,
    /// Current lifecycle state.
    pub state: CommandState,
    /// Human-readable detail, typically a failure or reconciliation reason.
    pub detail: Option<String>,
    /// When the command was first journaled.
    pub created_at: DateTime<Utc>,
    /// When the record was last updated.
    pub updated_at: DateTime<Utc>,
}

impl CommandRecord {
    /// Creates a [`Planned`](CommandState::Planned) record for a transition.
    pub fn planned(
        id: CommandId,
        target: ArtifactSource,
        transition: TransitionId,
        role: RoleId,
        effects: Vec<WorkflowEffect>,
        now: DateTime<Utc>,
    ) -> Self {
        Self {
            id,
            target,
            transition: Some(transition),
            role: Some(role),
            effects,
            state: CommandState::Planned,
            detail: None,
            created_at: now,
            updated_at: now,
        }
    }
}

/// Why a journal operation failed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum JournalError {
    /// No command with the given id is recorded.
    NotFound { id: CommandId },
    /// A storage backend operation failed.
    Backend { message: String },
}

impl fmt::Display for JournalError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            JournalError::NotFound { id } => write!(formatter, "no journaled command `{id}`"),
            JournalError::Backend { message } => {
                write!(formatter, "journal backend error: {message}")
            }
        }
    }
}

impl Error for JournalError {}

/// Append-and-update log of runtime commands.
///
/// Implementations persist [`CommandRecord`]s so the lifecycle survives a crash.
/// The trait is async so a durable backend can be added without changing call
/// sites. [`append`](CommandJournal::append) is idempotent on
/// [`CommandId`]: re-appending an existing id is a no-op, which makes recording
/// a retried command safe.
#[async_trait]
pub trait CommandJournal: Send + Sync {
    /// Records a new command, or does nothing if its id is already present.
    async fn append(&self, record: CommandRecord) -> Result<(), JournalError>;

    /// Moves a command to a new state, updating its detail and timestamp.
    async fn transition_state(
        &self,
        id: &CommandId,
        state: CommandState,
        detail: Option<String>,
        now: DateTime<Utc>,
    ) -> Result<(), JournalError>;

    /// Returns the record for an id, if present.
    async fn get(&self, id: &CommandId) -> Result<Option<CommandRecord>, JournalError>;

    /// Returns every record in append order.
    async fn list(&self) -> Result<Vec<CommandRecord>, JournalError>;

    /// Returns the records still in an incomplete state, in append order.
    ///
    /// This is the reconciler's entry point: after a restart it lists the
    /// commands that were planned or mid-flight when the previous run stopped.
    async fn incomplete(&self) -> Result<Vec<CommandRecord>, JournalError> {
        Ok(self
            .list()
            .await?
            .into_iter()
            .filter(|record| record.state.is_incomplete())
            .collect())
    }
}

/// In-memory [`CommandJournal`] backed by a shared, append-ordered log.
///
/// Cloning shares the same underlying store, so a test can simulate a process
/// restart by cloning the journal and observing that previously journaled
/// commands are still visible to the new handle.
#[derive(Clone, Default)]
pub struct InMemoryJournal {
    store: Arc<Mutex<Vec<CommandRecord>>>,
}

impl InMemoryJournal {
    /// Creates an empty in-memory journal.
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<CommandRecord>> {
        self.store.lock().expect("journal mutex is not poisoned")
    }
}

#[async_trait]
impl CommandJournal for InMemoryJournal {
    async fn append(&self, record: CommandRecord) -> Result<(), JournalError> {
        let mut store = self.lock();
        if store.iter().any(|existing| existing.id == record.id) {
            // Idempotent: a retried command keeps its original record.
            return Ok(());
        }
        store.push(record);
        Ok(())
    }

    async fn transition_state(
        &self,
        id: &CommandId,
        state: CommandState,
        detail: Option<String>,
        now: DateTime<Utc>,
    ) -> Result<(), JournalError> {
        let mut store = self.lock();
        let record = store
            .iter_mut()
            .find(|record| &record.id == id)
            .ok_or_else(|| JournalError::NotFound { id: id.clone() })?;
        record.state = state;
        record.detail = detail;
        record.updated_at = now;
        Ok(())
    }

    async fn get(&self, id: &CommandId) -> Result<Option<CommandRecord>, JournalError> {
        Ok(self.lock().iter().find(|record| &record.id == id).cloned())
    }

    async fn list(&self) -> Result<Vec<CommandRecord>, JournalError> {
        Ok(self.lock().clone())
    }
}

impl<'a, F: Forge + ?Sized> Executor<'a, F> {
    /// Executes a transition while journaling its lifecycle for recovery.
    ///
    /// Records the command's intended effects as
    /// [`Planned`](CommandState::Planned) before any mutation, advances it to
    /// [`Applying`](CommandState::Applying) immediately before applying, and
    /// finally marks it [`Completed`](CommandState::Completed) or
    /// [`Failed`](CommandState::Failed). If the process crashes between
    /// `Applying` and the terminal update, the journal entry remains incomplete
    /// so [`crate::reconcile`] can detect and repair it after a restart.
    ///
    /// Planning failures (an undeclared transition, unauthorized role, or unmet
    /// precondition) are returned without journaling, because no mutation was
    /// attempted and there is nothing to recover.
    ///
    /// The argument list mirrors [`Executor::execute`] plus the journal, command
    /// id, and clock, so it is kept flat rather than wrapped in a one-off
    /// request struct that no other method shares.
    #[allow(clippy::too_many_arguments)]
    pub async fn execute_journaled<J: CommandJournal>(
        &self,
        journal: &J,
        command_id: CommandId,
        repo_id: &RepositoryId,
        target: ArtifactSource,
        transition: &TransitionId,
        role: &RoleId,
        now: DateTime<Utc>,
    ) -> Result<ExecutionReport, ExecutionError> {
        // Plan first so the journal records the intended effects. A planning
        // failure means nothing was attempted, so it is surfaced unjournaled.
        let preview = self.plan(repo_id, target, transition, role).await?;
        journal
            .append(CommandRecord::planned(
                command_id.clone(),
                target,
                preview.transition.clone(),
                preview.role.clone(),
                preview.effects.clone(),
                now,
            ))
            .await
            .map_err(journal_backend)?;
        journal
            .transition_state(&command_id, CommandState::Applying, None, now)
            .await
            .map_err(journal_backend)?;

        match self.execute(repo_id, target, transition, role).await {
            Ok(report) => {
                journal
                    .transition_state(&command_id, CommandState::Completed, None, now)
                    .await
                    .map_err(journal_backend)?;
                Ok(report)
            }
            Err(error) => {
                journal
                    .transition_state(
                        &command_id,
                        CommandState::Failed,
                        Some(error.to_string()),
                        now,
                    )
                    .await
                    .map_err(journal_backend)?;
                Err(error)
            }
        }
    }
}

/// Maps a journal failure into a backend execution error.
///
/// A journal write failure during execution is reported as a backend error so a
/// caller sees that durable recording, not the transition itself, failed.
fn journal_backend(error: JournalError) -> ExecutionError {
    ExecutionError::Backend {
        message: error.to_string(),
    }
}
