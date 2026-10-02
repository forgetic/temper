//! The provider-neutral vocabulary of a conversation with an LLM.
//!
//! The model speaks this to every provider. The protocol layer turns it into
//! each provider's wire format (the Anthropic and `OpenAI` APIs, ...) and back,
//! and classifies whatever goes wrong as a [`Failure`]. It owns the schemas of
//! the tools a prompt offers, decodes the JSON the LLM writes as a tool's input
//! into a typed call, or into the [`Problem`] that keeps it from being one, and
//! renders what comes of a call as the text the LLM reads. The model never
//! parses: it keeps a call's input as the bytes the LLM wrote, to send back
//! verbatim, beside what was decoded from it.

use alloc::boxed::Box;

use temper_agent_model_tools as tools;
use temper_lib::{Duration, Token};

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
    /// call, which its result echoes; `name` and `input` are what the LLM
    /// wrote, the input a JSON object; `call` is what the protocol layer
    /// decoded from them.
    ToolCall {
        id: Box<[u8]>,
        name: Box<[u8]>,
        input: Box<[u8]>,
        call: Decoded,
    },
    /// What came of the tool call `id`.
    ToolResult {
        id: Box<[u8]>,
        result: Returned,
    },
}

/// A tool call, as the protocol layer decoded it.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Decoded {
    /// A call to one of the tools the session owns.
    Owned { call: tools::Call },
    /// A call to one of the tools the opener serves, with `effect`: the typed
    /// call is the opener's, kept under `ticket` (a ticket names a value the
    /// session cannot, and the layer that holds it resolves it).
    Delegated { ticket: Token, effect: tools::Effect },
    /// A call that is no call: the session answers it with its problem, and
    /// runs nothing.
    Invalid { problem: Problem },
}

/// What came of a tool call, for the protocol layer to render as the text the
/// LLM reads.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Returned {
    /// The tools' outcome: a success, or a failure, one that ran out of time
    /// included.
    Owned { outcome: tools::Outcome },
    /// The opener's answer to a delegated call.
    Delegated { answer: Answer },
    /// The call was malformed, and this is why.
    Invalid { problem: Problem },
    /// Nothing ran for the call: the LLM stopped for another reason than
    /// calling tools, its answer cut short or its turn ended.
    NotRun,
}

/// A tool the opener serves, which the prompt offers the LLM: `ticket` names
/// it, and its schema is the opener's. Calls to it read or write as `effect`
/// says, and are scheduled so.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Descriptor {
    pub ticket: Token,
    pub effect: tools::Effect,
}

/// The opener's answer to a delegated call: what it says is the opener's,
/// kept under `ticket`, and takes `bytes`, which count against the session's
/// byte limit as if the session held them; `error` marks a failure, one that
/// ran out of time included.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Answer {
    pub ticket: Token,
    pub bytes: u64,
    pub error: bool,
}

/// Why the protocol layer could not decode a tool call.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Problem {
    /// No tool the prompt offered has the call's name.
    UnknownTool,
    /// The input is not a JSON object.
    NotAnObject,
    /// The input has no `field`, which the tool needs.
    Missing { field: Box<[u8]> },
    /// The input's `field` has the wrong type.
    WrongType { field: Box<[u8]> },
    /// The input's `field` has a value the tool cannot take: a path with an
    /// empty name or a NUL in it, a number out of range.
    BadValue { field: Box<[u8]> },
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Message {
    pub role: Role,
    pub content: Box<[Block]>,
}

/// One call to an LLM: everything it needs to produce the next assistant
/// message.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Prompt {
    pub endpoint: Endpoint,
    /// The provider's name for the model.
    pub model: Box<[u8]>,
    pub system: Box<[u8]>,
    /// The families of tools the LLM may call, whose schemas the protocol
    /// layer offers it, and the tools the opener serves.
    pub tools: tools::Grants,
    pub delegated: Box<[Descriptor]>,
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

/// The tokens calls consumed, as the provider counts them: what it read
/// afresh, what it wrote, and what it read from and wrote to its prompt cache.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
}

impl Usage {
    pub const ZERO: Usage = Usage { input_tokens: 0, output_tokens: 0, cache_read_tokens: 0, cache_write_tokens: 0 };

    #[must_use]
    pub const fn saturating_add(self, other: Usage) -> Usage {
        Usage {
            input_tokens: self.input_tokens.saturating_add(other.input_tokens),
            output_tokens: self.output_tokens.saturating_add(other.output_tokens),
            cache_read_tokens: self.cache_read_tokens.saturating_add(other.cache_read_tokens),
            cache_write_tokens: self.cache_write_tokens.saturating_add(other.cache_write_tokens),
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
