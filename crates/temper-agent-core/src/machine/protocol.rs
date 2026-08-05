//! The machine's I/O protocol: the data the pure loop exchanges with its shell.
//!
//! These are the wire types of [`AgentMachine`](super::AgentMachine): the
//! events it emits as data ([`AgentEvent`]/[`StreamDelta`]), the reasons it can
//! stop ([`AgentStop`]), the completions the shell feeds back
//! ([`AgentCompletion`]), and the I/O requests it asks the shell to perform
//! ([`AgentRequest`]). Keeping them here — separate from the loop's logic —
//! lets the protocol be read and depended on without the driving code.

use tongs::model::{AssistantMessage, ContentBlock, Message, ToolCall};
use tongs::provider::ToolDef;
use tongs::tools::ToolOutput;

use crate::model_failure::ModelFailureDiagnostic;

/// An observability event the machine emits as data (the shell renders/records
/// it). Keeping events as machine output — rather than callbacks fired from
/// inside the loop, as pi does — preserves purity and makes the event stream
/// itself testable.
#[derive(Clone, Debug)]
pub enum AgentEvent {
    /// The exact model-visible startup context assembled for this invocation.
    /// It deliberately excludes provider transport and execution state.
    PromptPrepared {
        system_prompt: Option<String>,
        initial_user_message: String,
        tools: Vec<ToolDef>,
    },
    /// A model turn is starting (about to call the LLM).
    TurnStart { turn: usize },
    /// One provider attempt for a model turn is starting. Retries retain the
    /// same `call_id` and increment `attempt`.
    ModelCallStarted {
        turn: usize,
        call_id: String,
        attempt: u32,
        provider: String,
        model: String,
    },
    /// One provider attempt settled. Timing is measured on the runtime's
    /// monotonic clock around the streaming operation.
    ModelCallFinished {
        turn: usize,
        call_id: String,
        attempt: u32,
        status: ModelCallStatus,
        duration_ms: u64,
        time_to_first_token_ms: Option<u64>,
        stop_reason: Option<tongs::model::StopReason>,
        usage: tongs::model::Usage,
        /// Present only for a failed attempt. The provider boundary has
        /// already bounded and sanitized every retained field.
        failure: Option<ModelFailureDiagnostic>,
    },
    /// A failed provider attempt will be retried after a bounded delay.
    ModelCallRetrying {
        turn: usize,
        call_id: String,
        next_attempt: u32,
        delay_ms: u64,
        reason: ModelFailureDiagnostic,
    },
    /// A live streaming delta from the model, emitted by the shell as the
    /// response streams in (before the turn's full [`AgentEvent::AssistantMessage`]).
    /// Lets observers — a TUI, a transcript recorder — watch tokens and tool
    /// calls arrive in real time. The machine never sees these; they are the
    /// shell's responsibility, so the loop stays pure.
    StreamDelta(StreamDelta),
    /// The model produced an assistant message.
    AssistantMessage { content: Vec<ContentBlock> },
    /// Per-turn token accounting from the provider's terminal stream event.
    /// Emitted by the shell (like [`AgentEvent::StreamDelta`]) the moment the
    /// turn's final message arrives; the machine never sees it.
    TurnUsage {
        turn: usize,
        usage: tongs::model::Usage,
    },
    /// A tool is about to run.
    ToolStart {
        id: String,
        name: String,
        /// An optional one-line preview of the call's salient argument (e.g.
        /// the path read, the command run), filled in by the shell-side logger
        /// (see the agent-log-cleanup plan, pieces B/D). Left `None` here so the
        /// pure machine core need not compute it.
        arg_preview: Option<String>,
    },
    /// A tool finished. Timing is measured by the shell around execution and
    /// `result` is a bounded text-only candidate; unrestricted tool details are
    /// never placed in the machine event stream.
    ToolEnd {
        id: String,
        name: String,
        status: ToolCallStatus,
        duration_ms: u64,
        result: ToolResultMetadata,
    },
    /// Steering messages were injected at a turn boundary.
    Steered { count: usize },
    /// The agent run ended (with the reason it stopped).
    AgentEnd { reason: AgentStop },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModelCallStatus {
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolCallStatus {
    Succeeded,
    Failed,
    Cancelled,
}

/// Public-name prefix reserved for Temper's trusted codebase-memory wrappers.
pub const CODEBASE_MEMORY_TOOL_PREFIX: &str = "codebase_memory_";

/// Reserved [`ToolOutput::details`] key used by codebase-memory wrappers to
/// pass only a category marker to the shell. The shell reconstructs every
/// other diagnostic field from that category rather than trusting arbitrary
/// tool-owned JSON.
pub const SAFE_TOOL_FAILURE_DETAIL_KEY: &str = "temper_safe_tool_failure_v1";

/// Stable failure categories for model-visible codebase-memory tools.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolFailureCategory {
    ConfigurationStartup,
    ProjectNotReady,
    IndexFailure,
    Timeout,
    Transport,
    ProcessExit,
    ProviderProtocol,
    InvalidModelInput,
    CircuitOpen,
}

impl ToolFailureCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ConfigurationStartup => "configuration_startup",
            Self::ProjectNotReady => "project_not_ready",
            Self::IndexFailure => "index_failure",
            Self::Timeout => "timeout",
            Self::Transport => "transport",
            Self::ProcessExit => "process_exit",
            Self::ProviderProtocol => "provider_protocol",
            Self::InvalidModelInput => "invalid_model_input",
            Self::CircuitOpen => "circuit_open",
        }
    }

    pub fn from_stable_str(value: &str) -> Option<Self> {
        match value {
            "configuration_startup" => Some(Self::ConfigurationStartup),
            "project_not_ready" => Some(Self::ProjectNotReady),
            "index_failure" => Some(Self::IndexFailure),
            "timeout" => Some(Self::Timeout),
            "transport" => Some(Self::Transport),
            "process_exit" => Some(Self::ProcessExit),
            "provider_protocol" => Some(Self::ProviderProtocol),
            "invalid_model_input" => Some(Self::InvalidModelInput),
            "circuit_open" => Some(Self::CircuitOpen),
            _ => None,
        }
    }

    pub fn safe_message(self) -> &'static str {
        match self {
            Self::ConfigurationStartup => "codebase-memory setup did not complete",
            Self::ProjectNotReady => "codebase-memory project is not ready",
            Self::IndexFailure => "codebase-memory indexing failed",
            Self::Timeout => "codebase-memory request timed out",
            Self::Transport => "codebase-memory transport failed",
            Self::ProcessExit => "codebase-memory provider process exited",
            Self::ProviderProtocol => "codebase-memory provider or protocol request failed",
            Self::InvalidModelInput => "codebase-memory request input was invalid",
            Self::CircuitOpen => "codebase-memory is disabled for this run after repeated failures",
        }
    }

    pub fn retryable(self) -> bool {
        matches!(
            self,
            Self::ProjectNotReady | Self::Timeout | Self::Transport
        )
    }
}

/// Safe, bounded evidence for one codebase-memory failure. Messages are
/// category-owned constants; raw tool output, stderr, arguments, cache data,
/// and repository content can never populate this type.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolFailureDiagnostic {
    pub category: ToolFailureCategory,
    pub retryable: bool,
    pub fallback_to_conventional_discovery: bool,
    pub message: String,
}

impl ToolFailureDiagnostic {
    pub fn codebase_memory(category: ToolFailureCategory) -> Self {
        Self {
            category,
            retryable: category.retryable(),
            fallback_to_conventional_discovery: true,
            message: category.safe_message().to_string(),
        }
    }
}

/// Content-free timing metadata accepted only from Temper's trusted
/// codebase-memory wrappers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CodebaseMemoryTiming {
    pub readiness_wait_ms: u64,
    pub graph_execution_ms: u64,
}

/// Bounded, text-only metadata derived from a tool result plus an optional
/// trusted codebase-memory diagnostic. The original byte count and truncation
/// bit let capture projections describe omitted content without retaining
/// arbitrary output.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ToolResultMetadata {
    pub preview: Option<String>,
    pub bytes: u64,
    pub truncated: bool,
    pub failure: Option<ToolFailureDiagnostic>,
    pub codebase_memory_timing: Option<CodebaseMemoryTiming>,
}

/// A live streaming fragment of a model response, forwarded by the shell.
#[derive(Clone, Debug)]
pub enum StreamDelta {
    /// A chunk of assistant text.
    Text(String),
    /// A chunk of model "thinking" (extended reasoning).
    Thinking(String),
    /// A tool call finished streaming (its arguments are now complete).
    ToolCall { id: String, name: String },
}

/// Why the agent loop stopped.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentStop {
    /// The model returned a non-tool-use stop (normal completion).
    Completed,
    /// The model signalled an error stop reason.
    ModelError,
    /// The run was aborted (cancellation/steering-to-stop).
    Aborted,
    /// The tool-iteration budget was exhausted.
    BudgetExhausted,
}

/// Monotonic identity assigned to one model or tool operation.
///
/// Generations are scoped to one [`AgentMachine`](super::AgentMachine) run and
/// are never reused. They fence completions that arrive after cancellation or
/// after a later operation has already started.
pub type OperationGeneration = u64;

/// Monotonic identity shared by every tool call dispatched in one parallel
/// batch. Model calls use batch generation zero.
pub type BatchGeneration = u64;

/// A finished I/O event delivered to the machine.
pub enum AgentCompletion {
    /// The model stream completed, yielding the full assistant message.
    LlmResponded {
        operation_generation: OperationGeneration,
        batch_generation: BatchGeneration,
        message: AssistantMessage,
    },
    /// The model call failed at the transport/provider layer.
    LlmFailed {
        operation_generation: OperationGeneration,
        batch_generation: BatchGeneration,
        diagnostic: ModelFailureDiagnostic,
    },
    /// A tool the machine requested finished.
    ToolFinished {
        operation_generation: OperationGeneration,
        batch_generation: BatchGeneration,
        id: String,
        output: ToolOutput,
    },
    /// The shell has cancelled and joined every model/tool task owned by this
    /// run. The generations identify the matching cancellation request.
    TasksQuiesced {
        operation_generation: OperationGeneration,
        batch_generation: BatchGeneration,
    },
    /// Steering messages arrived from the controller; inject at the next turn
    /// boundary.
    Steer(Vec<Message>),
    /// The run was asked to abort.
    Abort,
}

/// An I/O request the shell must perform.
pub enum AgentRequest {
    /// Stream a model response over the current message history. The shell
    /// builds the provider `Context` from these messages + the agent's system
    /// prompt and tool defs (held by the shell), and replies with
    /// [`AgentCompletion::LlmResponded`] / [`AgentCompletion::LlmFailed`].
    CallLlm {
        operation_generation: OperationGeneration,
        batch_generation: BatchGeneration,
        messages: Vec<Message>,
    },
    /// Run one tool call; reply with [`AgentCompletion::ToolFinished`].
    RunTool {
        operation_generation: OperationGeneration,
        batch_generation: BatchGeneration,
        call: ToolCall,
    },
    /// Cancel every model/tool task owned by the shell. The machine finishes
    /// only after the matching [`AgentCompletion::TasksQuiesced`] arrives.
    CancelActive {
        operation_generation: OperationGeneration,
        batch_generation: BatchGeneration,
    },
    /// Emit an observability event.
    Emit(AgentEvent),
    /// The run is finished; `final_message` is the last assistant message (or a
    /// synthesized terminal message). The shell resolves the run with it.
    Finished {
        stop: AgentStop,
        final_message: AssistantMessage,
        messages: Vec<Message>,
        /// Authoritative typed failure for a terminal model error. The
        /// synthetic assistant message remains compatibility-only.
        model_failure: Option<ModelFailureDiagnostic>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_start_can_carry_arg_preview() {
        let event = AgentEvent::ToolStart {
            id: "call_1".to_string(),
            name: "read".to_string(),
            arg_preview: Some("src/main.rs".to_string()),
        };
        match event {
            AgentEvent::ToolStart {
                id,
                name,
                arg_preview,
            } => {
                assert_eq!(id, "call_1");
                assert_eq!(name, "read");
                assert_eq!(arg_preview.as_deref(), Some("src/main.rs"));
            }
            _ => panic!("expected ToolStart"),
        }
    }

    #[test]
    fn tool_start_arg_preview_defaults_to_none() {
        let event = AgentEvent::ToolStart {
            id: "call_2".to_string(),
            name: "bash".to_string(),
            arg_preview: None,
        };
        assert!(matches!(
            event,
            AgentEvent::ToolStart {
                arg_preview: None,
                ..
            }
        ));
    }
}
