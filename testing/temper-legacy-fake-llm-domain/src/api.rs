//! The provider's API, as its domain layer sees it once the protocol layer has
//! parsed a request.

use alloc::boxed::Box;

use skein_lib::Duration;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Role {
    User,
    Assistant,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Part {
    Opaque {
        bytes: Box<[u8]>,
    },
    Text {
        text: Box<[u8]>,
    },
    /// The model calls a tool with `arguments`, a JSON object.
    ToolCall {
        id: Box<[u8]>,
        name: Box<[u8]>,
        arguments: Box<[u8]>,
    },
    /// The client's answer to the tool call `id`.
    ToolOutput {
        id: Box<[u8]>,
        output: Box<[u8]>,
        is_error: bool,
    },
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Message {
    pub role: Role,
    pub parts: Box<[Part]>,
}

/// A tool the client offers. `parameters` is a JSON schema.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct ToolSpec {
    pub name: Box<[u8]>,
    pub description: Box<[u8]>,
    pub parameters: Box<[u8]>,
}

/// A request for the next assistant message.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Query {
    pub model: Box<[u8]>,
    pub system: Box<[u8]>,
    pub tools: Box<[ToolSpec]>,
    pub messages: Box<[Message]>,
    pub max_tokens: u32,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Answer {
    pub parts: Box<[Part]>,
    pub finish: Finish,
    pub usage: Usage,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Finish {
    Stop,
    ToolCalls,
    Length,
    ContentFilter,
}

/// Tokens a call took: the prompt's, read afresh or from the cache, the
/// cache's new entries, and the answer's.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Usage {
    pub prompt_tokens: u64,
    pub cached_tokens: u64,
    pub cache_creation_tokens: u64,
    pub completion_tokens: u64,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Error {
    Overloaded,
    RateLimited {
        retry_after: Duration,
    },
    /// The service failed, or could not be reached.
    Unavailable,
    /// The query does not fit the model's context window.
    ContextTooLong,
    /// The client's credentials were refused.
    Unauthorized,
    Exhausted {
        retry_after: Duration,
    },
    InvalidRequest,
}

/// A conversation the fake plays from a script rather than at random: one
/// whose system text holds `cue`.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Script {
    pub cue: Box<[u8]>,
    /// The answers, in order: the first answers a conversation with no
    /// assistant message yet, the next one with one, and so on.
    pub turns: Box<[Turn]>,
}

/// One scripted answer: what it says, why it stops, and the tokens it takes
/// to say.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Turn {
    pub lines: Box<[Line]>,
    pub finish: Finish,
    pub tokens: u64,
}

/// A piece of a scripted answer. The fake names its calls.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Line {
    Text {
        text: Box<[u8]>,
    },
    /// A call to the tool `name` with `arguments`, which may be anything: a
    /// script may call a tool that was not offered, or write what is not an
    /// object.
    Call {
        name: Box<[u8]>,
        arguments: Box<[u8]>,
    },
}
