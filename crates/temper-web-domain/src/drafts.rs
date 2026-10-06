//! The browser's drafts, written back only when the domain changes them.
use alloc::boxed::Box;

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
