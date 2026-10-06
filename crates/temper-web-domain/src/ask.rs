//! Keyed asks and durable answers in the client's vocabulary.
use crate::Person;
use alloc::boxed::Box;
use skein_lib::Wall;

/// Drawn from the domain seed and scoped by the engine to a person.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Key(pub [u8; 16]);

/// One durable operation, resent unchanged under its key.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Ask {
    StartChat { project: u32, words: Box<[u8]> },
    Decide { waiting: Waiting, revision: u64, decision: Decision },
}

/// The held object a person decides.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Waiting {
    Escalation { task: u64 },
}

/// A request to decide a held task.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Decision {
    Release,
    Reject { reason: Box<[u8]> },
    Pass,
}

/// The committed kind of decision.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Choice {
    Released,
    Rejected,
    Passed,
}

/// A committed successful result.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Outcome {
    Started { task: u64 },
    Decided { choice: Choice },
    DecidedBefore { by: Person, choice: Choice, at: Wall },
}

/// Why the person's authority did not cover a decision.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lack {
    Role,
    Funding,
    Scope,
}

/// A committed refusal.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Refusal {
    Role,
    Limit,
    Ended,
    Unknown,
    KeyConflict,
    Authority { lacks: Lack, proposable: bool },
    Moved { revision: u64 },
    Standing,
    NoFurther,
    NeedsAmend,
}
