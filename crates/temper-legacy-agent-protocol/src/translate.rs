//! Conversation translation. Domains hold names and opaque bytes; this layer
//! owns JSON, provider settings and the offered tool grammar (llm.md, 5–6).
use crate::{Error, Limits, limits, render, tools};
use alloc::boxed::Box;
use skein_json::Token;
use skein_lib::{List, bytes};
use temper_channel::wire::{EndpointDescriptor, Provider};
use temper_legacy_agent_domain::llm;
use temper_legacy_llm_anthropic as anthropic;
use temper_legacy_llm_openai as openai;
type Identifier = (Box<[u8]>, Option<Box<[u8]>>);

/// Deployment-supplied identity blocks/metadata. Historical headers do not
/// establish the current billing line; no value is invented here.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct AnthropicIdentity {
    pub system: Box<[Box<[u8]>]>,
    pub metadata: Option<anthropic::Json>,
    pub context_management: Option<anthropic::Json>,
}

pub fn anthropic(
    prompt: llm::Prompt,
    endpoint: &EndpointDescriptor,
    identity: &AnthropicIdentity,
    limits: &Limits,
) -> Result<anthropic::Request, Error> {
    admit(&prompt, endpoint, limits)?;
    if endpoint.provider != Provider::Anthropic {
        return Err(Error::Endpoint);
    }
    let specs = tools::offer(&Provider::Anthropic, prompt.tools, &prompt.served, limits)?;
    let mut offered = List::with_capacity(8);
    let bounded = limits.anthropic();
    for spec in specs {
        let schema = match anthropic::Json::from_bytes(&spec.parameters, &bounded) {
            Ok(value) => value,
            Err(error) => return Err(limits::anthropic_error(error)),
        };
        offered
            .push(anthropic::Tool { name: spec.name, description: spec.description, schema })
            .expect("eight schemas");
    }
    if identity.system.len() >= usize::try_from(limits.parts).expect("u32 fits usize") {
        return Err(Error::TooLarge);
    }
    let count = u32::try_from(identity.system.len()).or(Err(Error::TooLarge))?.checked_add(1).ok_or(Error::TooLarge)?;
    let mut system = List::with_capacity(count);
    for block in &identity.system {
        system.push(block.clone()).expect("the identity block count");
    }
    system.push(prompt.system).expect("room for the prompt's system text");
    let mut messages = List::with_capacity(u32::try_from(prompt.messages.len()).or(Err(Error::TooLarge))?);
    for message in prompt.messages {
        let role = match message.role {
            llm::Role::User => anthropic::Role::User,
            llm::Role::Assistant => anthropic::Role::Assistant,
        };
        let mut blocks = List::with_capacity(u32::try_from(message.content.len()).or(Err(Error::TooLarge))?);
        for block in message.content {
            let wire = match block {
                llm::Block::Text { text } => anthropic::Block::Text { text },
                llm::Block::Opaque { bytes } => {
                    let value = match anthropic::Json::from_bytes(&bytes, &bounded) {
                        Ok(value) => value,
                        Err(error) => return Err(limits::anthropic_error(error)),
                    };
                    anthropic::Block::Opaque { value }
                }
                llm::Block::ToolCall { id, name, input } => {
                    let input = historical_anthropic_input(&input, &bounded)?;
                    anthropic::Block::ToolUse { id, name, input }
                }
                llm::Block::ToolResult { id, result } => {
                    let (content, error) = render::result(&result, limits.render_bytes)?;
                    anthropic::Block::ToolResult { id, content, error }
                }
            };
            blocks.push(wire).expect("one block per source block");
        }
        messages
            .push(anthropic::Message { role, content: blocks.into_boxed() })
            .expect("one message per source message");
    }
    let request = anthropic::Request {
        model: prompt.model,
        system: system.into_boxed(),
        tools: offered.into_boxed(),
        messages: messages.into_boxed(),
        max_tokens: prompt.max_tokens,
        thinking_budget: endpoint.thinking,
        metadata: identity.metadata.clone(),
        context_management: identity.context_management.clone(),
    };
    match anthropic::measure_request(&request, &bounded) {
        Ok(_) => Ok(request),
        Err(error) => Err(limits::anthropic_error(error)),
    }
}

pub fn openai(
    prompt: llm::Prompt,
    endpoint: &EndpointDescriptor,
    session: &[u8],
    limits: &Limits,
) -> Result<openai::Request, Error> {
    admit(&prompt, endpoint, limits)?;
    if endpoint.provider != Provider::OpenAi {
        return Err(Error::Endpoint);
    }
    let specs = tools::offer(&Provider::OpenAi, prompt.tools, &prompt.served, limits)?;
    let bounded = limits.openai();
    let mut offered = List::with_capacity(8);
    for spec in specs {
        let schema = match openai::Json::from_bytes(&spec.parameters, &bounded) {
            Ok(value) => value,
            Err(error) => return Err(limits::openai_error(error)),
        };
        offered.push(openai::Tool { name: spec.name, description: spec.description, schema }).expect("eight schemas");
    }
    let mut input = List::with_capacity(limits.parts);
    for message in prompt.messages {
        let role = match message.role {
            llm::Role::User => openai::Role::User,
            llm::Role::Assistant => openai::Role::Assistant,
        };
        let mut head: Option<Identifier> = None;
        for block in message.content {
            let wire = match block {
                llm::Block::Text { text } => {
                    let (id, phase) = match head.take() {
                        Some((id, phase)) => (Some(id), phase),
                        None => (None, None),
                    };
                    openai::Input::Message { role, text, id, phase }
                }
                llm::Block::Opaque { bytes } => {
                    if role != openai::Role::Assistant || head.is_some() {
                        return Err(Error::Malformed);
                    }
                    let value = match openai::Json::from_bytes(&bytes, &bounded) {
                        Ok(value) => value,
                        Err(error) => return Err(limits::openai_error(error)),
                    };
                    match message_head(value.as_tokens())? {
                        Some(metadata) => {
                            head = Some(metadata);
                            continue;
                        }
                        None => openai::Input::Opaque { value },
                    }
                }
                llm::Block::ToolCall { id, name, input } => {
                    if role != openai::Role::Assistant || head.is_some() {
                        return Err(Error::Malformed);
                    }
                    let (call_id, item_id) = call_id(&id)?;
                    let arguments = historical_openai_input(input, &bounded)?;
                    openai::Input::FunctionCall { call_id, item_id, name, arguments }
                }
                llm::Block::ToolResult { id, result } => {
                    if role != openai::Role::User || head.is_some() {
                        return Err(Error::Malformed);
                    }
                    let (call_id, _) = call_id(&id)?;
                    let (output, _) = render::result(&result, limits.render_bytes)?;
                    openai::Input::FunctionOutput { call_id, output }
                }
            };
            input.push(wire).or(Err(Error::TooLarge))?;
        }
        if head.is_some() {
            return Err(Error::Malformed);
        }
    }
    if session.len() > usize::try_from(limits.name_bytes).expect("u32 fits usize") {
        return Err(Error::TooLarge);
    }
    let effort = if endpoint.effort.is_empty() { None } else { Some(endpoint.effort.clone()) };
    let request = openai::Request {
        model: prompt.model,
        instructions: prompt.system,
        tools: offered.into_boxed(),
        input: input.into_boxed(),
        effort,
        prompt_cache_key: Some(bytes::copy_of(session)),
    };
    match openai::measure_request(&request, &bounded) {
        Ok(_) => Ok(request),
        Err(error) => Err(limits::openai_error(error)),
    }
}
fn admit(prompt: &llm::Prompt, endpoint: &EndpointDescriptor, limits: &Limits) -> Result<(), Error> {
    if prompt.endpoint.0 != endpoint.endpoint {
        return Err(Error::Endpoint);
    }
    if prompt.model.is_empty() || prompt.max_tokens == 0 {
        return Err(Error::Malformed);
    }
    if prompt.messages.len() > usize::try_from(limits.parts).expect("u32 fits usize") || prompt.served.len() > 2 {
        return Err(Error::TooLarge);
    }
    let mut count: usize = 0;
    let mut size = prompt.model.len().checked_add(prompt.system.len()).ok_or(Error::TooLarge)?;
    for message in &prompt.messages {
        count = count.checked_add(message.content.len()).ok_or(Error::TooLarge)?;
        for block in &message.content {
            let n = match block {
                llm::Block::Text { text } | llm::Block::Opaque { bytes: text } => text.len(),
                llm::Block::ToolCall { id, name, input } => id
                    .len()
                    .checked_add(name.len())
                    .ok_or(Error::TooLarge)?
                    .checked_add(input.len())
                    .ok_or(Error::TooLarge)?,
                llm::Block::ToolResult { id, result } => {
                    id.len().checked_add(render::length(result, limits.render_bytes)?).ok_or(Error::TooLarge)?
                }
            };
            size = size.checked_add(n).ok_or(Error::TooLarge)?;
        }
    }
    if count > usize::try_from(limits.parts).expect("u32 fits usize")
        || size > usize::try_from(limits.request_bytes).expect("u32 fits usize")
    {
        return Err(Error::TooLarge);
    }
    Ok(())
}
fn historical_anthropic_input(bytes: &[u8], limits: &anthropic::Limits) -> Result<anthropic::Json, Error> {
    match anthropic::Json::from_bytes(bytes, limits) {
        Ok(value) if value.as_tokens().first() == Some(&Token::ObjectStart) => Ok(value),
        Ok(_)
        | Err(
            anthropic::DecodeError::Malformed | anthropic::DecodeError::Missing | anthropic::DecodeError::WrongType,
        ) => match anthropic::Json::from_bytes(b"{}", limits) {
            Ok(value) => Ok(value),
            Err(error) => Err(limits::anthropic_error(error)),
        },
        Err(anthropic::DecodeError::TooLarge) => Err(Error::TooLarge),
    }
}
fn historical_openai_input(bytes: Box<[u8]>, limits: &openai::Limits) -> Result<Box<[u8]>, Error> {
    match openai::Json::from_bytes(&bytes, limits) {
        Ok(value) if value.as_tokens().first() == Some(&Token::ObjectStart) => Ok(bytes),
        Ok(_) | Err(openai::DecodeError::Malformed | openai::DecodeError::Missing | openai::DecodeError::WrongType) => {
            Ok(bytes::copy_of(b"{}"))
        }
        Err(openai::DecodeError::TooLarge) => Err(Error::TooLarge),
    }
}
pub(crate) fn call_id(id: &[u8]) -> Result<Identifier, Error> {
    if id.is_empty() {
        return Err(Error::Malformed);
    }
    let mut split = None;
    for (index, &byte) in id.iter().enumerate() {
        if byte == b'|' {
            if split.is_some() {
                return Err(Error::Malformed);
            }
            split = Some(index);
        }
    }
    match split {
        Some(at) => {
            let call = id.get(..at).ok_or(Error::Malformed)?;
            let item = id.get(at.saturating_add(1)..).ok_or(Error::Malformed)?;
            if call.is_empty() || item.is_empty() {
                return Err(Error::Malformed);
            }
            Ok((bytes::copy_of(call), Some(bytes::copy_of(item))))
        }
        None => Ok((bytes::copy_of(id), None)),
    }
}
fn message_head(tokens: &[Token]) -> Result<Option<Identifier>, Error> {
    if tokens.first() != Some(&Token::ObjectStart) {
        return Err(Error::Malformed);
    }
    let type_offset = tools::field(tokens, b"type").or(Err(Error::Malformed))?;
    if type_offset.is_some() {
        return Ok(None);
    }
    let Some(offset) = tools::field(tokens, b"id").or(Err(Error::Malformed))? else {
        return Err(Error::Malformed);
    };
    let id = match tools::value_at(tokens, offset).or(Err(Error::Malformed))? {
        [Token::String(id)] => id.clone(),
        _ => return Err(Error::Malformed),
    };
    let phase = match tools::field(tokens, b"phase").or(Err(Error::Malformed))? {
        None => None,
        Some(offset) => match tools::value_at(tokens, offset).or(Err(Error::Malformed))? {
            [Token::String(phase)] => Some(phase.clone()),
            _ => return Err(Error::Malformed),
        },
    };
    Ok(Some((id, phase)))
}

/// Holds only the offer after upload: tool authorization is fixed by the call.
#[derive(Debug)]
pub struct Completion {
    provider: Provider,
    grants: temper_legacy_agent_domain::tools::Grants,
    served: Box<[llm::Served]>,
    parts: List<llm::Said>,
}
impl Completion {
    #[must_use]
    pub fn new(
        provider: Provider,
        grants: temper_legacy_agent_domain::tools::Grants,
        served: Box<[llm::Served]>,
        limits: &Limits,
    ) -> Completion {
        Completion { provider, grants, served, parts: List::with_capacity(limits.parts) }
    }
    pub fn anthropic(&mut self, part: anthropic::Part, limits: &Limits) -> Result<(), Error> {
        let said = match part {
            anthropic::Part::Text { text } => llm::Said::Text { text },
            anthropic::Part::Opaque { bytes } => llm::Said::Opaque { bytes },
            anthropic::Part::ToolCall { id, name, input, too_large } => self.call(id, name, input, too_large, limits),
        };
        self.parts.push(said).or(Err(Error::TooLarge))
    }
    pub fn openai(&mut self, part: openai::Part, limits: &Limits) -> Result<(), Error> {
        let said = match part {
            openai::Part::Text { text } => llm::Said::Text { text },
            openai::Part::Opaque { bytes } => llm::Said::Opaque { bytes },
            openai::Part::ToolCall { id, name, input, too_large } => self.call(id, name, input, too_large, limits),
        };
        self.parts.push(said).or(Err(Error::TooLarge))
    }
    fn call(&self, id: Box<[u8]>, name: Box<[u8]>, input: Box<[u8]>, too_large: bool, limits: &Limits) -> llm::Said {
        let call = tools::decode(&self.provider, self.grants, &self.served, &name, &input, too_large, limits);
        llm::Said::ToolCall { id, name, input, call }
    }
    #[must_use]
    pub fn finish(self, stop: llm::Stop, usage: llm::Usage) -> llm::Completion {
        llm::Completion { content: self.parts.into_boxed(), stop, usage }
    }
}

#[must_use]
pub fn openai_stop(stop: openai::Stop) -> llm::Stop {
    match stop {
        openai::Stop::EndTurn => llm::Stop::EndTurn,
        openai::Stop::ToolUse => llm::Stop::ToolUse,
        openai::Stop::MaxTokens => llm::Stop::MaxTokens,
        openai::Stop::Refusal => llm::Stop::Refusal,
    }
}
#[must_use]
pub fn anthropic_stop(stop: anthropic::Stop) -> llm::Stop {
    match stop {
        anthropic::Stop::EndTurn => llm::Stop::EndTurn,
        anthropic::Stop::ToolUse => llm::Stop::ToolUse,
        anthropic::Stop::MaxTokens => llm::Stop::MaxTokens,
        anthropic::Stop::Refusal => llm::Stop::Refusal,
    }
}
#[must_use]
pub fn openai_usage(usage: openai::Usage) -> llm::Usage {
    llm::Usage {
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        cache_read_tokens: usage.cache_read_tokens,
        cache_write_tokens: usage.cache_write_tokens,
    }
}
#[must_use]
pub fn anthropic_usage(usage: anthropic::Usage) -> llm::Usage {
    llm::Usage {
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        cache_read_tokens: usage.cache_read_tokens,
        cache_write_tokens: usage.cache_write_tokens,
    }
}
