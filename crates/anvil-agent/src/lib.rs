//! Sans-IO LLM agent loop for anvil.
//!
//! A anvil **sub-agent** is one bounded LLM agent run: a system prompt, an
//! initial message, a tool set (optionally scoped to a workspace), and an
//! iteration budget. Its loop — call the model, run the tools it asks for,
//! repeat until it stops or the budget is spent — is expressed as a pure
//! [`AgentMachine`] (`machine`) driven by an imperative shell that reuses
//! tongs providers and tools for the actual model streaming and tool
//! execution.
//!
//! This is the same sans-IO discipline as [`anvil_io_engine`] and the worker:
//! the loop is deterministic and replayable, so it is unit-testable with
//! synthetic completions and drivable under the skein lab for
//! simulation/fuzz testing. It is designed for observability (events are
//! emitted as data, not callbacks), control (steering at turn boundaries +
//! abort), and testability from the start.

pub mod machine;
pub mod run;
pub mod shell;
pub mod subagent_tool;

pub use machine::{
    AgentCompletion, AgentEvent, AgentMachine, AgentRequest, AgentStop, StreamDelta,
};
pub use run::{
    SubAgent, SubAgentControl, SubAgentError, run_sub_agent, run_sub_agent_controllable,
    run_sub_agent_with_events,
};
pub use shell::{AgentOutcome, AgentShell, EventSink, NullEventSink};
pub use subagent_tool::{SubAgentFactory, SubAgentTool};
