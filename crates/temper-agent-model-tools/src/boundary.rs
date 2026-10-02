//! The records that cross the boundary with the tools' parent, the session
//! sub-model (4.5), which hands the tools its LLM's calls and routes the file
//! operations they ask for out to io and the terminal events back. The tools
//! define them; the session depends on the tools.
//!
//! Three shapes cross it:
//!
//! - A kit opens and closes: an [`Event::Open`] is answered by exactly one
//!   [`Request::Opened`] or [`Request::Refused`], and an [`Event::Close`] by
//!   exactly one [`Request::Closed`], once every call the kit was running has
//!   been answered. The session names its kit by the token `Opened` gives it,
//!   and the tools echo the session's own token.
//! - Calls in, answers out: each [`Event::Call`] is answered by exactly one
//!   [`Request::Answer`], which consumes its `ReplyTo`.
//! - Operations out, one terminal event in: each [`Request::Io`] is ended by
//!   exactly one [`Event::Done`], after a [`Request::CancelIo`] too. Its
//!   `owner` is the tools' token, echoed on the terminal.
//!
//! The tools arm no timers. A call comes with its deadline, which bounds every
//! operation it asks for; io runs the race (5.3) and reports a lost one as
//! [`Done::TimedOut`].

use alloc::boxed::Box;

use temper_lib::{ReplyTo, Time, Token};

use crate::authority::Authority;
use crate::call::{Call, Entry, Fault, Outcome};
use crate::path::Place;

/// session -> tools
#[derive(PartialEq, Eq, Debug)]
pub enum Event {
    /// Open a kit for the session `session`, with `authority`.
    Open { session: Token, authority: Authority },
    /// Run `call` with the kit `kit`, and answer it by `deadline`.
    Call { kit: Token, reply_to: ReplyTo, call: Call, deadline: Time },
    /// Close the kit `kit`: cancel what it is running and, once every call
    /// has been answered, end it. No call follows.
    Close { kit: Token },
    /// Terminal for `Io`.
    Done { owner: Token, done: Done },
}

/// tools -> session
#[derive(PartialEq, Eq, Debug)]
pub enum Request {
    /// The answer to an `Open`: the kit is `kit` from now on.
    Opened { session: Token, kit: Token },
    /// The answer to an `Open`: no kit was opened.
    Refused { session: Token, refusal: Refusal },
    /// The answer to a `Call`: exactly one per call.
    Answer { to: ReplyTo, outcome: Outcome },
    /// The answer to a `Close`: the kit has ended.
    Closed { session: Token },
    /// Ask io for `op`, giving up at `deadline`.
    Io { owner: Token, op: Op, deadline: Time },
    /// Abandon the `Io` in flight for `owner`. Its terminal event still comes:
    /// `Cancelled`, or whichever outcome won the race.
    CancelIo { owner: Token },
}

/// Why a kit was not opened.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Refusal {
    /// Every kit slot is taken.
    Busy,
    /// The authority does not fit the limits.
    Invalid,
}

/// A file operation, asked of io. io resolves every place beneath its root
/// and refuses one that a symbolic link leads out of (`Escapes`).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Op {
    /// Read the regular file at `at`, following symbolic links, if it holds
    /// at most `max` bytes. Ends in `Loaded`, `Missing`, `NotFile`,
    /// `NotDirectory`, `TooLarge` or a common terminal.
    Load { at: Place, max: u32 },
    /// List the directory at `at`, following symbolic links: at most `max`
    /// entries, the first in name order, without `.` and `..`. Ends in
    /// `Scanned`, `Missing`, `NotDirectory` or a common terminal.
    Scan { at: Place, max: u32 },
    /// Make the regular file at `at` hold `content`, if it is as `expect`
    /// says: written beside it and renamed into place, so that it is replaced
    /// whole or not at all, after creating the missing directories on the
    /// way. A symbolic link at `at` is not written through. Ends in `Stored`,
    /// `Conflict`, `NotFile`, `NotDirectory` or a common terminal.
    Store { at: Place, content: Box<[u8]>, expect: Expect },
}

/// What a `Store` expects to replace.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Expect {
    /// Nothing: the file is created.
    Absent,
    /// The file at `version`.
    Is { version: Version },
}

/// io's terminal for an operation. Each operation ends in those its
/// documentation names, or in one of the common terminals: `Escapes`,
/// `Failed`, `TimedOut` and `Cancelled`.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Done {
    /// Load: the file holds `content`, at `version`.
    Loaded { content: Box<[u8]>, version: Version },
    /// Scan: the directory's first `entries` in name order, and how many
    /// `more` it has.
    Scanned { entries: Box<[Entry]>, more: u64 },
    /// Store: the file holds the content, at `version`.
    Stored { version: Version },
    /// Store: the file is not as expected. It is at `now`, or absent.
    Conflict { now: Option<Version> },
    /// Load, Scan: nothing is at the place.
    Missing,
    /// Load, Store: what is at the place is not a regular file (for Store,
    /// a symbolic link is not one either).
    NotFile,
    /// Scan: what is at the place is not a directory. Any operation: a
    /// directory on the way to the place is not one.
    NotDirectory,
    /// Load: the file is `size` bytes, more than asked for.
    TooLarge { size: u64 },
    /// A symbolic link on the way leads out of the root.
    Escapes,
    /// io failed with `fault`.
    Failed { fault: Fault },
    /// The deadline passed first.
    TimedOut,
    /// The cancel won the race.
    Cancelled,
}

/// Which version of a file io found, or made by storing it. io makes versions
/// so that a file changed in any way has a new one, and the model compares
/// them and never looks inside. In production a version is the file's device,
/// inode, change time and size; every store renames a new file into place, so
/// its inode changes.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Version([u64; 4]);

impl Version {
    /// Made below the model, from what io found.
    #[must_use]
    pub const fn new(raw: [u64; 4]) -> Version {
        Version(raw)
    }

    /// What it was made from, for the layer that made it.
    #[must_use]
    pub const fn raw(self) -> [u64; 4] {
        self.0
    }
}
