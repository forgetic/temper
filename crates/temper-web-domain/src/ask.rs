//! Keyed asks and durable answers in the client's vocabulary.
use alloc::boxed::Box;

/// Drawn from the domain seed and scoped by the engine to a person.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Key(pub [u8; 16]);

/// One durable operation, resent unchanged under its key.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Ask {
    StartChat { project: u32, words: Box<[u8]> },
}

/// A committed successful result.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Outcome {
    Started { task: u64 },
}

/// A committed refusal.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Refusal {
    Role,
    Limit,
    Ended,
    Unknown,
    KeyConflict,
}
