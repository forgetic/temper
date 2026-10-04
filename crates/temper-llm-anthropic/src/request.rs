use crate::{DecodeError, Json, Limits, identity, json};
use alloc::boxed::Box;
use skein_json::{Token, writer::Encoder};
use skein_lib::{List, bytes};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Role {
    User,
    Assistant,
}
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Block {
    Text { text: Box<[u8]> },
    ToolUse { id: Box<[u8]>, name: Box<[u8]>, input: Json },
    ToolResult { id: Box<[u8]>, content: Box<[u8]>, error: bool },
    Opaque { value: Json },
}
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Message {
    pub role: Role,
    pub content: Box<[Block]>,
}
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Tool {
    pub name: Box<[u8]>,
    pub description: Box<[u8]>,
    pub schema: Json,
}
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Request {
    pub model: Box<[u8]>,
    /// Complete ordered system blocks, including the deployment's identity.
    pub system: Box<[Box<[u8]>]>,
    pub tools: Box<[Tool]>,
    pub messages: Box<[Message]>,
    pub max_tokens: u32,
    pub thinking_budget: Option<u32>,
    pub metadata: Option<Json>,
    pub context_management: Option<Json>,
}

pub fn encode_request(request: &Request, limits: &Limits) -> Result<Box<[u8]>, DecodeError> {
    let len = measure_request(request, limits)?;
    let bounded = skein_json::writer::Limits { depth: limits.depth, length: limits.request_bytes };
    let mut write = Encoder::write(len, &bounded);
    write_request(&mut write, request);
    Ok(write.finish())
}
/// Validates and measures without allocating the request's body.
pub fn measure_request(request: &Request, limits: &Limits) -> Result<u32, DecodeError> {
    validate(request, limits)?;
    let bounded = skein_json::writer::Limits { depth: limits.depth, length: limits.request_bytes };
    let mut measure = Encoder::measure(&bounded);
    write_request(&mut measure, request);
    crate::common::measured(measure)
}
fn validate(request: &Request, limits: &Limits) -> Result<(), DecodeError> {
    if request.max_tokens == 0
        || request.messages.len() > usize::try_from(limits.parts).expect("u32 fits usize")
        || request.tools.len() > usize::try_from(limits.parts).expect("u32 fits usize")
    {
        return Err(DecodeError::TooLarge);
    }
    if let Some(budget) = request.thinking_budget
        && (budget == 0 || budget >= request.max_tokens)
    {
        return Err(DecodeError::Malformed);
    }
    for tool in &request.tools {
        if tool.schema.as_tokens().first() != Some(&Token::ObjectStart) {
            return Err(DecodeError::WrongType);
        }
    }
    for message in &request.messages {
        if message.content.len() > usize::try_from(limits.parts).expect("u32 fits usize") {
            return Err(DecodeError::TooLarge);
        }
        for block in &message.content {
            match block {
                Block::ToolUse { input, .. } => {
                    if message.role != Role::Assistant {
                        return Err(DecodeError::WrongType);
                    }
                    if input.as_tokens().first() != Some(&Token::ObjectStart) {
                        return Err(DecodeError::WrongType);
                    }
                }
                Block::Opaque { value } => {
                    if message.role != Role::Assistant {
                        return Err(DecodeError::WrongType);
                    }
                    if value.as_tokens().first() != Some(&Token::ObjectStart) {
                        return Err(DecodeError::WrongType);
                    }
                }
                Block::ToolResult { .. } => {
                    if message.role != Role::User {
                        return Err(DecodeError::WrongType);
                    }
                }
                Block::Text { .. } => {}
            }
        }
    }
    Ok(())
}
fn cache(out: &mut Encoder) {
    out.key(b"cache_control");
    out.object_start();
    out.key(b"type");
    out.string(b"ephemeral");
    out.key(b"ttl");
    out.string(identity::CACHE_TTL);
    out.object_end();
}
fn write_request(out: &mut Encoder, request: &Request) {
    out.object_start();
    out.key(b"model");
    out.string(&request.model);
    out.key(b"max_tokens");
    out.unsigned(u64::from(request.max_tokens));
    out.key(b"stream");
    out.boolean(true);
    out.key(b"system");
    out.array_start();
    for (at, text) in request.system.iter().enumerate() {
        out.object_start();
        out.key(b"type");
        out.string(b"text");
        out.key(b"text");
        out.string(text);
        if at.checked_add(1) == Some(request.system.len()) {
            cache(out);
        }
        out.object_end();
    }
    out.array_end();
    out.key(b"tools");
    out.array_start();
    for (at, tool) in request.tools.iter().enumerate() {
        out.object_start();
        out.key(b"name");
        out.string(&tool.name);
        out.key(b"description");
        out.string(&tool.description);
        out.key(b"input_schema");
        tool.schema.write(out);
        if at.checked_add(1) == Some(request.tools.len()) {
            cache(out);
        }
        out.object_end();
    }
    out.array_end();
    out.key(b"messages");
    out.array_start();
    // Only the last two user messages are marked, giving at most four marks.
    let mut users: usize = 0;
    for message in &request.messages {
        if message.role == Role::User {
            users = users.saturating_add(1);
        }
    }
    for message in &request.messages {
        let mark = message.role == Role::User && users <= 2;
        if message.role == Role::User {
            users = users.saturating_sub(1);
        }
        out.object_start();
        out.key(b"role");
        out.string(match message.role {
            Role::User => b"user",
            Role::Assistant => b"assistant",
        });
        out.key(b"content");
        out.array_start();
        for (at, block) in message.content.iter().enumerate() {
            write_block(out, block, mark && at.checked_add(1) == Some(message.content.len()));
        }
        out.array_end();
        out.object_end();
    }
    out.array_end();
    if let Some(budget) = request.thinking_budget {
        out.key(b"thinking");
        out.object_start();
        out.key(b"type");
        out.string(b"enabled");
        out.key(b"budget_tokens");
        out.unsigned(u64::from(budget));
        out.key(b"display");
        out.string(b"omitted");
        out.object_end();
    }
    if let Some(metadata) = &request.metadata {
        out.key(b"metadata");
        metadata.write(out);
    }
    if let Some(context) = &request.context_management {
        out.key(b"context_management");
        context.write(out);
    }
    out.object_end();
}
pub(crate) fn write_block(out: &mut Encoder, block: &Block, mark: bool) {
    match block {
        Block::Opaque { value } => value.write(out),
        Block::Text { text } => {
            out.object_start();
            out.key(b"type");
            out.string(b"text");
            out.key(b"text");
            out.string(text);
            if mark {
                cache(out);
            }
            out.object_end();
        }
        Block::ToolUse { id, name, input } => {
            out.object_start();
            out.key(b"type");
            out.string(b"tool_use");
            out.key(b"id");
            out.string(id);
            out.key(b"name");
            out.string(name);
            out.key(b"input");
            input.write(out);
            if mark {
                cache(out);
            }
            out.object_end();
        }
        Block::ToolResult { id, content, error } => {
            out.object_start();
            out.key(b"type");
            out.string(b"tool_result");
            out.key(b"tool_use_id");
            out.string(id);
            out.key(b"content");
            out.string(content);
            out.key(b"is_error");
            out.boolean(*error);
            if mark {
                cache(out);
            }
            out.object_end();
        }
    }
}

pub fn decode_request(value: &Json, limits: &Limits) -> Result<Request, DecodeError> {
    let mut admission =
        Encoder::measure(&skein_json::writer::Limits { depth: limits.depth, length: limits.request_bytes });
    value.write(&mut admission);
    let _length = crate::common::measured(admission)?;
    let tokens = value.as_tokens();
    if !json::boolean(json::value_at(tokens, json::required(tokens, b"stream")?)?)? {
        return Err(DecodeError::Malformed);
    }
    let model = json::text(json::value_at(tokens, json::required(tokens, b"model")?)?)?;
    let max = json::unsigned(json::value_at(tokens, json::required(tokens, b"max_tokens")?)?)?;
    let Ok(max_tokens) = u32::try_from(max) else {
        return Err(DecodeError::TooLarge);
    };
    let system_tokens = json::value_at(tokens, json::required(tokens, b"system")?)?;
    let mut system = List::with_capacity(limits.parts);
    match system_tokens {
        [Token::String(text)] => push_text(&mut system, text.clone())?,
        _ => {
            for &offset in &json::array(system_tokens, limits.parts)? {
                let block = json::value_at(system_tokens, offset)?;
                if json::text_ref(json::value_at(block, json::required(block, b"type")?)?)? != b"text" {
                    return Err(DecodeError::WrongType);
                }
                push_text(&mut system, json::text(json::value_at(block, json::required(block, b"text")?)?)?)?;
            }
        }
    }
    let mut tools = List::with_capacity(limits.parts);
    if let Some(values) = json::optional_at(tokens, json::field(tokens, b"tools")?)? {
        for &offset in &json::array(values, limits.parts)? {
            let tool = json::value_at(values, offset)?;
            let description = match json::optional_at(tool, json::field(tool, b"description")?)? {
                Some(value) => json::text(value)?,
                None => bytes::copy_of(b""),
            };
            let tool = Tool {
                name: json::text(json::value_at(tool, json::required(tool, b"name")?)?)?,
                description,
                schema: Json::from_tokens(json::value_at(tool, json::required(tool, b"input_schema")?)?, limits)?,
            };
            if tools.push(tool).is_err() {
                return Err(DecodeError::TooLarge);
            }
        }
    }
    let values = json::value_at(tokens, json::required(tokens, b"messages")?)?;
    let mut messages = List::with_capacity(limits.parts);
    for &offset in &json::array(values, limits.parts)? {
        let message = json::value_at(values, offset)?;
        let role = match json::text_ref(json::value_at(message, json::required(message, b"role")?)?)? {
            b"user" => Role::User,
            b"assistant" => Role::Assistant,
            _ => return Err(DecodeError::WrongType),
        };
        let content_values = json::value_at(message, json::required(message, b"content")?)?;
        let mut content = List::with_capacity(limits.parts);
        match content_values {
            [Token::String(text)] => push_block(&mut content, Block::Text { text: text.clone() })?,
            _ => {
                for &offset in &json::array(content_values, limits.parts)? {
                    push_block(&mut content, read_block(json::value_at(content_values, offset)?, limits)?)?;
                }
            }
        }
        if messages.push(Message { role, content: content.into_boxed() }).is_err() {
            return Err(DecodeError::TooLarge);
        }
    }
    let thinking_budget = match json::optional_at(tokens, json::field(tokens, b"thinking")?)? {
        Some(value) => {
            if json::text_ref(json::value_at(value, json::required(value, b"type")?)?)? != b"enabled" {
                return Err(DecodeError::WrongType);
            }
            let n = json::unsigned(json::value_at(value, json::required(value, b"budget_tokens")?)?)?;
            Some(match u32::try_from(n) {
                Ok(n) => n,
                Err(_) => return Err(DecodeError::TooLarge),
            })
        }
        None => None,
    };
    let metadata = optional_json(tokens, b"metadata", limits)?;
    let context_management = optional_json(tokens, b"context_management", limits)?;
    let request = Request {
        model,
        system: system.into_boxed(),
        tools: tools.into_boxed(),
        messages: messages.into_boxed(),
        max_tokens,
        thinking_budget,
        metadata,
        context_management,
    };
    validate(&request, limits)?;
    Ok(request)
}
fn optional_json(tokens: &[Token], name: &[u8], limits: &Limits) -> Result<Option<Json>, DecodeError> {
    match json::optional_at(tokens, json::field(tokens, name)?)? {
        Some(value) => Ok(Some(Json::from_tokens(value, limits)?)),
        None => Ok(None),
    }
}
fn push_text(out: &mut List<Box<[u8]>>, text: Box<[u8]>) -> Result<(), DecodeError> {
    match out.push(text) {
        Ok(()) => Ok(()),
        Err(_) => Err(DecodeError::TooLarge),
    }
}
fn push_block(out: &mut List<Block>, block: Block) -> Result<(), DecodeError> {
    match out.push(block) {
        Ok(()) => Ok(()),
        Err(_) => Err(DecodeError::TooLarge),
    }
}
pub(crate) fn read_block(tokens: &[Token], limits: &Limits) -> Result<Block, DecodeError> {
    match json::text_ref(json::value_at(tokens, json::required(tokens, b"type")?)?)? {
        b"text" => Ok(Block::Text { text: json::text(json::value_at(tokens, json::required(tokens, b"text")?)?)? }),
        b"tool_use" => Ok(Block::ToolUse {
            id: json::text(json::value_at(tokens, json::required(tokens, b"id")?)?)?,
            name: json::text(json::value_at(tokens, json::required(tokens, b"name")?)?)?,
            input: Json::from_tokens(json::value_at(tokens, json::required(tokens, b"input")?)?, limits)?,
        }),
        b"tool_result" => Ok(Block::ToolResult {
            id: json::text(json::value_at(tokens, json::required(tokens, b"tool_use_id")?)?)?,
            content: result_text(json::value_at(tokens, json::required(tokens, b"content")?)?, limits)?,
            error: match json::optional_at(tokens, json::field(tokens, b"is_error")?)? {
                Some(value) => json::boolean(value)?,
                None => false,
            },
        }),
        _ => Ok(Block::Opaque { value: Json::from_tokens(tokens, limits)? }),
    }
}

fn result_text(tokens: &[Token], limits: &Limits) -> Result<Box<[u8]>, DecodeError> {
    match tokens {
        [Token::String(text)] => Ok(text.clone()),
        _ => {
            let mut text = List::with_capacity(limits.request_bytes);
            for &offset in &json::array(tokens, limits.parts)? {
                let block = json::value_at(tokens, offset)?;
                if json::text_ref(json::value_at(block, json::required(block, b"type")?)?)? != b"text" {
                    return Err(DecodeError::WrongType);
                }
                crate::common::append(
                    &mut text,
                    json::text_ref(json::value_at(block, json::required(block, b"text")?)?)?,
                )?;
            }
            Ok(text.into_boxed())
        }
    }
}
