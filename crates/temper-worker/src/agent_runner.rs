//! The agent-turn seam.
//!
//! [`CodingExecutor`](crate::coding_executor::CodingExecutor) owns the workspace
//! lifecycle — prepare the scoped checkout root, run one agent turn, map the
//! result to a
//! [`JobOutcome`](crate::executor::JobOutcome), commit/push or discard. The
//! *agent turn itself* is abstracted behind [`AgentRunner`] so the orchestration
//! is independent of how the turn is produced — and, crucially, so the worker
//! links **no** agent/LLM code: the agent runs out-of-process behind the
//! `smith-agent-protocol` wire contract.
//!
//! - [`OutOfProcessRunner`](crate::out_of_process_runner::OutOfProcessRunner)
//!   spawns an agent program (the `temper-agent` binary by default, or any
//!   coder) speaking the protocol: context in via `--context`, result back via
//!   `--result`.
//! - test fakes return scripted results without any subprocess.
//!
//! The agent has git credentials only via the prepared repo checkouts; it never
//! calls the forge API. The executor owns the final branch push.

use std::path::Path;

use temper_protocol_agent::WorkspaceContext;
use temper_protocol_worker::FailureClass;

pub use temper_protocol_agent::WorkspaceResult;

/// Why an agent turn could not produce a [`WorkspaceResult`].
///
/// Carries the [`FailureClass`] the executor reports to the daemon so the
/// classification (transient vs permanent vs protocol) lives with the runner
/// that knows the failure's nature, rather than being re-derived downstream.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentRunError {
    pub class: FailureClass,
    pub message: String,
}

impl AgentRunError {
    pub fn new(class: FailureClass, message: impl Into<String>) -> Self {
        Self {
            class,
            message: message.into(),
        }
    }

    /// A transient failure: retrying later may succeed (provider/transport
    /// hiccup, subprocess spawn failure, ...).
    pub fn transient(message: impl Into<String>) -> Self {
        Self::new(FailureClass::Transient, message)
    }

    /// A permanent failure: the same input will fail the same way (the agent
    /// produced no usable product, an undeclared verdict, ...).
    pub fn permanent(message: impl Into<String>) -> Self {
        Self::new(FailureClass::Permanent, message)
    }
}

impl std::fmt::Display for AgentRunError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for AgentRunError {}

/// Runs one role-aware coding/triage/review turn in a prepared workspace root.
///
/// `context` is the work-item context the worker assembled (repositories, role,
/// branch, verdict vocabulary, ...). `cwd` is the prepared scoped workspace root
/// the turn operates on: writable repos live in sibling dirs where the agent
/// leaves product diffs; read-only repos are inspected for verdicts. The runner
/// must not commit, push, or otherwise mutate Forge state — the executor owns
/// that.
pub trait AgentRunner: Send + Sync {
    fn run(
        &self,
        context: &WorkspaceContext,
        cwd: &Path,
    ) -> impl std::future::Future<Output = Result<WorkspaceResult, AgentRunError>> + Send;
}
