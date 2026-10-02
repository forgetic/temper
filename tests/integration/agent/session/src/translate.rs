//! The mapping between the agent's vocabulary and the provider's: what the two
//! protocol layers and the wire between them do, without the bytes. The
//! agent's side owns the tools' schemas, decodes the arguments the LLM writes
//! into typed calls, and renders what comes of them as text.

use std::collections::BTreeMap;

use temper_agent_model_session::Event;
use temper_agent_model_session::llm as agent;
use temper_agent_model_tools::{Call, Grants, Name, Outcome, Part, Path};
use temper_lib::Token;
use temper_llm_model::api as provider;

/// The tools the agent's side offers, by name: the family that grants each,
/// and its schema.
const TOOLS: [(&[u8], Family, &[u8]); 4] = [
    (b"read_file", Family::Inspect, br#"{"path":"string"}"#),
    (b"list_dir", Family::Inspect, br#"{"path":"string"}"#),
    (b"write_file", Family::Modify, br#"{"path":"string","content":"string"}"#),
    (b"run_shell", Family::Shell, br#"{"command":"string"}"#),
];

#[derive(Clone, Copy)]
enum Family {
    Inspect,
    Modify,
    Shell,
}

/// The provider's query for an agent's prompt. There is one provider, so the
/// endpoint names nothing.
#[must_use]
pub fn query(prompt: agent::Prompt) -> provider::Query {
    let agent::Prompt { endpoint: _, model, system, tools, messages, max_tokens } = prompt;
    provider::Query {
        model,
        system,
        tools: offer(tools),
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

/// The schemas of the tools `grants` allows.
fn offer(grants: Grants) -> Box<[provider::ToolSpec]> {
    let granted = |family: Family| match family {
        Family::Inspect => grants.inspect,
        Family::Modify => grants.modify,
        Family::Shell => grants.shell,
    };
    TOOLS
        .iter()
        .filter(|(_, family, _)| granted(*family))
        .map(|(name, _, schema)| provider::ToolSpec {
            name: (*name).into(),
            description: b"A tool.".as_slice().into(),
            parameters: (*schema).into(),
        })
        .collect()
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
        // The call goes back as the LLM wrote it.
        agent::Block::ToolCall { id, name, input, call: _ } => provider::Part::ToolCall { id, name, arguments: input },
        agent::Block::ToolResult { id, result } => {
            let (output, is_error) = render(&result);
            provider::Part::ToolOutput { id, output, is_error }
        }
    }
}

fn completion(answer: provider::Answer) -> agent::Completion {
    let stop = match answer.finish {
        provider::Finish::Stop => agent::Stop::EndTurn,
        provider::Finish::ToolCalls => agent::Stop::ToolUse,
        provider::Finish::Length => agent::Stop::MaxTokens,
        provider::Finish::ContentFilter => agent::Stop::Refusal,
    };
    let provider::Usage { prompt_tokens, cached_tokens, cache_creation_tokens, completion_tokens } = answer.usage;
    let usage = agent::Usage {
        input_tokens: prompt_tokens,
        output_tokens: completion_tokens,
        cache_read_tokens: cached_tokens,
        cache_write_tokens: cache_creation_tokens,
    };
    agent::Completion { content: answer.parts.into_iter().map(block).collect(), stop, usage }
}

fn block(part: provider::Part) -> agent::Block {
    match part {
        provider::Part::Text { text } => agent::Block::Text { text },
        provider::Part::ToolCall { id, name, arguments } => {
            let call = decode(&name, &arguments);
            agent::Block::ToolCall { id, name, input: arguments, call }
        }
        provider::Part::ToolOutput { .. } => unreachable!("the fake answers with text and tool calls"),
    }
}

fn failure(error: provider::Error) -> agent::Failure {
    match error {
        provider::Error::Overloaded => agent::Failure::Overloaded,
        provider::Error::RateLimited { retry_after } => agent::Failure::RateLimited { retry_after },
        provider::Error::InvalidRequest => agent::Failure::Invalid,
    }
}

/// The call the LLM made to the tool `name` with `arguments`, as the agent's
/// protocol layer decodes it: or what keeps it from being one.
#[must_use]
pub fn decode(name: &[u8], arguments: &[u8]) -> agent::Decoded {
    match call(name, arguments) {
        Ok(call) => agent::Decoded::Owned { call },
        Err(problem) => agent::Decoded::Invalid { problem },
    }
}

fn call(name: &[u8], arguments: &[u8]) -> Result<Call, agent::Problem> {
    let fields = object(arguments).ok_or(agent::Problem::NotAnObject)?;
    let field = |key: &[u8]| -> Result<Box<[u8]>, agent::Problem> {
        fields.get(key).map(|value| value.as_slice().into()).ok_or(agent::Problem::Missing { field: key.into() })
    };
    match name {
        b"read_file" => Ok(Call::Read { path: path(&field(b"path")?)?, skip: 0, lines: None }),
        b"list_dir" => Ok(Call::List { path: path(&field(b"path")?)? }),
        b"write_file" => Ok(Call::Write { path: path(&field(b"path")?)?, content: field(b"content")? }),
        b"run_shell" => Ok(Call::Shell { command: field(b"command")?, timeout: None }),
        _ => Err(agent::Problem::UnknownTool),
    }
}

/// The fields of a flat JSON object of strings without escapes, which is all
/// the fake writes; `None` for anything else.
fn object(json: &[u8]) -> Option<BTreeMap<Vec<u8>, Vec<u8>>> {
    let inner = json.strip_prefix(b"{")?.strip_suffix(b"}")?;
    let mut fields = BTreeMap::new();
    let mut rest = inner;
    while !rest.is_empty() {
        let (key, after) = string(rest)?;
        let after = after.strip_prefix(b":")?;
        let (value, after) = string(after)?;
        fields.insert(key.to_vec(), value.to_vec());
        rest = match after.strip_prefix(b",") {
            Some(next) => next,
            None if after.is_empty() => after,
            None => return None,
        };
    }
    Some(fields)
}

/// A JSON string at the start of `json`, and what follows it.
fn string(json: &[u8]) -> Option<(&[u8], &[u8])> {
    let inner = json.strip_prefix(b"\"")?;
    let end = inner.iter().position(|&byte| byte == b'"')?;
    Some((&inner[..end], &inner[end + 1..]))
}

/// A path as the agent's protocol layer splits it.
fn path(text: &[u8]) -> Result<Path, agent::Problem> {
    let bad = || agent::Problem::BadValue { field: b"path".as_slice().into() };
    let absolute = text.first() == Some(&b'/');
    let mut parts = Vec::new();
    for piece in text.split(|&byte| byte == b'/').filter(|piece| !piece.is_empty()) {
        parts.push(match piece {
            b"." => Part::Current,
            b".." => Part::Parent,
            name => Part::Name { name: Name::new(name.into()).ok_or_else(bad)? },
        });
    }
    if parts.is_empty() && !absolute {
        return Err(bad());
    }
    Ok(Path { absolute, parts: parts.into() })
}

/// The text the LLM reads for what came of a call, and whether it failed.
#[must_use]
pub fn render(result: &agent::Returned) -> (Box<[u8]>, bool) {
    match result {
        agent::Returned::Owned { outcome } => {
            let failed = !matches!(outcome, Outcome::Read { .. } | Outcome::Listed { .. } | Outcome::Written { .. });
            let text = if let Outcome::Read { content, .. } = outcome {
                content.clone()
            } else {
                format!("{outcome:?}").into_bytes().into()
            };
            (text, failed)
        }
        agent::Returned::Invalid { problem } => (format!("malformed call: {problem:?}").into_bytes().into(), true),
        agent::Returned::NotRun => (b"not run: the answer stopped first".as_slice().into(), true),
    }
}
