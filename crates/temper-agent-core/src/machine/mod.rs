//! The pure agent loop, as a sans-IO state machine.
//!
//! [`AgentMachine`] is the functional core of an LLM agent turn: it owns the
//! conversation state and the iteration budget, and decides — purely — when to
//! call the model, which tools to run, when to inject steering, and when to
//! stop. It performs no I/O. The actual model streaming and tool execution are
//! done by the shell ([`crate::shell`]), which reuses tongs providers and
//! tools and feeds results back as [`AgentCompletion`]s.
//!
//! This mirrors the [`temper_agent_io::Machine`] discipline used by the worker:
//! `(state, completion) -> [request]`, deterministic and replayable, so the
//! whole loop — tool orchestration, max-iteration cutoff, stop-reason handling,
//! steering at turn boundaries — is unit-testable with synthetic completions and
//! drivable under the skein lab for simulation/fuzz testing.
//!
//! Design note (steering): steering messages are injected at **turn
//! boundaries** — after a model turn and its tool batch complete, before the
//! next model call — not mid-tool-batch. This keeps the machine simple while
//! still supporting live interaction (the user's stated control goal); pi's
//! finer-grained mid-batch steering is deliberately not reproduced.
//!
//! Split by domain responsibility:
//! - [`protocol`] — the I/O types exchanged with the shell.
//! - [`batching`] — the pure effect-compatible tool-batching policy.
//! - [`core`] — the [`AgentMachine`] driving logic and `Machine` trait impl.

mod batching;
mod core;
mod decision_anchor;
mod messages;
mod ordinary_failure;
mod protocol;
mod tool_failure;

pub use core::{AgentMachine, ArgPreviewFn};
pub use decision_anchor::{
    DECISION_ANCHOR_MUTATION_BLOCKED_MESSAGE, DECISION_ANCHOR_RECOVERY_MESSAGE,
    SAFE_DECISION_ANCHOR_LINEAGE_DETAIL_KEY,
};
pub use protocol::{
    AgentCompletion, AgentEvent, AgentRequest, AgentStop, BatchGeneration,
    CODEBASE_MEMORY_TOOL_PREFIX, CodebaseMemoryTiming, ModelCallStatus, OperationGeneration,
    SAFE_GRAPH_CORRELATION_DETAIL_KEY, SAFE_TOOL_FAILURE_DETAIL_KEY, StreamDelta, ToolCallStatus,
    ToolResultMetadata,
};
pub use temper_protocol_activity::{
    DecisionAnchorLineageStageV1, DecisionAnchorLineageV1, DecisionAnchorTargetKindV1,
};
pub use tool_failure::{
    ToolFailureCategory, ToolFailureDiagnostic, ToolFailureReason, ToolRetryDisposition,
};

#[cfg(test)]
mod tests;
