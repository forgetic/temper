//! The provider-neutral vocabulary of a conversation with an LLM, as the model
//! speaks it to the protocol layer.
//!
//! It is the session sub-model's ([`temper_agent_model_session::llm`]), with
//! what the session carries as tickets resolved into the values they stand
//! for: the tools the run serves, the run's typed asks the LLM makes of them,
//! and the run's answers. The protocol layer turns it into each provider's
//! wire format and back, as it does the session's: it owns the schemas of the
//! tools a prompt offers, decodes the JSON the LLM writes as a tool's input
//! into a typed call, or into the [`Problem`] that keeps it from being one,
//! and renders what comes of a call as the text the LLM reads.

use alloc::boxed::Box;

use temper_agent_model_run as run;
use temper_agent_model_tools as tools;

pub use temper_agent_model_session::llm::{Endpoint, Failure, Problem, Role, Stop, Usage};

/// One call to an LLM: everything it needs to produce the next assistant
/// message.
#[derive(PartialEq, Eq, Hash, Debug)]
pub struct Prompt {
    pub endpoint: Endpoint,
    /// The provider's name for the model.
    pub model: Box<[u8]>,
    pub system: Box<[u8]>,
    /// The families of tools the LLM may call, whose schemas the protocol
    /// layer offers it, and the tools the run serves.
    pub tools: tools::Grants,
    pub served: Box<[Served]>,
    /// The conversation so far, oldest first, ending with a user message.
    pub messages: Box<[Message]>,
    /// The most tokens the answer may take.
    pub max_tokens: u32,
}

/// A tool the run serves, which a prompt offers the LLM, and which the
/// protocol layer decodes into a [`run::Ask`].
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Served {
    /// Finish the run with an outcome.
    Finish,
    /// Ask for a sub-agent.
    SubAgent,
}

#[derive(PartialEq, Eq, Hash, Debug)]
pub struct Message {
    pub role: Role,
    pub content: Box<[Block]>,
}

/// A piece of a message in a prompt. Text is UTF-8, checked by the protocol
/// layer.
#[derive(PartialEq, Eq, Hash, Debug)]
pub enum Block {
    Text {
        text: Box<[u8]>,
    },
    /// A tool call the LLM made, sent back as it wrote it: `id` is the
    /// provider's name for the call, `name` and `input` what the LLM wrote.
    ToolCall {
        id: Box<[u8]>,
        name: Box<[u8]>,
        input: Box<[u8]>,
    },
    /// What came of the tool call `id`.
    ToolResult {
        id: Box<[u8]>,
        result: Returned,
    },
}

/// What came of a tool call, for the protocol layer to render as the text the
/// LLM reads.
#[derive(PartialEq, Eq, Hash, Debug)]
pub enum Returned {
    /// The tools' outcome: a success, or a failure, one that ran out of time
    /// included.
    Owned { outcome: tools::Outcome },
    /// The run's answer to one of the tools it serves; `error` marks a
    /// failure.
    Served { returned: run::Returned, error: bool },
    /// The call was malformed, or too large to hold, and this is why.
    Invalid { problem: Problem },
    /// Nothing ran for the call: the LLM stopped for another reason than
    /// calling tools, its answer cut short or its turn ended.
    NotRun,
}

/// The next assistant message.
#[derive(PartialEq, Eq, Hash, Debug)]
pub struct Completion {
    pub content: Box<[Said]>,
    pub stop: Stop,
    pub usage: Usage,
}

/// A piece of an assistant message.
#[derive(PartialEq, Eq, Hash, Debug)]
pub enum Said {
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
}

/// A tool call, as the protocol layer decoded it.
#[derive(PartialEq, Eq, Hash, Debug)]
pub enum Decoded {
    /// A call to one of the tools a session owns.
    Owned { call: tools::Call },
    /// A call to one of the tools the run serves.
    Served { ask: run::Ask },
    /// A call that is no call: it is answered with its problem, and nothing
    /// runs for it.
    Invalid { problem: Problem },
}
