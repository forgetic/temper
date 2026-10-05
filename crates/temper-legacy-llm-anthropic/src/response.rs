use crate::{
    Block, DecodeError, Failure, Json, Limits, ProviderError, RateLimit, Stop, Usage, classify, common, json, request,
};
use alloc::boxed::Box;
use core::mem;
use skein_json::{Token, writer::Encoder};
use skein_lib::{List, Queue, Wall, bytes};

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Delta {
    Text { text: Box<[u8]> },
    Thinking { text: Box<[u8]> },
    Signature { text: Box<[u8]> },
    Input { text: Box<[u8]> },
}
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Event {
    MessageStart { usage: Usage },
    BlockStart { index: u32, block: Block },
    BlockDelta { index: u32, delta: Delta },
    BlockStop { index: u32 },
    MessageDelta { stop: Stop, usage: Usage },
    MessageStop,
    Ping,
    Error { error: ProviderError },
    Unknown,
}
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Part {
    Text { text: Box<[u8]> },
    Opaque { bytes: Box<[u8]> },
    ToolCall { id: Box<[u8]>, name: Box<[u8]>, input: Box<[u8]>, too_large: bool },
}
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Output {
    Part(Part),
    Completed { stop: Stop, usage: Usage },
    Failed { failure: Failure, detail: Box<[u8]> },
    Progress,
}
/// A block plus a terminal when a configured answer limit cuts the stream.
pub const MAX_OUT: u32 = 2;
#[derive(Debug)]
pub struct StreamDecoder {
    phase: Phase,
    next: u32,
    open: Open,
    usage: Usage,
    stop: Stop,
    tools: bool,
    bytes: u64,
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Phase {
    Before,
    Blocks,
    Ending,
    Over,
}
#[derive(Debug)]
enum Open {
    None,
    Text { text: List<u8> },
    Tool { id: Box<[u8]>, name: Box<[u8]>, input: List<u8>, too_large: bool },
    Thinking { head: Json, text: List<u8>, signature: List<u8> },
    Opaque { value: Json },
}
impl StreamDecoder {
    #[must_use]
    pub const fn new(_limits: &Limits) -> StreamDecoder {
        StreamDecoder {
            phase: Phase::Before,
            next: 0,
            open: Open::None,
            usage: Usage::ZERO,
            stop: Stop::EndTurn,
            tools: false,
            bytes: 0,
        }
    }
    pub fn event(&mut self, event: Event, limits: &Limits, wall: Wall, out: &mut Queue<Output>) {
        if self.phase == Phase::Over {
            return;
        }
        let result = self.accept(event, limits, wall, out);
        match result {
            Ok(()) => {}
            Err(DecodeError::TooLarge) => self.truncate(limits, out),
            Err(DecodeError::Malformed | DecodeError::Missing | DecodeError::WrongType) => {
                self.fail(Failure::Unavailable, bytes::copy_of(b"malformed Anthropic stream"), out);
            }
        }
    }
    pub fn end(&mut self, out: &mut Queue<Output>) {
        if self.phase != Phase::Over {
            self.fail(Failure::Unavailable, bytes::copy_of(b"incomplete Anthropic stream"), out);
        }
    }
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.phase == Phase::Over
    }
    fn accept(
        &mut self,
        event: Event,
        limits: &Limits,
        wall: Wall,
        out: &mut Queue<Output>,
    ) -> Result<(), DecodeError> {
        match event {
            Event::MessageStart { usage } => {
                if self.phase != Phase::Before {
                    return Err(DecodeError::Malformed);
                }
                self.phase = Phase::Blocks;
                self.usage = usage;
                out.push(Output::Progress);
            }
            Event::BlockStart { index, block } => {
                if self.phase != Phase::Blocks || index != self.next || !self.empty() {
                    return Err(DecodeError::Malformed);
                }
                if self.next >= limits.parts {
                    return Err(DecodeError::TooLarge);
                }
                self.open = open_block(block, self.remaining(limits), limits)?;
                out.push(Output::Progress);
            }
            Event::BlockDelta { index, delta } => {
                if self.phase != Phase::Blocks || index != self.next {
                    return Err(DecodeError::Malformed);
                }
                append_delta(&mut self.open, delta, limits)?;
                out.push(Output::Progress);
            }
            Event::BlockStop { index } => {
                if index != self.next || self.phase != Phase::Blocks || self.empty() {
                    return Err(DecodeError::Malformed);
                }
                let open = mem::replace(&mut self.open, Open::None);
                let part = close(open, limits)?;
                self.emit(part, limits, out)?;
                self.next = self.next.checked_add(1).ok_or(DecodeError::TooLarge)?;
            }
            Event::MessageDelta { stop, usage } => {
                if self.phase != Phase::Blocks || !self.empty() {
                    return Err(DecodeError::Malformed);
                }
                self.phase = Phase::Ending;
                self.stop = stop;
                self.usage = Usage {
                    input_tokens: self.usage.input_tokens.max(usage.input_tokens),
                    output_tokens: self.usage.output_tokens.max(usage.output_tokens),
                    cache_read_tokens: self.usage.cache_read_tokens.max(usage.cache_read_tokens),
                    cache_write_tokens: self.usage.cache_write_tokens.max(usage.cache_write_tokens),
                };
                out.push(Output::Progress);
            }
            Event::MessageStop => {
                if self.phase != Phase::Ending || !self.empty() {
                    return Err(DecodeError::Malformed);
                }
                self.phase = Phase::Over;
                out.push(Output::Completed {
                    stop: if self.tools { Stop::ToolUse } else { self.stop },
                    usage: self.usage,
                });
            }
            Event::Ping | Event::Unknown => out.push(Output::Progress),
            Event::Error { error } => {
                let failure = classify(0, Some(&error), RateLimit::NONE, wall);
                self.fail(failure, common::clipped(&error.message, limits.detail_bytes), out);
            }
        }
        Ok(())
    }
    fn empty(&self) -> bool {
        match self.open {
            Open::None => true,
            Open::Text { .. } | Open::Tool { .. } | Open::Thinking { .. } | Open::Opaque { .. } => false,
        }
    }
    fn remaining(&self, limits: &Limits) -> u32 {
        limits.answer_bytes.saturating_sub(u32::try_from(self.bytes).unwrap_or(u32::MAX))
    }
    fn emit(&mut self, part: Part, limits: &Limits, out: &mut Queue<Output>) -> Result<(), DecodeError> {
        let size = match &part {
            Part::Text { text } => text.len(),
            Part::Opaque { bytes } => bytes.len(),
            Part::ToolCall { id, name, input, .. } => {
                self.tools = true;
                id.len().saturating_add(name.len()).saturating_add(input.len())
            }
        };
        let total =
            self.bytes.checked_add(u64::try_from(size).expect("usize fits u64")).ok_or(DecodeError::TooLarge)?;
        if total > u64::from(limits.answer_bytes) {
            return Err(DecodeError::TooLarge);
        }
        self.bytes = total;
        out.push(Output::Part(part));
        Ok(())
    }
    fn truncate(&mut self, limits: &Limits, out: &mut Queue<Output>) {
        let open = mem::replace(&mut self.open, Open::None);
        match open {
            Open::Tool { id, name, input, too_large } => {
                let part = Part::ToolCall { id, name, input: input.into_boxed(), too_large };
                if self.emit(part, limits, out).is_err() {}
            }
            Open::None | Open::Text { .. } | Open::Thinking { .. } | Open::Opaque { .. } => {}
        }
        self.phase = Phase::Over;
        out.push(Output::Completed { stop: Stop::MaxTokens, usage: self.usage });
    }
    fn fail(&mut self, failure: Failure, detail: Box<[u8]>, out: &mut Queue<Output>) {
        self.open = Open::None;
        self.phase = Phase::Over;
        out.push(Output::Failed { failure, detail });
    }
}
fn close(open: Open, limits: &Limits) -> Result<Part, DecodeError> {
    match open {
        Open::Text { text } => Ok(Part::Text { text: text.into_boxed() }),
        Open::Tool { id, name, input, too_large } => Ok(Part::ToolCall {
            id,
            name,
            input: if input.is_empty() && !too_large { bytes::copy_of(b"{}") } else { input.into_boxed() },
            too_large,
        }),
        Open::Opaque { value } => Ok(Part::Opaque { bytes: value.to_bytes(limits)? }),
        Open::Thinking { head, text, signature } => {
            let bounded = skein_json::writer::Limits {
                depth: limits.depth,
                length: limits.opaque_bytes.min(limits.document_bytes),
            };
            let mut measure = Encoder::measure(&bounded);
            write_thinking(&mut measure, &head, text.as_slice(), signature.as_slice());
            let len = common::measured(measure)?;
            let mut write = Encoder::write(len, &bounded);
            write_thinking(&mut write, &head, text.as_slice(), signature.as_slice());
            Ok(Part::Opaque { bytes: write.finish() })
        }
        Open::None => Err(DecodeError::Malformed),
    }
}
fn write_thinking(out: &mut Encoder, head: &Json, text: &[u8], signature: &[u8]) {
    out.object_start();
    let tokens = head.as_tokens();
    let mut skip: usize = 1;
    for (at, token) in tokens.iter().enumerate() {
        if at < skip {
            continue;
        }
        match token {
            Token::Key(key) => {
                let start = at.checked_add(1).expect("within bounded tokens");
                let end = json::span(tokens, start).expect("validated JSON value");
                skip = end;
                if key.as_ref() != b"thinking" && key.as_ref() != b"signature" {
                    out.key(key);
                    for token in tokens.get(start..end).expect("the value is within tokens") {
                        out.token(token);
                    }
                }
            }
            Token::ObjectEnd => break,
            Token::ObjectStart
            | Token::ArrayStart
            | Token::ArrayEnd
            | Token::String(_)
            | Token::Number(_)
            | Token::True
            | Token::False
            | Token::Null => unreachable!("all value tokens were skipped"),
        }
    }
    out.key(b"thinking");
    out.string(text);
    out.key(b"signature");
    out.string(signature);
    out.object_end();
}

pub fn decode_event(value: &Json, limits: &Limits) -> Result<Event, DecodeError> {
    let tokens = value.as_tokens();
    let kind = json::text_ref(json::value_at(tokens, json::required(tokens, b"type")?)?)?;
    match kind {
        b"message_start" => Ok(Event::MessageStart {
            usage: read_usage(json::value_at(
                json::value_at(tokens, json::required(tokens, b"message")?)?,
                json::required(json::value_at(tokens, json::required(tokens, b"message")?)?, b"usage")?,
            )?)?,
        }),
        b"content_block_start" => Ok(Event::BlockStart {
            index: read_index(tokens)?,
            block: request::read_block(json::value_at(tokens, json::required(tokens, b"content_block")?)?, limits)?,
        }),
        b"content_block_delta" => {
            let delta = json::value_at(tokens, json::required(tokens, b"delta")?)?;
            let kind = json::text_ref(json::value_at(delta, json::required(delta, b"type")?)?)?;
            let delta = match kind {
                b"text_delta" => {
                    Delta::Text { text: json::text(json::value_at(delta, json::required(delta, b"text")?)?)? }
                }
                b"thinking_delta" => {
                    Delta::Thinking { text: json::text(json::value_at(delta, json::required(delta, b"thinking")?)?)? }
                }
                b"signature_delta" => {
                    Delta::Signature { text: json::text(json::value_at(delta, json::required(delta, b"signature")?)?)? }
                }
                b"input_json_delta" => {
                    Delta::Input { text: json::text(json::value_at(delta, json::required(delta, b"partial_json")?)?)? }
                }
                _ => return Err(DecodeError::WrongType),
            };
            Ok(Event::BlockDelta { index: read_index(tokens)?, delta })
        }
        b"content_block_stop" => Ok(Event::BlockStop { index: read_index(tokens)? }),
        b"message_delta" => Ok(Event::MessageDelta {
            stop: read_stop(json::text_ref(json::value_at(
                json::value_at(tokens, json::required(tokens, b"delta")?)?,
                json::required(json::value_at(tokens, json::required(tokens, b"delta")?)?, b"stop_reason")?,
            )?)?)?,
            usage: read_usage(json::value_at(tokens, json::required(tokens, b"usage")?)?)?,
        }),
        b"message_stop" => Ok(Event::MessageStop),
        b"ping" => Ok(Event::Ping),
        b"error" => Ok(Event::Error { error: decode_error(value, limits)? }),
        _ => Ok(Event::Unknown),
    }
}
fn read_index(tokens: &[Token]) -> Result<u32, DecodeError> {
    match u32::try_from(json::unsigned(json::value_at(tokens, json::required(tokens, b"index")?)?)?) {
        Ok(n) => Ok(n),
        Err(_) => Err(DecodeError::TooLarge),
    }
}
fn read_stop(text: &[u8]) -> Result<Stop, DecodeError> {
    match text {
        b"end_turn" | b"stop_sequence" => Ok(Stop::EndTurn),
        b"tool_use" => Ok(Stop::ToolUse),
        b"max_tokens" | b"model_context_window_exceeded" => Ok(Stop::MaxTokens),
        b"refusal" => Ok(Stop::Refusal),
        _ => Err(DecodeError::WrongType),
    }
}
fn count(tokens: &[Token], name: &[u8]) -> Result<u64, DecodeError> {
    match json::optional_at(tokens, json::field(tokens, name)?)? {
        Some(value) => json::unsigned(value),
        None => Ok(0),
    }
}
fn read_usage(tokens: &[Token]) -> Result<Usage, DecodeError> {
    Ok(Usage {
        input_tokens: count(tokens, b"input_tokens")?,
        output_tokens: count(tokens, b"output_tokens")?,
        cache_read_tokens: count(tokens, b"cache_read_input_tokens")?,
        cache_write_tokens: count(tokens, b"cache_creation_input_tokens")?,
    })
}
fn write_usage(out: &mut Encoder, usage: Usage) {
    out.object_start();
    out.key(b"input_tokens");
    out.unsigned(usage.input_tokens);
    out.key(b"output_tokens");
    out.unsigned(usage.output_tokens);
    out.key(b"cache_read_input_tokens");
    out.unsigned(usage.cache_read_tokens);
    out.key(b"cache_creation_input_tokens");
    out.unsigned(usage.cache_write_tokens);
    out.object_end();
}
pub fn decode_error(value: &Json, limits: &Limits) -> Result<ProviderError, DecodeError> {
    let tokens = json::value_at(value.as_tokens(), json::required(value.as_tokens(), b"error")?)?;
    Ok(ProviderError {
        kind: json::text(json::value_at(tokens, json::required(tokens, b"type")?)?)?,
        message: common::clipped(
            json::text_ref(json::value_at(tokens, json::required(tokens, b"message")?)?)?,
            limits.detail_bytes,
        ),
        resets_in_seconds: None,
        resets_at: None,
    })
}
pub fn encode_error(error: &ProviderError, limits: &Limits) -> Result<Box<[u8]>, DecodeError> {
    encode_event(&Event::Error { error: error.clone() }, limits)
}
pub fn encode_event(event: &Event, limits: &Limits) -> Result<Box<[u8]>, DecodeError> {
    let bounded = limits.writer_limits();
    let mut measure = Encoder::measure(&bounded);
    write_event(&mut measure, event);
    let len = common::measured(measure)?;
    let mut write = Encoder::write(len, &bounded);
    write_event(&mut write, event);
    Ok(write.finish())
}
fn write_event(out: &mut Encoder, event: &Event) {
    out.object_start();
    out.key(b"type");
    match event {
        Event::MessageStart { usage } => {
            out.string(b"message_start");
            out.key(b"message");
            out.object_start();
            out.key(b"usage");
            write_usage(out, *usage);
            out.object_end();
        }
        Event::BlockStart { index, block } => {
            out.string(b"content_block_start");
            out.key(b"index");
            out.unsigned(u64::from(*index));
            out.key(b"content_block");
            request::write_block(out, block, false);
        }
        Event::BlockDelta { index, delta } => {
            out.string(b"content_block_delta");
            out.key(b"index");
            out.unsigned(u64::from(*index));
            out.key(b"delta");
            out.object_start();
            out.key(b"type");
            match delta {
                Delta::Text { text } => {
                    out.string(b"text_delta");
                    out.key(b"text");
                    out.string(text);
                }
                Delta::Thinking { text } => {
                    out.string(b"thinking_delta");
                    out.key(b"thinking");
                    out.string(text);
                }
                Delta::Signature { text } => {
                    out.string(b"signature_delta");
                    out.key(b"signature");
                    out.string(text);
                }
                Delta::Input { text } => {
                    out.string(b"input_json_delta");
                    out.key(b"partial_json");
                    out.string(text);
                }
            }
            out.object_end();
        }
        Event::BlockStop { index } => {
            out.string(b"content_block_stop");
            out.key(b"index");
            out.unsigned(u64::from(*index));
        }
        Event::MessageDelta { stop, usage } => {
            out.string(b"message_delta");
            out.key(b"delta");
            out.object_start();
            out.key(b"stop_reason");
            out.string(match stop {
                Stop::EndTurn => b"end_turn",
                Stop::ToolUse => b"tool_use",
                Stop::MaxTokens => b"max_tokens",
                Stop::Refusal => b"refusal",
            });
            out.object_end();
            out.key(b"usage");
            write_usage(out, *usage);
        }
        Event::MessageStop => out.string(b"message_stop"),
        Event::Ping => out.string(b"ping"),
        Event::Unknown => out.string(b"future_event"),
        Event::Error { error } => {
            out.string(b"error");
            out.key(b"error");
            out.object_start();
            out.key(b"type");
            out.string(&error.kind);
            out.key(b"message");
            out.string(&error.message);
            out.object_end();
        }
    }
    out.object_end();
}

fn open_block(block: Block, remaining: u32, limits: &Limits) -> Result<Open, DecodeError> {
    match block {
        Block::Text { text } => {
            let mut collected = List::with_capacity(remaining);
            common::append(&mut collected, &text)?;
            Ok(Open::Text { text: collected })
        }
        Block::ToolUse { id, name, input } => {
            if input.as_tokens().first() != Some(&Token::ObjectStart) {
                return Err(DecodeError::WrongType);
            }
            let empty = input.as_tokens() == [Token::ObjectStart, Token::ObjectEnd];
            let identifiers = u32::try_from(id.len().saturating_add(name.len())).unwrap_or(u32::MAX);
            let available = remaining.checked_sub(identifiers).ok_or(DecodeError::TooLarge)?;
            let mut collected = List::with_capacity(limits.input_bytes.min(available));
            if !empty {
                common::append(&mut collected, &input.to_bytes(limits)?)?;
            }
            Ok(Open::Tool { id, name, input: collected, too_large: false })
        }
        Block::Opaque { value } => {
            if json::text_ref(json::value_at(value.as_tokens(), json::required(value.as_tokens(), b"type")?)?)?
                == b"thinking"
            {
                let mut text = List::with_capacity(limits.opaque_bytes.min(remaining));
                let mut signature = List::with_capacity(limits.opaque_bytes.min(remaining));
                if let Some(value) = json::optional_at(value.as_tokens(), json::field(value.as_tokens(), b"thinking")?)?
                {
                    common::append(&mut text, json::text_ref(value)?)?;
                }
                if let Some(value) =
                    json::optional_at(value.as_tokens(), json::field(value.as_tokens(), b"signature")?)?
                {
                    common::append(&mut signature, json::text_ref(value)?)?;
                }
                Ok(Open::Thinking { head: value, text, signature })
            } else {
                if value.to_bytes(limits)?.len() > usize::try_from(limits.opaque_bytes).expect("u32 fits usize") {
                    return Err(DecodeError::TooLarge);
                }
                Ok(Open::Opaque { value })
            }
        }
        Block::ToolResult { .. } => Err(DecodeError::WrongType),
    }
}

fn append_delta(open: &mut Open, delta: Delta, limits: &Limits) -> Result<(), DecodeError> {
    match open {
        Open::Text { text } => match delta {
            Delta::Text { text: fragment } => common::append(text, &fragment)?,
            Delta::Thinking { .. } | Delta::Signature { .. } | Delta::Input { .. } => {
                return Err(DecodeError::Malformed);
            }
        },
        Open::Tool { input, too_large, .. } => match delta {
            Delta::Input { text } => {
                if !*too_large && common::append(input, &text).is_err() {
                    if input.capacity() < limits.input_bytes {
                        let count = usize::try_from(input.room()).expect("u32 fits usize");
                        common::append(input, text.get(..count).ok_or(DecodeError::Malformed)?)?;
                        return Err(DecodeError::TooLarge);
                    }
                    *input = List::with_capacity(0);
                    *too_large = true;
                }
            }
            Delta::Text { .. } | Delta::Thinking { .. } | Delta::Signature { .. } => {
                return Err(DecodeError::Malformed);
            }
        },
        Open::Thinking { text, signature, .. } => match delta {
            Delta::Thinking { text: fragment } => common::append(text, &fragment)?,
            Delta::Signature { text } => common::append(signature, &text)?,
            Delta::Text { .. } | Delta::Input { .. } => return Err(DecodeError::Malformed),
        },
        Open::None | Open::Opaque { .. } => return Err(DecodeError::Malformed),
    }

    Ok(())
}

pub(crate) fn decoder_worst_case(limits: &Limits) -> Option<u64> {
    // A thinking head plus the two capped text/signature buffers; other open
    // blocks hold at most one capped text/input buffer and bounded identifiers.
    u64::from(limits.answer_bytes).checked_mul(2)?.checked_add(u64::from(limits.document_bytes).checked_mul(2)?)
}
