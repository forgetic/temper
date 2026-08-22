//! The machine's I/O protocol: the data the pure loop exchanges with its shell.
//!
//! These are the wire types of [`AgentMachine`](super::AgentMachine): the
//! events it emits as data ([`AgentEvent`]/[`StreamDelta`]), the reasons it can
//! stop ([`AgentStop`]), the completions the shell feeds back
//! ([`AgentCompletion`]), and the I/O requests it asks the shell to perform
//! ([`AgentRequest`]). Keeping them here — separate from the loop's logic —
//! lets the protocol be read and depended on without the driving code.

use temper_protocol_activity::{
    DecisionAnchorLineageV1, GraphCorrelationV1, GraphExplorationClosedV1,
    ShellDiscoveryDispositionV1,
};
use tongs::model::{AssistantMessage, ContentBlock, Message, ToolCall};
use tongs::provider::ToolDef;
use tongs::tools::ToolOutput;

use crate::model_failure::ModelFailureDiagnostic;

use super::tool_failure::ToolFailureDiagnostic;

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
        /// A complete, structured shell-argument representation screened by
        /// the shell-side presentation hook. This is a diagnostic-capture
        /// candidate only; transcript and human projections must use
        /// `arg_preview` instead.
        diagnostic_arguments: Option<DiagnosticToolArguments>,
        /// Closed discovery classification present only when machine-owned
        /// admission denies `bash` before registry or process execution.
        shell_discovery_disposition: Option<ShellDiscoveryDispositionV1>,
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

/// Complete structured tool arguments eligible only for diagnostic activity.
///
/// The value has no `Display` implementation and its `Debug` output exposes
/// only a byte count. This prevents a complete command from accidentally
/// becoming an operational log field while it crosses the core event seam.
#[derive(Clone, Eq, PartialEq)]
pub struct DiagnosticToolArguments(String);

impl DiagnosticToolArguments {
    /// Wraps a representation already screened by the shell-side producer for
    /// secrets, completeness, and its production byte bound.
    pub fn new(value: String) -> Self {
        Self(value)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl std::fmt::Debug for DiagnosticToolArguments {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DiagnosticToolArguments")
            .field("bytes", &self.0.len())
            .finish()
    }
}

/// The two deliberately separate presentations finalized for a tool start.
///
/// `arg_preview` is short, redacted, and human-facing. `diagnostic_arguments`
/// is complete structured evidence and may be retained only by diagnostic
/// activity policy.
#[derive(Clone, Default, Eq, PartialEq)]
pub struct ToolStartPresentation {
    pub arg_preview: Option<String>,
    pub diagnostic_arguments: Option<DiagnosticToolArguments>,
}

impl std::fmt::Debug for ToolStartPresentation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ToolStartPresentation")
            .field("arg_preview", &self.arg_preview)
            .field("diagnostic_arguments", &self.diagnostic_arguments)
            .finish()
    }
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
/// Reserved [`ToolOutput::details`] key for the closed, wrapper-extracted graph
/// correlation record. Generic tool details never enter activity metadata.
pub const SAFE_GRAPH_CORRELATION_DETAIL_KEY: &str = "temper_graph_correlation_v1";

/// A local admission decision made before a registered tool can be invoked.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ToolCallDenial {
    /// A trusted anchor still lacks the required later source evidence.
    DecisionAnchorMutation,
    /// Graph convergence or its non-progress budget closed graph exploration.
    /// Details are absent only for the legacy provider-unavailable fallback.
    GraphExplorationClosed(Option<GraphExplorationClosedV1>),
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
#[derive(Clone, Default, Eq, PartialEq)]
pub struct ToolResultMetadata {
    pub preview: Option<String>,
    pub bytes: u64,
    pub truncated: bool,
    pub failure: Option<ToolFailureDiagnostic>,
    pub codebase_memory_timing: Option<CodebaseMemoryTiming>,
    pub graph_correlation: Option<GraphCorrelationV1>,
    /// Typed current-root lineage from the trusted wrapper. It is restricted
    /// to durable activity's closed, provider-neutral schema.
    pub decision_anchor_lineage: Option<DecisionAnchorLineageV1>,
}

// A preview may be safe for its explicit consumer but is never safe to expose
// accidentally through an event or error's diagnostic formatting.
impl std::fmt::Debug for ToolResultMetadata {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ToolResultMetadata")
            .field("preview_bytes", &self.preview.as_ref().map(String::len))
            .field("bytes", &self.bytes)
            .field("truncated", &self.truncated)
            .field("failure", &self.failure)
            .field("codebase_memory_timing", &self.codebase_memory_timing)
            .field("graph_correlation", &self.graph_correlation)
            .field("decision_anchor_lineage", &self.decision_anchor_lineage)
            .finish()
    }
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
    /// Bounded recovery could not make a successful anchor consumable.
    DecisionAnchorRecoveryExhausted,
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
        /// Shell-owned typed outcome. Failed output is reconstructed from this
        /// value before it enters the next model turn; future machine policy
        /// consumes this same value rather than parsing output text.
        failure: Option<ToolFailureDiagnostic>,
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
        /// Machine-owned policy denial settled without registry execution.
        denial: Option<ToolCallDenial>,
        /// Catalog-owned preflight rejection. The shell settles this locally
        /// and must not perform a registry lookup or execution.
        rejection: Option<ToolFailureDiagnostic>,
    },
    /// Settle a circuit-broken ordinary invocation locally. The shell emits a
    /// canonical failed `ToolEnd` and returns a valid model tool result, but it
    /// must not consult or invoke the tool registry.
    RedirectTool {
        operation_generation: OperationGeneration,
        batch_generation: BatchGeneration,
        call: ToolCall,
        /// Content-free reason selected entirely by the machine's bounded
        /// process-local state.
        failure: ToolFailureDiagnostic,
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
            diagnostic_arguments: None,
            shell_discovery_disposition: None,
        };
        match event {
            AgentEvent::ToolStart {
                id,
                name,
                arg_preview,
                diagnostic_arguments,
                shell_discovery_disposition,
            } => {
                assert_eq!(id, "call_1");
                assert_eq!(name, "read");
                assert_eq!(arg_preview.as_deref(), Some("src/main.rs"));
                assert_eq!(diagnostic_arguments, None);
                assert_eq!(shell_discovery_disposition, None);
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
            diagnostic_arguments: None,
            shell_discovery_disposition: None,
        };
        assert!(matches!(
            event,
            AgentEvent::ToolStart {
                arg_preview: None,
                ..
            }
        ));
    }

    #[test]
    fn diagnostic_tool_arguments_do_not_expose_content_in_debug() {
        let secret = "complete-command-sentinel";
        let event = AgentEvent::ToolStart {
            id: "call_3".to_string(),
            name: "bash".to_string(),
            arg_preview: Some("`short preview`".to_string()),
            diagnostic_arguments: Some(DiagnosticToolArguments::new(format!(
                r#"{{"command":"{secret}"}}"#
            ))),
            shell_discovery_disposition: None,
        };
        let debug = format!("{event:?}");
        assert!(!debug.contains(secret));
        assert!(debug.contains("bytes"));
    }
}
