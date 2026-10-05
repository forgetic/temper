//! Admission bounds for translation and connection state (llm.md, 10).
use skein_lib::Duration;
use temper_legacy_llm_anthropic as anthropic;
use temper_legacy_llm_openai as openai;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Limits {
    pub calls: u32,
    pub endpoints: u32,
    pub accounts: u32,
    pub token_bytes: u32,
    pub name_bytes: u32,
    pub request_bytes: u32,
    pub head_bytes: u32,
    pub headers: u32,
    pub event_bytes: u32,
    pub string_bytes: u32,
    pub answer_bytes: u32,
    pub parts: u32,
    pub input_bytes: u32,
    pub opaque_bytes: u32,
    pub error_bytes: u32,
    pub detail_bytes: u32,
    pub render_bytes: u32,
    pub depth: u32,
    pub tokens: u32,
    pub chunk: u32,
    pub connect: Duration,
    pub handshake: Duration,
    pub head: Duration,
    pub idle: Duration,
    pub keep_idle: Duration,
    pub skew: Duration,
}

impl Limits {
    #[must_use]
    pub fn openai(&self) -> openai::Limits {
        openai::Limits {
            request_bytes: self.request_bytes,
            document_bytes: self.event_bytes.max(self.request_bytes),
            string_bytes: self.string_bytes,
            depth: self.depth,
            tokens: self.tokens,
            parts: self.parts,
            input_bytes: self.input_bytes,
            opaque_bytes: self.opaque_bytes,
            answer_bytes: self.answer_bytes,
            detail_bytes: self.detail_bytes,
        }
    }
    #[must_use]
    pub fn anthropic(&self) -> anthropic::Limits {
        anthropic::Limits {
            request_bytes: self.request_bytes,
            document_bytes: self.event_bytes.max(self.request_bytes),
            string_bytes: self.string_bytes,
            depth: self.depth,
            tokens: self.tokens,
            parts: self.parts,
            input_bytes: self.input_bytes,
            opaque_bytes: self.opaque_bytes,
            answer_bytes: self.answer_bytes,
            detail_bytes: self.detail_bytes,
        }
    }
    #[must_use]
    pub fn tool_json(&self) -> openai::Limits {
        let mut limits = self.openai();
        limits.document_bytes = self.input_bytes;
        limits.string_bytes = self.input_bytes;
        limits
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Error {
    Malformed,
    TooLarge,
    Endpoint,
    Grant,
    Unsupported,
}

pub(crate) fn openai_error(error: openai::DecodeError) -> Error {
    match error {
        openai::DecodeError::TooLarge => Error::TooLarge,
        openai::DecodeError::Malformed | openai::DecodeError::Missing | openai::DecodeError::WrongType => {
            Error::Malformed
        }
    }
}
pub(crate) fn anthropic_error(error: anthropic::DecodeError) -> Error {
    match error {
        anthropic::DecodeError::TooLarge => Error::TooLarge,
        anthropic::DecodeError::Malformed | anthropic::DecodeError::Missing | anthropic::DecodeError::WrongType => {
            Error::Malformed
        }
    }
}
