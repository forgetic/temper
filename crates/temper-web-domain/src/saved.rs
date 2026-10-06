//! Typed session-storage state, applied whole by the shell.
use crate::{Ask, FieldRef, Key};
use alloc::boxed::Box;

/// A draft saved across a reload.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SavedDraft {
    pub field: FieldRef,
    pub text: Box<[u8]>,
    /// Task number for an unsent held-task reason; absent for other drafts.
    pub target: Option<u64>,
}

/// A keyed ask that must be resent until its durable answer.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SavedPending {
    pub key: Key,
    pub ask: Ask,
}

/// Everything kept in session storage, replaced as a whole.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Saved {
    pub project: Option<u32>,
    pub drafts: Box<[SavedDraft]>,
    pub pending: Box<[SavedPending]>,
}
