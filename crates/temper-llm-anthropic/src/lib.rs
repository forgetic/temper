//! Anthropic Messages documents, both sides, and an ordered answer decoder.
#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]
extern crate alloc;
mod common;
pub mod identity;
mod json;
mod request;
mod response;
pub use common::{DecodeError, Failure, Limits, ProviderError, RateLimit, Stop, Usage, classify, worst_case};
pub use json::{Collector, Json};
pub use request::{Block, Message, Request, Role, Tool, decode_request, encode_request, measure_request};
pub use response::{
    Delta, Event, MAX_OUT, Output, Part, StreamDecoder, decode_error, decode_event, encode_error, encode_event,
};

#[cfg(test)]
mod tests;
