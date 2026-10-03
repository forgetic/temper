//! The mapping between the agent's vocabulary and the provider's: what the two
//! protocol layers and the wire between them do, without the bytes. The
//! agent's side owns the tools' schemas, decodes the arguments the LLM writes
//! into typed calls, and renders what comes of them as text.

use std::collections::BTreeMap;

use temper_agent_domain_session::Event;
use temper_agent_domain_session::llm as agent;

use crate::tickets::{Ticketed, Tickets};
use temper_agent_domain_tools::{Call, Effect, Exit, Grants, Name, Outcome, Part, Path};
use temper_lib::Token;
use temper_llm_domain::api as provider;

/// The tools the agent's side offers, by name: the family that grants each,
/// and its schema.
const TOOLS: [(&[u8], Family, &[u8]); 6] = [
    (b"read_file", Family::Inspect, br#"{"path":"string"}"#),
    (b"list_dir", Family::Inspect, br#"{"path":"string"}"#),
    (b"search", Family::Inspect, br#"{"path":"string","pattern":"string"}"#),
    (b"write_file", Family::Modify, br#"{"path":"string","content":"string"}"#),
    (b"edit_file", Family::Modify, br#"{"path":"string","old":"string","new":"string"}"#),
    (b"run_shell", Family::Shell, br#"{"command":"string"}"#),
];

#[derive(Clone, Copy)]
enum Family {
    Inspect,
    Modify,
    Shell,
}

/// The provider's query for an agent's prompt, its tickets resolved in
/// `tickets`: the tools the opener serves, and its answers. There is one
/// provider, so the endpoint names nothing.
#[must_use]
pub fn query(prompt: agent::Prompt, tickets: &Tickets) -> provider::Query {
    let agent::Prompt { endpoint: _, model, system, tools, delegated, messages, max_tokens } = prompt;
    let mut offered = offer(tools).into_vec();
    for descriptor in &delegated {
        let Ticketed::Tool { name, effect, schema } = tickets.resolve(descriptor.ticket) else {
            panic!("a descriptor's ticket names a tool");
        };
        assert_eq!(*effect, descriptor.effect, "a descriptor says what its tool does");
        offered.push(provider::ToolSpec {
            name: (*name).into(),
            description: b"Served.".as_slice().into(),
            parameters: (*schema).into(),
        });
    }
    let messages = messages.into_iter().map(|message| translate_message(message, tickets)).collect();
    provider::Query { model, system, tools: offered.into(), messages, max_tokens }
}

/// The agent's terminal event for the provider's answer to the call of `owner`,
/// a call of the session of `opener` that offered the tools of `served`. A
/// call to one of those is kept in `tickets`, as the top level would keep it.
#[must_use]
pub fn outcome(
    owner: Token,
    result: Result<provider::Answer, provider::Error>,
    tickets: &mut Tickets,
    opener: u64,
    served: &[agent::Descriptor],
) -> Event {
    match result {
        Ok(answer) => Event::Completed { owner, completion: completion(answer, tickets, opener, served) },
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

fn translate_message(message: agent::Message, tickets: &Tickets) -> provider::Message {
    let role = match message.role {
        agent::Role::User => provider::Role::User,
        agent::Role::Assistant => provider::Role::Assistant,
    };
    provider::Message { role, parts: message.content.into_iter().map(|block| part(block, tickets)).collect() }
}

fn part(block: agent::Block, tickets: &Tickets) -> provider::Part {
    match block {
        agent::Block::Text { text } => provider::Part::Text { text },
        // The call goes back as the LLM wrote it.
        agent::Block::ToolCall { id, name, input, call: _ } => provider::Part::ToolCall { id, name, arguments: input },
        agent::Block::ToolResult { id, result: agent::Returned::Delegated { answer } } => {
            let Ticketed::Answer { text, error } = tickets.resolve(answer.ticket) else {
                panic!("an answer's ticket names an answer");
            };
            assert_eq!(u64::try_from(text.len()), Ok(answer.bytes), "an answer counts its bytes");
            assert_eq!(*error, answer.error, "an answer says whether it failed");
            provider::Part::ToolOutput { id, output: text.clone(), is_error: *error }
        }
        agent::Block::ToolResult { id, result } => {
            let (output, is_error) = render(&result);
            provider::Part::ToolOutput { id, output, is_error }
        }
    }
}

fn completion(
    answer: provider::Answer,
    tickets: &mut Tickets,
    opener: u64,
    served: &[agent::Descriptor],
) -> agent::Completion {
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
    let content = answer.parts.into_iter().map(|part| block(part, tickets, opener, served)).collect();
    agent::Completion { content, stop, usage }
}

fn block(part: provider::Part, tickets: &mut Tickets, opener: u64, served: &[agent::Descriptor]) -> agent::Block {
    match part {
        provider::Part::Text { text } => agent::Block::Text { text },
        provider::Part::ToolCall { id, name, arguments } => {
            let call = match delegated(&name, served, tickets) {
                Some((tool, effect)) => {
                    let ticket = tickets.issue(opener, Ticketed::Call { tool, arguments: arguments.clone() });
                    agent::Decoded::Delegated { ticket, effect }
                }
                None => decode(&name, &arguments),
            };
            agent::Block::ToolCall { id, name, input: arguments, call }
        }
        provider::Part::ToolOutput { .. } => unreachable!("the fake answers with text and tool calls"),
    }
}

/// The tool of `served` named `name`, if the call is to one of them.
fn delegated(name: &[u8], served: &[agent::Descriptor], tickets: &Tickets) -> Option<(&'static [u8], Effect)> {
    served.iter().find_map(|descriptor| match tickets.resolve(descriptor.ticket) {
        Ticketed::Tool { name: tool, effect, .. } => (*tool == name).then_some((*tool, *effect)),
        Ticketed::Call { .. } | Ticketed::Answer { .. } => panic!("a descriptor's ticket names a tool"),
    })
}

fn failure(error: provider::Error) -> agent::Failure {
    match error {
        provider::Error::Overloaded => agent::Failure::Overloaded,
        provider::Error::RateLimited { retry_after } => agent::Failure::RateLimited { retry_after },
        provider::Error::Unavailable => agent::Failure::Unavailable,
        provider::Error::ContextTooLong => agent::Failure::ContextTooLong,
        provider::Error::Unauthorized => agent::Failure::Unauthorized,
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
        b"search" => Ok(Call::Search { path: path(&field(b"path")?)?, pattern: field(b"pattern")?, glob: None }),
        b"write_file" => Ok(Call::Write { path: path(&field(b"path")?)?, content: field(b"content")? }),
        b"edit_file" => {
            let (old, new) = (field(b"old")?, field(b"new")?);
            Ok(Call::Edit { path: path(&field(b"path")?)?, old, new, all: false })
        }
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
            let failed = !matches!(
                outcome,
                Outcome::Read { .. }
                    | Outcome::Listed { .. }
                    | Outcome::Found { .. }
                    | Outcome::Written { .. }
                    | Outcome::Edited { .. }
                    | Outcome::Exited { exit: Exit::Code { code: 0 }, .. }
            );
            let text = if let Outcome::Read { content, .. } = outcome {
                content.clone()
            } else {
                format!("{outcome:?}").into_bytes().into()
            };
            (text, failed)
        }
        agent::Returned::Invalid { problem } => (format!("malformed call: {problem:?}").into_bytes().into(), true),
        agent::Returned::Delegated { .. } => unreachable!("the opener's answers are rendered from their tickets"),
        agent::Returned::NotRun => (b"not run: the answer stopped first".as_slice().into(), true),
    }
}
