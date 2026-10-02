//! The provider-neutral vocabulary of a conversation with an LLM.
//!
//! The model speaks this to every provider. The protocol layer turns it into
//! each provider's wire format (the Anthropic and `OpenAI` APIs, ...) and back,
//! and classifies whatever goes wrong as a [`Failure`]. What is structured
//! inside a payload (a tool's input, a JSON schema) stays opaque bytes here:
//! the model never parses.

use alloc::boxed::Box;

use temper_lib::Duration;

/// A provider endpoint the protocol layer is configured with: which provider,
/// where, with which credentials. The model only names it.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Endpoint(pub u32);

/// Who wrote a message.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Role {
    /// The agent's side: the task, and the results of tool calls.
    User,
    /// The LLM.
    Assistant,
}

/// A piece of a message. Text is UTF-8, checked by the protocol layer.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Block {
    Text {
        text: Box<[u8]>,
    },
    /// The LLM asks for a tool to run. `id` is the provider's name for this
    /// call, which its result echoes; `input` is a JSON object.
    ToolCall {
        id: Box<[u8]>,
        name: Box<[u8]>,
        input: Box<[u8]>,
    },
    /// What the tool call `id` produced. `error` marks a run that failed, and
    /// `output` then says why.
    ToolResult {
        id: Box<[u8]>,
        output: Box<[u8]>,
        error: bool,
    },
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Message {
    pub role: Role,
    pub content: Box<[Block]>,
}

/// A tool the LLM may call. `schema` is the JSON schema of its input.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Tool {
    pub name: Box<[u8]>,
    pub description: Box<[u8]>,
    pub schema: Box<[u8]>,
}

/// One call to an LLM: everything it needs to produce the next assistant
/// message.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Prompt {
    pub endpoint: Endpoint,
    /// The provider's name for the model.
    pub model: Box<[u8]>,
    pub system: Box<[u8]>,
    pub tools: Box<[Tool]>,
    /// The conversation so far, oldest first, ending with a user message.
    pub messages: Box<[Message]>,
    /// The most tokens the answer may take.
    pub max_tokens: u32,
}

/// The next assistant message.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Completion {
    pub content: Box<[Block]>,
    pub stop: Stop,
    pub usage: Usage,
}

/// Why the LLM stopped.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Stop {
    /// It finished its turn.
    EndTurn,
    /// It wants the tool calls in its message run, and their results back.
    ToolUse,
    /// It ran out of its token budget mid-answer.
    MaxTokens,
    /// It declined to answer.
    Refusal,
}

/// The tokens calls consumed, as the provider counts them.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

impl Usage {
    pub const ZERO: Usage = Usage { input_tokens: 0, output_tokens: 0 };

    #[must_use]
    pub const fn saturating_add(self, other: Usage) -> Usage {
        Usage {
            input_tokens: self.input_tokens.saturating_add(other.input_tokens),
            output_tokens: self.output_tokens.saturating_add(other.output_tokens),
        }
    }
}

/// Why a call produced no message, as the protocol layer classifies the
/// provider's answer, or the lack of one.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Failure {
    /// The provider is overloaded (HTTP 529, 503). Transient.
    Overloaded,
    /// The provider limits our rate (HTTP 429) and asks us to wait
    /// `retry_after`, zero if it did not say. Transient.
    RateLimited { retry_after: Duration },
    /// The provider could not be reached, or failed (a connection error,
    /// HTTP 500). Transient.
    Unavailable,
    /// No answer within the call's deadline. Transient.
    TimedOut,
    /// The conversation does not fit the model's context window.
    ContextTooLong,
    /// The provider rejected the call as malformed or unsupported (HTTP 400,
    /// 404, 422).
    Invalid,
    /// The provider refused our credentials (HTTP 401, 403).
    Unauthorized,
}
