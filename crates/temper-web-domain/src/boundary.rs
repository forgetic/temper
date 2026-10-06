//! Typed shell and engine boundary (web/architecture.md, sections 3–4).
use crate::{Action, Address, Ask, ChatLine, Key, Outcome, Person, Project, Refusal, Saved};
use alloc::boxed::Box;
use skein_lib::Token;

/// Local wall-clock offset from UTC, in seconds.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Offset(pub i32);

/// Opaque position in a paged engine read.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Cursor(pub u64);

/// An input to one domain step, with a terminal for each issued operation.
#[derive(PartialEq, Eq, Debug)]
pub enum Event {
    Start { address: Address, saved: Option<Saved>, offset: Offset },
    Went { address: Address },
    Act { action: Action },
    Answered { request: Token, answer: Answer },
    Read { read: Token, result: ReadResult },
    Opened { stream: Token },
    Streamed { stream: Token, event: StreamEvent },
    Ended { stream: Token, end: StreamEnd },
}

/// An ordered request to the shell. `Save` precedes a newly minted `Send`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Request {
    Send { request: Token, key: Key, ask: Ask },
    Read { read: Token, query: Query },
    Open { stream: Token, watch: Watch },
    Close { stream: Token },
    Address { address: Address, push: bool },
    Save { saved: Saved },
    SignIn { then: Address },
}

/// One read of a bounded page.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Query {
    Chats { project: Option<u32>, live: bool, after: Option<Cursor> },
}

/// One live source, whose first event is a snapshot.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Watch {
    Person,
}

/// A terminal for a keyed send.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Answer {
    Done(Outcome),
    Refused(Refusal),
    Busy,
    SignedOut,
    Unreachable,
}

/// A terminal for a paged read.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ReadResult {
    Chats { rows: Box<[ChatLine]>, older: Option<Cursor> },
    Refused(Refusal),
    SignedOut,
    Unreachable,
}

/// The person's complete current frame.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PersonSnapshot {
    pub person: Person,
    pub projects: Box<[Project]>,
    pub inbox_count: u32,
}

/// The first event from a watch, and again after reopening.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Snapshot {
    Person(PersonSnapshot),
}

/// An incremental update from a watch.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Change {
    Person(PersonSnapshot),
}

/// An event from a watch between its open and terminal.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum StreamEvent {
    Snapshot(Snapshot),
    Change(Change),
    Missed { count: u64 },
    Alive,
}

/// The single terminal of an issued open.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StreamEnd {
    Closed,
    Dropped,
    Refused(Refusal),
    SignedOut,
    Gone,
}
