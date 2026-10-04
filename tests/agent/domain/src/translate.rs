//! Domain-tier wiring to the neutral fake vocabulary. Production code owns
//! schemas, JSON tool decoding and result rendering. This linear enum mapping
//! deliberately preserves the fake's max_tokens/cache-write semantics; the
//! separate protocol worlds exercise provider wire documents.
use skein_lib::Duration;
use temper_agent_domain::{llm as agent, tools};
use temper_agent_protocol::{Limits, render, tools as grammar, translate};
use temper_channel::wire::Provider;
use temper_fake_llm_domain::api as provider;
use temper_llm_openai as openai;

pub(crate) fn limits() -> Limits {
    Limits {
        calls: 0,
        endpoints: 64,
        accounts: 1,
        token_bytes: 0,
        name_bytes: 1024,
        request_bytes: 1 << 24,
        head_bytes: 0,
        headers: 0,
        event_bytes: 1 << 24,
        string_bytes: 1 << 20,
        answer_bytes: 1 << 20,
        parts: 512,
        input_bytes: 1 << 16,
        opaque_bytes: 1 << 20,
        error_bytes: 0,
        detail_bytes: 0,
        render_bytes: 1 << 20,
        depth: 24,
        tokens: 4096,
        chunk: 0,
        connect: Duration::ZERO,
        handshake: Duration::ZERO,
        head: Duration::ZERO,
        idle: Duration::ZERO,
        keep_idle: Duration::ZERO,
        skew: Duration::ZERO,
    }
}
#[must_use]
pub fn query(prompt: agent::Prompt) -> provider::Query {
    let offered =
        grammar::offer(&Provider::OpenAi, prompt.tools, &prompt.served, &limits()).expect("the world's bounded prompt");
    let specs = offered
        .into_iter()
        .map(|spec| provider::ToolSpec { name: spec.name, description: spec.description, parameters: spec.parameters })
        .collect();
    let messages = prompt
        .messages
        .into_iter()
        .map(|message| provider::Message {
            role: match message.role {
                agent::Role::User => provider::Role::User,
                agent::Role::Assistant => provider::Role::Assistant,
            },
            parts: message.content.into_iter().map(part).collect(),
        })
        .collect();
    provider::Query {
        model: prompt.model,
        system: prompt.system,
        tools: specs,
        messages,
        max_tokens: prompt.max_tokens,
    }
}
fn part(block: agent::Block) -> provider::Part {
    match block {
        agent::Block::Text { text } => provider::Part::Text { text },
        agent::Block::Opaque { bytes } => provider::Part::Opaque { bytes },
        agent::Block::ToolCall { id, name, input } => provider::Part::ToolCall { id, name, arguments: input },
        agent::Block::ToolResult { id, result } => {
            let (output, is_error) =
                render::result(&result, limits().render_bytes).expect("the world's bounded tool outcome");
            provider::Part::ToolOutput { id, output, is_error }
        }
    }
}
#[must_use]
pub fn completion(answer: provider::Answer, grants: tools::Grants, served: Box<[agent::Served]>) -> agent::Completion {
    let mut completion = translate::Completion::new(Provider::OpenAi, grants, served, &limits());
    for part in answer.parts {
        let part = match part {
            provider::Part::Text { text } => openai::Part::Text { text },
            provider::Part::Opaque { bytes } => openai::Part::Opaque { bytes },
            provider::Part::ToolCall { id, name, arguments } => {
                openai::Part::ToolCall { id, name, input: arguments, too_large: false }
            }
            provider::Part::ToolOutput { .. } => panic!("a provider answer contains no tool output"),
        };
        completion.openai(part, &limits()).expect("the fake's bounded answer");
    }
    let stop = match answer.finish {
        provider::Finish::Stop => agent::Stop::EndTurn,
        provider::Finish::ToolCalls => agent::Stop::ToolUse,
        provider::Finish::Length => agent::Stop::MaxTokens,
        provider::Finish::ContentFilter => agent::Stop::Refusal,
    };
    let usage = agent::Usage {
        input_tokens: answer.usage.prompt_tokens,
        output_tokens: answer.usage.completion_tokens,
        cache_read_tokens: answer.usage.cached_tokens,
        cache_write_tokens: answer.usage.cache_creation_tokens,
    };
    completion.finish(stop, usage)
}
#[must_use]
pub fn failure(error: provider::Error) -> agent::Failure {
    match error {
        provider::Error::Overloaded => agent::Failure::Overloaded,
        provider::Error::RateLimited { retry_after } => agent::Failure::RateLimited { retry_after },
        provider::Error::Unavailable => agent::Failure::Unavailable,
        provider::Error::ContextTooLong => agent::Failure::ContextTooLong,
        provider::Error::Unauthorized => agent::Failure::Unauthorized,
        provider::Error::Exhausted { retry_after } => agent::Failure::Exhausted { retry_after },
        provider::Error::InvalidRequest => agent::Failure::Invalid,
    }
}
