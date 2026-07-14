//! The machine's I/O protocol: the data the pure loop exchanges with its shell.
//!
//! These are the wire types of [`AgentMachine`](super::AgentMachine): the
//! events it emits as data ([`AgentEvent`]/[`StreamDelta`]), the reasons it can
//! stop ([`AgentStop`]), the completions the shell feeds back
//! ([`AgentCompletion`]), and the I/O requests it asks the shell to perform
//! ([`AgentRequest`]). Keeping them here — separate from the loop's logic —
//! lets the protocol be read and depended on without the driving code.

use tongs::model::{AssistantMessage, ContentBlock, Message, ToolCall};
use tongs::tools::ToolOutput;

/// An observability event the machine emits as data (the shell renders/records
/// it). Keeping events as machine output — rather than callbacks fired from
/// inside the loop, as pi does — preserves purity and makes the event stream
/// itself testable.
#[derive(Clone, Debug)]
pub enum AgentEvent {
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
        /// Present only for a failed attempt. Consumers must redact it before
        /// projecting it to logs or transport.
        failure: Option<String>,
    },
    /// A failed provider attempt will be retried after a bounded delay.
    ModelCallRetrying {
        turn: usize,
        call_id: String,
        next_attempt: u32,
        delay_ms: u64,
        reason: String,
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

/// Bounded, text-only metadata derived from a tool result. The original byte
/// count and truncation bit let capture projections describe omitted content
/// without retaining arbitrary output.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ToolResultMetadata {
    pub preview: Option<String>,
    pub bytes: u64,
    pub truncated: bool,
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

/// A finished I/O event delivered to the machine.
pub enum AgentCompletion {
    /// The model stream completed, yielding the full assistant message.
    LlmResponded(AssistantMessage),
    /// The model call failed at the transport/provider layer.
    LlmFailed(String),
    /// A tool the machine requested finished.
    ToolFinished { id: String, output: ToolOutput },
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
    CallLlm { messages: Vec<Message> },
    /// Run one tool call; reply with [`AgentCompletion::ToolFinished`].
    RunTool(ToolCall),
    /// Emit an observability event.
    Emit(AgentEvent),
    /// The run is finished; `final_message` is the last assistant message (or a
    /// synthesized terminal message). The shell resolves the run with it.
    Finished {
        stop: AgentStop,
        final_message: AssistantMessage,
        messages: Vec<Message>,
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
