//! The mapping between the agent's vocabulary and the provider's: what the two
//! protocol layers and the wire between them do, without the bytes.

use temper_agent_model::Event;
use temper_agent_model::llm as agent;
use temper_lib::Token;
use temper_llm_model::api as provider;

/// The provider's query for an agent's prompt. There is one provider, so the
/// endpoint names nothing.
#[must_use]
pub fn query(prompt: agent::Prompt) -> provider::Query {
    let agent::Prompt { endpoint: _, model, system, tools, messages, max_tokens } = prompt;
    provider::Query {
        model,
        system,
        tools: tools.into_iter().map(tool).collect(),
        messages: messages.into_iter().map(message).collect(),
        max_tokens,
    }
}

/// The agent's terminal event for the provider's answer to the call of `owner`.
#[must_use]
pub fn outcome(owner: Token, result: Result<provider::Answer, provider::Error>) -> Event {
    match result {
        Ok(answer) => Event::Completed { owner, completion: completion(answer) },
        Err(error) => Event::Failed { owner, failure: failure(error) },
    }
}

fn tool(tool: agent::Tool) -> provider::ToolSpec {
    let agent::Tool { name, description, schema } = tool;
    provider::ToolSpec { name, description, parameters: schema }
}

fn message(message: agent::Message) -> provider::Message {
    let role = match message.role {
        agent::Role::User => provider::Role::User,
        agent::Role::Assistant => provider::Role::Assistant,
    };
    provider::Message { role, parts: message.content.into_iter().map(part).collect() }
}

fn part(block: agent::Block) -> provider::Part {
    match block {
        agent::Block::Text { text } => provider::Part::Text { text },
        agent::Block::ToolCall { id, name, input } => provider::Part::ToolCall { id, name, arguments: input },
        agent::Block::ToolResult { id, output, error } => provider::Part::ToolOutput { id, output, is_error: error },
    }
}

fn completion(answer: provider::Answer) -> agent::Completion {
    let stop = match answer.finish {
        provider::Finish::Stop => agent::Stop::EndTurn,
        provider::Finish::ToolCalls => agent::Stop::ToolUse,
        provider::Finish::Length => agent::Stop::MaxTokens,
        provider::Finish::ContentFilter => agent::Stop::Refusal,
    };
    let usage =
        agent::Usage { input_tokens: answer.usage.prompt_tokens, output_tokens: answer.usage.completion_tokens };
    agent::Completion { content: answer.parts.into_iter().map(block).collect(), stop, usage }
}

fn block(part: provider::Part) -> agent::Block {
    match part {
        provider::Part::Text { text } => agent::Block::Text { text },
        provider::Part::ToolCall { id, name, arguments } => agent::Block::ToolCall { id, name, input: arguments },
        provider::Part::ToolOutput { id, output, is_error } => agent::Block::ToolResult { id, output, error: is_error },
    }
}

fn failure(error: provider::Error) -> agent::Failure {
    match error {
        provider::Error::Overloaded => agent::Failure::Overloaded,
        provider::Error::RateLimited { retry_after } => agent::Failure::RateLimited { retry_after },
        provider::Error::InvalidRequest => agent::Failure::Invalid,
    }
}
