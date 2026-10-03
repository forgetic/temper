//! The mappings between the agent's vocabulary and the LLM provider's: what
//! the protocol layers on each side, and the wire between them, do without
//! the bytes. The prompts and completions, with the schemas of the tools the
//! agent offers, the decoding of the calls the LLM writes into typed calls and
//! asks, and the rendering of what came of them as text. The channel to the
//! worker is [`crate::channel`]'s.

use std::collections::BTreeMap;

use temper_agent_domain::llm::{self as agent, Decoded, Served};
use temper_agent_domain::run::charter;
use temper_agent_domain::run::outcome::{Change, Child, Declared, Field, Verdict};
use temper_agent_domain::run::{self, Ask, Spend};
use temper_agent_domain::tools::{Call, Exit, Grants, Name, Outcome, Part, Path};
use temper_fake_llm_domain::api as provider;

/// The tools the agent's side offers, by name: the family that grants each,
/// and its schema. The run's tools follow.
const TOOLS: [(&[u8], Family, &[u8]); 6] = [
    (b"read_file", Family::Inspect, br#"{"path":"string"}"#),
    (b"list_dir", Family::Inspect, br#"{"path":"string"}"#),
    (b"search", Family::Inspect, br#"{"path":"string","pattern":"string"}"#),
    (b"write_file", Family::Modify, br#"{"path":"string","content":"string"}"#),
    (b"edit_file", Family::Modify, br#"{"path":"string","old":"string","new":"string"}"#),
    (b"run_shell", Family::Shell, br#"{"command":"string"}"#),
];

/// The run's tools: `finish`, with a change's title and body or a verdict's
/// name, body and children (each child's kind and fields under keys numbered
/// for it, `1.kind`, `1.path`); and `sub_agent`, with a brief, the families
/// of tools (a list of `inspect`, `modify` and `shell`), whether it may ask
/// for sub-agents itself, the LLM, and caps on its share.
const FINISH: (&[u8], &[u8]) = (b"finish", br#"{"title":"string","body":"string","verdict":"string"}"#);
const SUB_AGENT: (&[u8], &[u8]) =
    (b"sub_agent", br#"{"brief":"string","tools":"string","agents":"string","llm":"string","turns":"string"}"#);

#[derive(Clone, Copy)]
enum Family {
    Inspect,
    Modify,
    Shell,
}

/// The provider's query for an agent's prompt: the schemas of the tools its
/// families grant and of the run's tools it offers, and its messages, every
/// result rendered as text. There is one provider, so the endpoint names
/// nothing.
#[must_use]
pub fn query(prompt: agent::Prompt) -> provider::Query {
    let agent::Prompt { endpoint: _, model, system, tools, served, messages, max_tokens } = prompt;
    let mut offered = offer(tools);
    for tool in &served {
        let (name, schema) = match tool {
            Served::Finish => FINISH,
            Served::SubAgent => SUB_AGENT,
        };
        offered.push(spec(name, schema));
    }
    let messages = messages.into_iter().map(message).collect();
    provider::Query { model, system, tools: offered.into(), messages, max_tokens }
}

/// The schemas of the tools `grants` allows.
fn offer(grants: Grants) -> Vec<provider::ToolSpec> {
    let granted = |family: Family| match family {
        Family::Inspect => grants.inspect,
        Family::Modify => grants.modify,
        Family::Shell => grants.shell,
    };
    TOOLS.iter().filter(|(_, family, _)| granted(*family)).map(|(name, _, schema)| spec(name, schema)).collect()
}

fn spec(name: &[u8], schema: &[u8]) -> provider::ToolSpec {
    provider::ToolSpec { name: name.into(), description: b"A tool.".as_slice().into(), parameters: schema.into() }
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
        agent::Block::ToolCall { id, name, input } => provider::Part::ToolCall { id, name, arguments: input },
        agent::Block::ToolResult { id, result } => {
            let (output, is_error) = render(&result);
            provider::Part::ToolOutput { id, output, is_error }
        }
    }
}

/// The text the LLM reads for what came of a call, and whether it failed.
#[must_use]
pub fn render(result: &agent::Returned) -> (Box<[u8]>, bool) {
    match result {
        agent::Returned::Owned { outcome } => match outcome {
            Outcome::Read { content, .. } => (content.clone(), false),
            Outcome::Exited { exit, .. } => {
                let failed = match exit {
                    Exit::Code { code } => *code != 0,
                    Exit::Signal { .. } | Exit::TimedOut => true,
                };
                (format!("{outcome:?}").into_bytes().into(), failed)
            }
            Outcome::Listed { .. } | Outcome::Found { .. } | Outcome::Written { .. } | Outcome::Edited { .. } => {
                (format!("{outcome:?}").into_bytes().into(), false)
            }
            Outcome::NotGranted
            | Outcome::Outside
            | Outcome::ReadOnly
            | Outcome::TooLong
            | Outcome::NotFound
            | Outcome::NotFile
            | Outcome::Linked
            | Outcome::Protected
            | Outcome::NotDirectory
            | Outcome::TooLarge { .. }
            | Outcome::NotRead
            | Outcome::Stale
            | Outcome::NoMatch
            | Outcome::Ambiguous { .. }
            | Outcome::Unchanged
            | Outcome::Failed { .. }
            | Outcome::TimedOut
            | Outcome::Cancelled
            | Outcome::Busy
            | Outcome::NulByte => (format!("{outcome:?}").into_bytes().into(), true),
        },
        agent::Returned::Served { returned, error } => {
            let text = match returned {
                run::Returned::Answered { text, .. } => text.clone(),
                returned @ (run::Returned::Accepted
                | run::Returned::Rejected { .. }
                | run::Returned::ChecksFailed { .. }
                | run::Returned::Moved
                | run::Returned::Unpushed
                | run::Returned::Cancelled
                | run::Returned::TimedOut
                | run::Returned::Busy
                | run::Returned::Unanswered { .. }
                | run::Returned::Refused { .. }) => format!("{returned:?}").into_bytes().into(),
            };
            (text, *error)
        }
        agent::Returned::Invalid { problem } => (format!("malformed call: {problem:?}").into_bytes().into(), true),
        agent::Returned::NotRun => (b"not run: the answer stopped first".as_slice().into(), true),
    }
}

/// The agent's completion for the provider's answer, every call decoded.
#[must_use]
pub fn completion(answer: provider::Answer) -> agent::Completion {
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
    let content = answer.parts.into_iter().map(said).collect();
    agent::Completion { content, stop, usage }
}

fn said(part: provider::Part) -> agent::Said {
    match part {
        provider::Part::Text { text } => agent::Said::Text { text },
        provider::Part::ToolCall { id, name, arguments } => {
            let call = decode(&name, &arguments);
            agent::Said::ToolCall { id, name, input: arguments, call }
        }
        provider::Part::ToolOutput { .. } => unreachable!("the fake answers with text and tool calls"),
    }
}

/// The provider's failure, as the agent hears it.
#[must_use]
pub fn failure(error: provider::Error) -> agent::Failure {
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
/// protocol layer decodes it: a call to the session's tools, an ask of the
/// run's, or what keeps it from being either.
#[must_use]
pub fn decode(name: &[u8], arguments: &[u8]) -> Decoded {
    let decoded = match name {
        b"finish" => finish(arguments).map(|ask| Decoded::Served { ask }),
        b"sub_agent" => sub_agent(arguments).map(|ask| Decoded::Served { ask }),
        _ => call(name, arguments).map(|call| Decoded::Owned { call }),
    };
    decoded.unwrap_or_else(|problem| Decoded::Invalid { problem })
}

type Fields = BTreeMap<Vec<u8>, Vec<u8>>;

/// A verdict's children by their numbers: each one's kind, and its fields.
type Numbered = BTreeMap<u32, (Option<Box<[u8]>>, Vec<Field>)>;

fn field(fields: &Fields, key: &[u8]) -> Result<Box<[u8]>, agent::Problem> {
    fields.get(key).map(|value| value.as_slice().into()).ok_or(agent::Problem::Missing { field: key.into() })
}

fn call(name: &[u8], arguments: &[u8]) -> Result<Call, agent::Problem> {
    let fields = object(arguments).ok_or(agent::Problem::NotAnObject)?;
    let path = |fields: &Fields| path(&field(fields, b"path")?);
    match name {
        b"read_file" => Ok(Call::Read { path: path(&fields)?, skip: 0, lines: None }),
        b"list_dir" => Ok(Call::List { path: path(&fields)? }),
        b"search" => Ok(Call::Search { path: path(&fields)?, pattern: field(&fields, b"pattern")?, glob: None }),
        b"write_file" => Ok(Call::Write { path: path(&fields)?, content: field(&fields, b"content")? }),
        b"edit_file" => {
            let (old, new) = (field(&fields, b"old")?, field(&fields, b"new")?);
            Ok(Call::Edit { path: path(&fields)?, old, new, all: false })
        }
        b"run_shell" => Ok(Call::Shell { command: field(&fields, b"command")?, timeout: None }),
        _ => Err(agent::Problem::UnknownTool),
    }
}

/// A finish: a verdict if it names one, else a change.
fn finish(arguments: &[u8]) -> Result<Ask, agent::Problem> {
    let fields = object(arguments).ok_or(agent::Problem::NotAnObject)?;
    let body = field(&fields, b"body")?;
    let Ok(name) = field(&fields, b"verdict") else {
        let title = field(&fields, b"title")?;
        return Ok(Ask::Finish { outcome: Declared::Change(Change { title, body }) });
    };
    // Each child's keys are numbered for it, in order.
    let mut children = Numbered::new();
    for (key, value) in &fields {
        let Some(dot) = key.iter().position(|byte| *byte == b'.') else {
            continue;
        };
        let bad = || agent::Problem::BadValue { field: key.as_slice().into() };
        let number = std::str::from_utf8(&key[..dot]).ok().and_then(|number| number.parse().ok()).ok_or_else(bad)?;
        let (kind, fields) = children.entry(number).or_default();
        match &key[dot + 1..] {
            b"kind" => *kind = Some(value.as_slice().into()),
            name => fields.push(Field { name: name.into(), value: value.as_slice().into() }),
        }
    }
    let mut listed = Vec::new();
    for (number, (kind, fields)) in children {
        let kind =
            kind.ok_or_else(|| agent::Problem::Missing { field: format!("{number}.kind").into_bytes().into() })?;
        listed.push(Child { kind, fields: fields.into() });
    }
    Ok(Ask::Finish { outcome: Declared::Verdict(Verdict { name, body, children: listed.into() }) })
}

/// A sub-agent: on its brief, with the families named, which are none but
/// those listed; on the LLM named, if one is; and with no more turns than
/// asked for, if a cap is.
fn sub_agent(arguments: &[u8]) -> Result<Ask, agent::Problem> {
    let fields = object(arguments).ok_or(agent::Problem::NotAnObject)?;
    let brief = field(&fields, b"brief")?;
    let mut tools = charter::Tools { inspect: false, modify: false, shell: false };
    if let Some(listed) = fields.get(b"tools".as_slice()) {
        for family in listed.split(|byte| *byte == b',') {
            match family {
                b"inspect" => tools.inspect = true,
                b"modify" => tools.modify = true,
                b"shell" => tools.shell = true,
                _ => return Err(agent::Problem::BadValue { field: b"tools".as_slice().into() }),
            }
        }
    }
    let yes = |key: &[u8]| fields.get(key).is_some_and(|value| value == b"yes");
    let families = charter::Families { tools, forge: yes(b"forge"), agents: yes(b"agents") };
    let llm = fields.get(b"llm".as_slice()).map(|llm| llm.as_slice().into());
    let share = match fields.get(b"turns".as_slice()) {
        Some(turns) => {
            let bad = || agent::Problem::BadValue { field: b"turns".as_slice().into() };
            let turns = std::str::from_utf8(turns).ok().and_then(|turns| turns.parse().ok()).ok_or_else(bad)?;
            let most = u64::MAX;
            Some(Spend { turns, input: most, output: most, cache_read: most, cache_write: most })
        }
        None => None,
    };
    Ok(Ask::SubAgent { brief, families, llm, share })
}

/// The fields of a flat JSON object of strings without escapes, which is all
/// the fake writes; `None` for anything else.
fn object(json: &[u8]) -> Option<Fields> {
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
