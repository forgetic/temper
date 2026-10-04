//! Bounded Forgejo v15 documents, independent of temper's domains.
#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]
extern crate alloc;

pub mod binary;
pub mod json;
pub mod request;
pub mod response;
pub mod time;
pub mod types;
pub mod webhook;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Error {
    Malformed,
    Missing,
    Duplicate,
    TooLarge,
    Text,
    Unsupported,
}

/// These caps bound retained documents. Long-history execution awaits JSON
/// skip/string-piece support; raising these is not a streaming substitute.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Limits {
    pub depth: u32,
    pub tokens: u32,
    pub page: u32,
    pub fields: u32,
    pub name_bytes: u32,
    pub title_bytes: u32,
    pub body_bytes: u32,
    pub marker_bytes: u32,
    pub document_bytes: u32,
    pub hook_bytes: u32,
}
impl Limits {
    pub const STARTING: Limits = Limits {
        depth: 16,
        tokens: 16_384,
        page: 50,
        fields: 128,
        name_bytes: 1024,
        title_bytes: 4096,
        body_bytes: 262_144,
        marker_bytes: 1024,
        document_bytes: 1_048_576,
        hook_bytes: 1_048_576,
    };
    #[must_use]
    pub const fn valid(&self) -> bool {
        self.depth >= 8
            && self.tokens > 0
            && self.page > 0
            && self.fields > 0
            && self.name_bytes > 0
            && self.title_bytes > 0
            && self.body_bytes > 0
            && self.marker_bytes <= self.body_bytes
            && self.document_bytes > 0
            && self.hook_bytes > 0
    }
}

/// Convenience finite document paths, including simultaneous JSON tokens,
/// decoded page storage, sized output, and base64's cap+2 scratch plus result.
/// Live histories must use the token decoder once upstream skip/string pieces
/// exist; this bound does not authorize buffering an arbitrary history.
#[must_use]
pub fn worst_case(limits: &Limits) -> Option<u64> {
    if !limits.valid() {
        return None;
    }
    json::worst_case(limits)?
        .checked_add(response::worst_case(limits)?)?
        .checked_add(webhook::worst_case(limits)?)?
        .checked_add(u64::from(limits.document_bytes).checked_mul(3)?.checked_add(2)?)
}
