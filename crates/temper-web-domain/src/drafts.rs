//! The browser's drafts, written back only when the domain changes them.
use crate::{Intent, Object};
use alloc::boxed::Box;
use skein_lib::Id;

/// Input text with a count of domain-origin writes.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Field {
    pub text: Box<[u8]>,
    pub written: u32,
}

impl Field {
    pub(crate) fn empty() -> Field {
        Field { text: Box::from([]), written: 0 }
    }
}

/// Why an open confirmation cannot send yet.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Problem {
    ReasonMissing,
    Changed,
    NotOffered,
}

/// One card action being confirmed against the revision first shown.
#[derive(Debug)]
pub struct Confirming {
    pub intent: Intent,
    pub object: Id<Object>,
    pub revision: u64,
    pub changed: bool,
    pub problem: Option<Problem>,
}
