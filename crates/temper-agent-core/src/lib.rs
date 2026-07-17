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
//! This is the same sans-IO discipline as [`temper_agent_io`] and the worker:
//! the loop is deterministic and replayable, so it is unit-testable with
//! synthetic completions and drivable under the skein lab for
//! simulation/fuzz testing. It is designed for observability (events are
//! emitted as data, not callbacks), control (steering at turn boundaries +
//! abort), and testability from the start.

pub mod machine;
mod managed_bash;
mod managed_fs;
pub mod run;
pub mod shell;
pub mod subagent_tool;

pub use machine::{
    AgentCompletion, AgentEvent, AgentMachine, AgentRequest, AgentStop, ArgPreviewFn,
    BatchGeneration, ModelCallStatus, OperationGeneration, StreamDelta, ToolCallStatus,
    ToolResultMetadata,
};
pub use managed_bash::ManagedBashTool;
pub use managed_fs::joined_filesystem_tool;
pub use run::{
    AgentOperationLimits, SubAgent, SubAgentControl, SubAgentError, run_sub_agent,
    run_sub_agent_controllable, run_sub_agent_controllable_with_hook,
    run_sub_agent_controllable_with_hooks, run_sub_agent_controllable_with_observability,
    run_sub_agent_with_events, run_sub_agent_with_hook,
};
pub use shell::{
    AgentOutcome, AgentShell, EventClock, EventSink, ModelIdentity, NullEventSink,
    RunObservability, SystemEventClock, TurnHook,
};
#[cfg(feature = "test-support")]
pub use shell::{
    StreamRetryConfig, StreamRetryConfigOverrideGuard, install_stream_retry_config_override,
};
pub use subagent_tool::{SubAgentFactory, SubAgentObserverFactory, SubAgentTool};
#[cfg(target_os = "linux")]
#[doc(hidden)]
pub use temper_process_containment::dispatch_linux_supervisor_helper;
pub use temper_process_containment::{ProcessContainment, configure_descendant_command};
