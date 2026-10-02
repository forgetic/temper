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

use crate::authority::{Authority, Var};
use crate::call::{Call, Entry, Exit, Fault, Hit, Outcome};
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
/// and refuses one that a symbolic link leads out of (`Escapes`). A store
/// follows no link at all.
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
    /// says. No part of `at` may be a symbolic link (`Linked`): io resolves
    /// it beneath the root following none (`RESOLVE_NO_SYMLINKS`), so a store
    /// lands in the repository its place names and in no other mounted
    /// beneath it. The content is written beside the file and renamed into
    /// place, so that the file is replaced whole or not at all, after
    /// creating the missing directories on the way.
    ///
    /// io compares the file's version with `expect` just before the rename. A
    /// change made before that is caught; one made by a writer outside the
    /// agent between the comparison and the rename is not, for the two are
    /// not one atomic step. (io runs one store at a time per root, so the
    /// agent's own kits do not race each other there.)
    ///
    /// Ends in `Stored`, `Conflict`, `Linked`, `NotFile`, `NotDirectory` or a
    /// common terminal.
    Store { at: Place, content: Box<[u8]>, expect: Expect },
    /// Run `command` with the shell, in the directory at `cwd`, as a process
    /// tree contained by io. It sees the repositories in `roots`, and may
    /// write only those marked writable: io mounts the rest read-only, a
    /// repository mounted inside another over it, so that what a process may
    /// write beneath a root is decided by the deepest root that holds it. Its
    /// environment is `env` and nothing else. Its output,
    /// standard output and standard error together, is captured: the first
    /// `head` bytes, the last `tail`, and a count of those dropped between.
    /// At the deadline io kills the whole tree and ends it as `Exited`, timed
    /// out, with what it captured. Ends in `Exited`, `Missing` or
    /// `NotDirectory` (for `cwd`), `Escapes`, `Failed` (it could not start),
    /// or `Cancelled` (killed by a cancel).
    Spawn { cwd: Place, command: Box<[u8]>, env: Box<[Var]>, roots: Box<[Root]>, head: u32, tail: u32 },
    /// Search the files at and beneath `at` for lines matching `pattern`, in
    /// those whose names match `glob` if given, with rg run as a contained
    /// process: it follows no link beneath `at`, and skips what rg skips by
    /// default (hidden and ignored files, binary ones). The protocol layer
    /// decodes its output into hits, in path order: at most `hits` of them,
    /// with at most `bytes` of text between them, the last one cut to fit;
    /// the hits beyond are counted. Ends in `Found`, `Exited` (rg failed, as
    /// for a pattern it cannot read: its exit, and at most `bytes` of what it
    /// wrote to standard error in `head`), `Missing`, `NotDirectory`,
    /// `Escapes`, or a common terminal.
    Search { at: Place, pattern: Box<[u8]>, glob: Option<Box<[u8]>>, hits: u32, bytes: u32 },
}

/// A repository a command sees: io's name for its root, and whether the
/// command may write in it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Root {
    pub root: Token,
    pub writable: bool,
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
    /// Spawn: the command ended so, with the output captured. Search: rg
    /// failed so.
    Exited { exit: Exit, head: Box<[u8]>, tail: Box<[u8]>, dropped: u64 },
    /// Search: the lines found, and how many more matched.
    Found { hits: Box<[Hit]>, more: u64 },
    /// Load, Scan: nothing is at the place.
    Missing,
    /// Load, Store: what is at the place is not a regular file.
    NotFile,
    /// Store: a part of the place is a symbolic link, which a store does not
    /// follow.
    Linked,
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
