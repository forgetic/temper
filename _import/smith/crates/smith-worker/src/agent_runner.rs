//! The agent-turn seam.
//!
//! [`CodingExecutor`](crate::coding_executor::CodingExecutor) owns the workspace
//! lifecycle — prepare the checkout, run one agent turn, map the result to a
//! [`JobOutcome`](crate::executor::JobOutcome), commit/push or discard. The
//! *agent turn itself* is abstracted behind [`AgentRunner`] so the orchestration
//! is independent of how the turn is produced — and, crucially, so the worker
//! links **no** agent/LLM code: the agent runs out-of-process behind the
//! `smith-agent-protocol` wire contract.
//!
//! - [`OutOfProcessRunner`](crate::out_of_process_runner::OutOfProcessRunner)
//!   spawns an agent program (the `anvil-agent` binary by default, or any coder)
//!   speaking the protocol: context in via `TEMPER_CODING_WORKSPACE_CONTEXT`,
//!   step-progress out on stdout (relayed via a [`ProgressSink`]), result back
//!   via `TEMPER_CODING_WORKSPACE_RESULT`. This is the production path and the
//!   reuse contract — "bring any agent that speaks the protocol".
//! - test fakes return scripted results (and may emit scripted progress)
//!   without any subprocess.
//!
//! The agent has git credentials only via the prepared checkout (to push
//! commits/checkpoints); it never calls the forge API. Step-progress markers
//! are crash-recovery checkpoints the worker relays onward to the forge.

use std::path::Path;

use smith_agent_protocol::{StepProgress, WorkspaceContext};
use temper_worker_protocol::FailureClass;

pub use smith_agent_protocol::WorkspaceResult;

/// Where an [`AgentRunner`] reports step-progress checkpoints during a turn.
///
/// The runner emits one [`StepProgress`] per coherent step boundary (a marker
/// of what was done and what was pushed); the sink is the worker's hook to
/// relay it onward to the forge (via the daemon — the worker has no forge API
/// client itself; the forge API is the daemon's job). Implementations must be
/// cheap and non-blocking: a slow or failing sink must never stall or fail the
/// agent turn, whose real product is the result + the pushed commits.
pub trait ProgressSink: Send + Sync {
    /// Records one checkpoint. Infallible by contract — swallow transport
    /// trouble rather than surfacing it into the turn.
    fn report(&self, progress: StepProgress);
}

/// A [`ProgressSink`] that logs each checkpoint via [`crate::observability`] and
/// does nothing else. The default sink until the worker→daemon progress relay
/// lands; safe in production now.
#[derive(Clone, Copy, Debug, Default)]
pub struct LoggingProgressSink;

impl ProgressSink for LoggingProgressSink {
    fn report(&self, progress: StepProgress) {
        println!("{}", crate::observability::step_progress_line(&progress));
    }
}

/// A [`ProgressSink`] that discards every checkpoint. For paths that do not
/// relay progress (e.g. the stub executor, or tests not asserting on it).
#[derive(Clone, Copy, Debug, Default)]
pub struct NullProgressSink;

impl ProgressSink for NullProgressSink {
    fn report(&self, _progress: StepProgress) {}
}

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

/// Runs one role-aware coding/triage/review turn in a prepared checkout.
///
/// `context` is the work-item context the worker assembled (repository, role,
/// branch, verdict vocabulary, ...). `cwd` is the prepared checkout the turn
/// operates on: a writable role leaves a product diff in it; a read-only role
/// inspects it and returns a verdict. The runner must not commit, push, or
/// otherwise mutate Forge state — the executor owns that.
pub trait AgentRunner: Send + Sync {
    fn run(
        &self,
        context: &WorkspaceContext,
        cwd: &Path,
        progress: &dyn ProgressSink,
    ) -> impl std::future::Future<Output = Result<WorkspaceResult, AgentRunError>> + Send;
}
