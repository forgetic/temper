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
    ToolStart { id: String, name: String },
    /// A tool finished.
    ToolEnd { id: String, is_error: bool },
    /// Steering messages were injected at a turn boundary.
    Steered { count: usize },
    /// A model call failed (transport/API error or stall). Emitted by the shell
    /// for observability before the failure is folded into the loop; carries the
    /// human-readable reason and whether a retry will be attempted.
    ModelCallFailed { reason: String, will_retry: bool },
    /// The agent run ended (with the reason it stopped).
    AgentEnd { reason: AgentStop },
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
