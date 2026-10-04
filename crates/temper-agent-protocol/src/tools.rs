//! Offered schemas and total tool decoding (llm.md, 5.1 and 6.4).
use crate::{Error, Limits};
use alloc::boxed::Box;
use skein_json::Token;
use skein_lib::{Duration, List, bytes};
use temper_agent_domain::{llm, run, tools};
use temper_channel::wire::Provider;
use temper_llm_anthropic::identity as anthropic;
use temper_llm_openai::{self as openai, identity};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Read,
    List,
    Search,
    Write,
    Edit,
    Shell,
    Finish,
    SubAgent,
}
const ALL: [Kind; 8] =
    [Kind::Read, Kind::List, Kind::Search, Kind::Write, Kind::Edit, Kind::Shell, Kind::Finish, Kind::SubAgent];
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Spec {
    pub name: Box<[u8]>,
    pub description: Box<[u8]>,
    pub parameters: Box<[u8]>,
}

#[must_use]
pub fn name(provider: &Provider, kind: Kind) -> &[u8] {
    match provider {
        Provider::Anthropic => match kind {
            Kind::Read => anthropic::READ_TOOL,
            Kind::List => anthropic::LIST_TOOL,
            Kind::Search => anthropic::SEARCH_TOOL,
            Kind::Write => anthropic::WRITE_TOOL,
            Kind::Edit => anthropic::EDIT_TOOL,
            Kind::Shell => anthropic::SHELL_TOOL,
            Kind::Finish => anthropic::FINISH_TOOL,
            Kind::SubAgent => anthropic::SUBAGENT_TOOL,
        },
        Provider::OpenAi => match kind {
            Kind::Read => identity::READ_TOOL,
            Kind::List => identity::LIST_TOOL,
            Kind::Search => identity::SEARCH_TOOL,
            Kind::Write => identity::WRITE_TOOL,
            Kind::Edit => identity::EDIT_TOOL,
            Kind::Shell => identity::SHELL_TOOL,
            Kind::Finish => identity::FINISH_TOOL,
            Kind::SubAgent => identity::SUBAGENT_TOOL,
        },
    }
}
#[must_use]
pub fn allowed(kind: Kind, grants: tools::Grants, served: &[llm::Served]) -> bool {
    match kind {
        Kind::Read | Kind::List | Kind::Search => grants.inspect,
        Kind::Write | Kind::Edit => grants.modify,
        Kind::Shell => grants.shell,
        Kind::Finish | Kind::SubAgent => {
            for offered in served {
                match offered {
                    llm::Served::Finish if kind == Kind::Finish => return true,
                    llm::Served::SubAgent if kind == Kind::SubAgent => return true,
                    llm::Served::Finish | llm::Served::SubAgent => {}
                }
            }
            false
        }
    }
}
pub fn offer(
    provider: &Provider,
    grants: tools::Grants,
    served: &[llm::Served],
    limits: &Limits,
) -> Result<Box<[Spec]>, Error> {
    if served.len() > 2 {
        return Err(Error::TooLarge);
    }
    let mut specs = List::with_capacity(8);
    for kind in ALL {
        if allowed(kind, grants, served) {
            if specs.len() >= limits.parts {
                return Err(Error::TooLarge);
            }
            specs
                .push(Spec {
                    name: bytes::copy_of(name(provider, kind)),
                    description: bytes::copy_of(description(kind)),
                    parameters: bytes::copy_of(schema(kind)),
                })
                .expect("eight distinct tools");
        }
    }
    Ok(specs.into_boxed())
}
fn description(kind: Kind) -> &'static [u8] {
    match kind {
        Kind::Read => b"Read lines of a file. Read a file before changing it.",
        Kind::List => b"List a directory.",
        Kind::Search => b"Search files with a regular expression and optional name glob.",
        Kind::Write => b"Create or replace a file.",
        Kind::Edit => b"Replace a snippet, once or everywhere.",
        Kind::Shell => b"Run a shell command; timeout is whole seconds.",
        Kind::Finish => b"Declare a change (title and body) or a verdict (verdict, body and children). Give exactly one of title and verdict. The run checks its outcome contract.",
        Kind::SubAgent => b"Delegate a brief with requested tool families, optional model and budget share.",
    }
}
fn schema(kind: Kind) -> &'static [u8] {
    match kind {
        Kind::Read => br#"{"type":"object","properties":{"path":{"type":"string"},"skip":{"type":"integer","minimum":0},"lines":{"type":"integer","minimum":0}},"required":["path"]}"#,
        Kind::List => br#"{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}"#,
        Kind::Search => br#"{"type":"object","properties":{"path":{"type":"string"},"pattern":{"type":"string"},"glob":{"type":"string"}},"required":["path","pattern"]}"#,
        Kind::Write => br#"{"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path","content"]}"#,
        Kind::Edit => br#"{"type":"object","properties":{"path":{"type":"string"},"old":{"type":"string"},"new":{"type":"string"},"all":{"type":"boolean"}},"required":["path","old","new"]}"#,
        Kind::Shell => br#"{"type":"object","properties":{"command":{"type":"string"},"timeout":{"type":"integer","minimum":0}},"required":["command"]}"#,
        Kind::Finish => br#"{"type":"object","properties":{"title":{"type":"string"},"body":{"type":"string"},"verdict":{"type":"string"},"children":{"type":"array","items":{"type":"object","properties":{"kind":{"type":"string"},"fields":{"type":"object","additionalProperties":{"type":"string"}}},"required":["kind","fields"]}}},"required":["body"],"anyOf":[{"required":["title"]},{"required":["verdict"]}]}"#,
        Kind::SubAgent => br#"{"type":"object","properties":{"brief":{"type":"string"},"tools":{"type":"array","items":{"type":"string","enum":["inspect","modify","shell"]}},"forge":{"type":"boolean"},"agents":{"type":"boolean"},"llm":{"type":"string"},"share":{"type":"object","properties":{"turns":{"type":"integer","minimum":0},"input":{"type":"integer","minimum":0},"output":{"type":"integer","minimum":0},"cache_read":{"type":"integer","minimum":0},"cache_write":{"type":"integer","minimum":0}}}},"required":["brief"]}"#,
    }
}

#[must_use]
pub fn decode(
    provider: &Provider,
    grants: tools::Grants,
    served: &[llm::Served],
    tool_name: &[u8],
    input: &[u8],
    too_large: bool,
    limits: &Limits,
) -> llm::Decoded {
    let mut found = None;
    for kind in ALL {
        if name(provider, kind) == tool_name && allowed(kind, grants, served) {
            found = Some(kind);
        }
    }
    let Some(kind) = found else {
        return llm::Decoded::Invalid { problem: llm::Problem::UnknownTool };
    };
    if too_large {
        return llm::Decoded::Invalid { problem: llm::Problem::TooLarge };
    }
    let json = match openai::Json::from_bytes(input, &limits.tool_json()) {
        Ok(json) => json,
        Err(openai::DecodeError::TooLarge) => return llm::Decoded::Invalid { problem: llm::Problem::TooLarge },
        Err(openai::DecodeError::Malformed | openai::DecodeError::Missing | openai::DecodeError::WrongType) => {
            return llm::Decoded::Invalid { problem: llm::Problem::NotAnObject };
        }
    };
    let tokens = json.as_tokens();
    if tokens.first() != Some(&Token::ObjectStart) {
        return llm::Decoded::Invalid { problem: llm::Problem::NotAnObject };
    }
    match decoded(kind, tokens, limits) {
        Ok(call) => call,
        Err(problem) => llm::Decoded::Invalid { problem },
    }
}
fn decoded(kind: Kind, tokens: &[Token], limits: &Limits) -> Result<llm::Decoded, llm::Problem> {
    let call = match kind {
        Kind::Read => tools::Call::Read {
            path: path(&text(tokens, b"path", true)?.ok_or(missing(b"path"))?, limits)?,
            skip: small(tokens, b"skip", 0)?,
            lines: optional_small(tokens, b"lines")?,
        },
        Kind::List => tools::Call::List { path: path(&text(tokens, b"path", true)?.ok_or(missing(b"path"))?, limits)? },
        Kind::Search => tools::Call::Search {
            path: path(&text(tokens, b"path", true)?.ok_or(missing(b"path"))?, limits)?,
            pattern: text(tokens, b"pattern", true)?.ok_or(missing(b"pattern"))?,
            glob: text(tokens, b"glob", false)?,
        },
        Kind::Write => tools::Call::Write {
            path: path(&text(tokens, b"path", true)?.ok_or(missing(b"path"))?, limits)?,
            content: text(tokens, b"content", true)?.ok_or(missing(b"content"))?,
        },
        Kind::Edit => tools::Call::Edit {
            path: path(&text(tokens, b"path", true)?.ok_or(missing(b"path"))?, limits)?,
            old: text(tokens, b"old", true)?.ok_or(missing(b"old"))?,
            new: text(tokens, b"new", true)?.ok_or(missing(b"new"))?,
            all: boolean(tokens, b"all", false)?,
        },
        Kind::Shell => {
            let timeout = match number(tokens, b"timeout")? {
                Some(n) => Some(Duration::from_nanos(n.checked_mul(1_000_000_000).ok_or(bad(b"timeout"))?)),
                None => None,
            };
            tools::Call::Shell { command: text(tokens, b"command", true)?.ok_or(missing(b"command"))?, timeout }
        }
        Kind::Finish => return Ok(llm::Decoded::Served { ask: finish(tokens, limits)? }),
        Kind::SubAgent => return Ok(llm::Decoded::Served { ask: subagent(tokens, limits)? }),
    };
    Ok(llm::Decoded::Owned { call })
}
fn finish(tokens: &[Token], limits: &Limits) -> Result<run::Ask, llm::Problem> {
    let body = text(tokens, b"body", true)?.ok_or(missing(b"body"))?;
    let verdict = text(tokens, b"verdict", false)?;
    let title = text(tokens, b"title", false)?;
    match verdict {
        None => Ok(run::Ask::Finish {
            outcome: run::outcome::Declared::Change(run::outcome::Change {
                title: title.ok_or(missing(b"title"))?,
                body,
            }),
        }),
        Some(name) => {
            if title.is_some() {
                return Err(bad(b"title"));
            }
            let mut children = List::with_capacity(limits.parts);
            if let Some(offset) = field(tokens, b"children")? {
                let array = value_at(tokens, offset)?;
                let offsets = array_offsets(array, limits.parts, b"children")?;
                for offset in &offsets {
                    let child = value_at(array, *offset)?;
                    object(child, b"children")?;
                    let kind = text(child, b"kind", true)?.ok_or(missing(b"kind"))?;
                    let fields_offset = field(child, b"fields")?.ok_or(missing(b"fields"))?;
                    let fields = fields(value_at(child, fields_offset)?, limits)?;
                    children.push(run::outcome::Child { kind, fields }).or(Err(llm::Problem::TooLarge))?;
                }
            }
            Ok(run::Ask::Finish {
                outcome: run::outcome::Declared::Verdict(run::outcome::Verdict {
                    name,
                    body,
                    children: children.into_boxed(),
                }),
            })
        }
    }
}
fn fields(tokens: &[Token], limits: &Limits) -> Result<Box<[run::outcome::Field]>, llm::Problem> {
    object(tokens, b"fields")?;
    let mut values = List::with_capacity(limits.parts);
    let mut at = 1;
    for _ in 0..tokens.len() {
        match tokens.get(at) {
            Some(Token::ObjectEnd) => return Ok(values.into_boxed()),
            Some(Token::Key(name)) => {
                if name.is_empty() || name.contains(&0) {
                    return Err(bad(b"fields"));
                }
                let offset = field(tokens, name)?.ok_or(missing(name))?;
                let value = string_at(tokens, offset, name)?;
                values.push(run::outcome::Field { name: name.clone(), value }).or(Err(llm::Problem::TooLarge))?;
                at = span(tokens, at.saturating_add(1))?;
            }
            Some(
                Token::ObjectStart
                | Token::ArrayStart
                | Token::ArrayEnd
                | Token::String(_)
                | Token::Number(_)
                | Token::True
                | Token::False
                | Token::Null,
            )
            | None => return Err(bad(b"fields")),
        }
    }
    Err(bad(b"fields"))
}
fn subagent(tokens: &[Token], limits: &Limits) -> Result<run::Ask, llm::Problem> {
    let brief = text(tokens, b"brief", true)?.ok_or(missing(b"brief"))?;
    let mut tools = run::charter::Tools { inspect: false, modify: false, shell: false };
    if let Some(offset) = field(tokens, b"tools")? {
        let array = value_at(tokens, offset)?;
        for offset in &array_offsets(array, 3, b"tools")? {
            let name = match value_at(array, *offset)? {
                [Token::String(name)] => name,
                []
                | [_, _, ..]
                | [
                    Token::ObjectStart
                    | Token::ObjectEnd
                    | Token::ArrayStart
                    | Token::ArrayEnd
                    | Token::Key(_)
                    | Token::Number(_)
                    | Token::True
                    | Token::False
                    | Token::Null,
                ] => {
                    return Err(wrong(b"tools"));
                }
            };
            match name.as_ref() {
                b"inspect" if !tools.inspect => tools.inspect = true,
                b"modify" if !tools.modify => tools.modify = true,
                b"shell" if !tools.shell => tools.shell = true,
                _ => return Err(bad(b"tools")),
            }
        }
    }
    let families = run::charter::Families {
        tools,
        forge: boolean(tokens, b"forge", false)?,
        agents: boolean(tokens, b"agents", false)?,
    };
    let llm = text(tokens, b"llm", false)?;
    if let Some(name) = &llm
        && (name.is_empty()
            || name.contains(&0)
            || name.len() > usize::try_from(limits.name_bytes).expect("u32 fits usize"))
    {
        return Err(bad(b"llm"));
    }
    let share = match field(tokens, b"share")? {
        None => None,
        Some(offset) => {
            let share = value_at(tokens, offset)?;
            object(share, b"share")?;
            Some(run::Spend {
                turns: small(share, b"turns", u32::MAX)?,
                input: number(share, b"input")?.unwrap_or(u64::MAX),
                output: number(share, b"output")?.unwrap_or(u64::MAX),
                cache_read: number(share, b"cache_read")?.unwrap_or(u64::MAX),
                cache_write: number(share, b"cache_write")?.unwrap_or(u64::MAX),
            })
        }
    };
    Ok(run::Ask::SubAgent { brief, families, llm, share })
}
fn path(text: &[u8], limits: &Limits) -> Result<tools::Path, llm::Problem> {
    if text.is_empty() {
        return Err(bad(b"path"));
    }
    let absolute = text.first() == Some(&b'/');
    let count = u32::try_from(text.len()).or(Err(llm::Problem::TooLarge))?.min(limits.input_bytes);
    let mut parts = List::with_capacity(count);
    let mut start = usize::from(absolute);
    let first = start;
    for at in first..=text.len() {
        if at == text.len() || text.get(at) == Some(&b'/') {
            let part = text.get(start..at).ok_or(bad(b"path"))?;
            if part.is_empty() {
                if absolute && text.len() == 1 {
                    break;
                }
                return Err(bad(b"path"));
            }
            let value = match part {
                b"." => tools::Part::Current,
                b".." => tools::Part::Parent,
                name => tools::Part::Name { name: tools::Name::new(bytes::copy_of(name)).ok_or(bad(b"path"))? },
            };
            parts.push(value).or(Err(llm::Problem::TooLarge))?;
            start = at.saturating_add(1);
        }
    }
    Ok(tools::Path { absolute, parts: parts.into_boxed() })
}
fn object(tokens: &[Token], name: &[u8]) -> Result<(), llm::Problem> {
    if tokens.first() == Some(&Token::ObjectStart) { Ok(()) } else { Err(wrong(name)) }
}
pub(crate) fn span(tokens: &[Token], at: usize) -> Result<usize, llm::Problem> {
    match tokens.get(at) {
        Some(Token::ObjectStart | Token::ArrayStart) => {
            let mut depth: u32 = 0;
            for (offset, token) in tokens.get(at..).ok_or(llm::Problem::NotAnObject)?.iter().enumerate() {
                match token {
                    Token::ObjectStart | Token::ArrayStart => {
                        depth = depth.checked_add(1).ok_or(llm::Problem::TooLarge)?;
                    }
                    Token::ObjectEnd | Token::ArrayEnd => {
                        depth = depth.checked_sub(1).ok_or(llm::Problem::NotAnObject)?;
                        if depth == 0 {
                            return at
                                .checked_add(offset)
                                .ok_or(llm::Problem::TooLarge)?
                                .checked_add(1)
                                .ok_or(llm::Problem::TooLarge);
                        }
                    }
                    Token::Key(_) | Token::String(_) | Token::Number(_) | Token::True | Token::False | Token::Null => {}
                }
            }
            Err(llm::Problem::NotAnObject)
        }
        Some(Token::String(_) | Token::Number(_) | Token::True | Token::False | Token::Null) => {
            at.checked_add(1).ok_or(llm::Problem::TooLarge)
        }
        Some(Token::ObjectEnd | Token::ArrayEnd | Token::Key(_)) | None => Err(llm::Problem::NotAnObject),
    }
}
pub(crate) fn field(tokens: &[Token], name: &[u8]) -> Result<Option<u32>, llm::Problem> {
    object(tokens, name)?;
    let mut at: usize = 1;
    let mut found = None;
    for _ in 0..tokens.len() {
        match tokens.get(at) {
            Some(Token::ObjectEnd) => return Ok(found),
            Some(Token::Key(key)) => {
                let start = at.checked_add(1).ok_or(llm::Problem::TooLarge)?;
                at = span(tokens, start)?;
                if key.as_ref() == name {
                    if found.is_some() {
                        return Err(bad(name));
                    }
                    found = Some(u32::try_from(start).or(Err(llm::Problem::TooLarge))?);
                }
            }
            Some(
                Token::ObjectStart
                | Token::ArrayStart
                | Token::ArrayEnd
                | Token::String(_)
                | Token::Number(_)
                | Token::True
                | Token::False
                | Token::Null,
            )
            | None => return Err(llm::Problem::NotAnObject),
        }
    }
    Err(llm::Problem::NotAnObject)
}
pub(crate) fn value_at(tokens: &[Token], offset: u32) -> Result<&[Token], llm::Problem> {
    let start = usize::try_from(offset).expect("u32 fits usize");
    tokens.get(start..span(tokens, start)?).ok_or(llm::Problem::NotAnObject)
}
fn string_at(tokens: &[Token], offset: u32, name: &[u8]) -> Result<Box<[u8]>, llm::Problem> {
    match value_at(tokens, offset)? {
        [Token::String(text)] => Ok(text.clone()),
        _ => Err(wrong(name)),
    }
}
fn text(tokens: &[Token], name: &[u8], required: bool) -> Result<Option<Box<[u8]>>, llm::Problem> {
    match field(tokens, name)? {
        Some(offset) => Ok(Some(string_at(tokens, offset, name)?)),
        None if required => Err(missing(name)),
        None => Ok(None),
    }
}
fn number(tokens: &[Token], name: &[u8]) -> Result<Option<u64>, llm::Problem> {
    let Some(offset) = field(tokens, name)? else {
        return Ok(None);
    };
    let digits = match value_at(tokens, offset)? {
        [Token::Number(digits)] => digits,
        []
        | [_, _, ..]
        | [
            Token::ObjectStart
            | Token::ObjectEnd
            | Token::ArrayStart
            | Token::ArrayEnd
            | Token::Key(_)
            | Token::String(_)
            | Token::True
            | Token::False
            | Token::Null,
        ] => {
            return Err(wrong(name));
        }
    };
    let mut n: u64 = 0;
    for &byte in digits {
        if !byte.is_ascii_digit() {
            return Err(bad(name));
        }
        n = n.checked_mul(10).ok_or(bad(name))?.checked_add(u64::from(byte.saturating_sub(b'0'))).ok_or(bad(name))?;
    }
    Ok(Some(n))
}
fn optional_small(tokens: &[Token], name: &[u8]) -> Result<Option<u32>, llm::Problem> {
    match number(tokens, name)? {
        Some(n) => Ok(Some(u32::try_from(n).or(Err(bad(name)))?)),
        None => Ok(None),
    }
}
fn small(tokens: &[Token], name: &[u8], fallback: u32) -> Result<u32, llm::Problem> {
    Ok(optional_small(tokens, name)?.unwrap_or(fallback))
}
fn boolean(tokens: &[Token], name: &[u8], fallback: bool) -> Result<bool, llm::Problem> {
    match field(tokens, name)? {
        None => Ok(fallback),
        Some(offset) => match value_at(tokens, offset)? {
            [Token::True] => Ok(true),
            [Token::False] => Ok(false),
            _ => Err(wrong(name)),
        },
    }
}
fn array_offsets(tokens: &[Token], max: u32, name: &[u8]) -> Result<List<u32>, llm::Problem> {
    if tokens.first() != Some(&Token::ArrayStart) {
        return Err(wrong(name));
    }
    let mut offsets = List::with_capacity(max);
    let mut at: usize = 1;
    for _ in 0..tokens.len() {
        if tokens.get(at) == Some(&Token::ArrayEnd) {
            return Ok(offsets);
        }
        offsets.push(u32::try_from(at).or(Err(llm::Problem::TooLarge))?).or(Err(llm::Problem::TooLarge))?;
        at = span(tokens, at)?;
    }
    Err(bad(name))
}
fn missing(name: &[u8]) -> llm::Problem {
    llm::Problem::Missing { field: bytes::copy_of(name) }
}
fn wrong(name: &[u8]) -> llm::Problem {
    llm::Problem::WrongType { field: bytes::copy_of(name) }
}
fn bad(name: &[u8]) -> llm::Problem {
    llm::Problem::BadValue { field: bytes::copy_of(name) }
}
