//! The calls a session's LLM makes and what comes of them: the vocabulary the
//! session and the protocol layer share with the tools.
//!
//! The protocol layer owns each tool's schema, decodes the JSON the LLM wrote
//! into a [`Call`], valid by construction, and renders an [`Outcome`] as the
//! text the LLM reads. A malformed call never gets here: the session answers
//! it. What an outcome holds, such as how much of a file comes back, is the
//! tools' decision.

use alloc::boxed::Box;

use temper_lib::Duration;

use crate::path::{Name, Path};

/// A call the LLM made to one of the tools.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Call {
    /// Read the file at `path`: its lines after the first `skip`, at most
    /// `lines` of them if given, within the tools' limit on what a read
    /// answers with.
    Read { path: Path, skip: u32, lines: Option<u32> },
    /// List the directory at `path`.
    List { path: Path },
    /// Search the files at and beneath `path` for `pattern`, a regular
    /// expression as rg reads it, in those whose names match `glob` if given.
    Search { path: Path, pattern: Box<[u8]>, glob: Option<Box<[u8]>> },
    /// Make the file at `path` hold `content`, creating it if there is none.
    Write { path: Path, content: Box<[u8]> },
    /// Replace `old` with `new` in the file at `path`: its one occurrence, or
    /// every one if `all`.
    Edit { path: Path, old: Box<[u8]>, new: Box<[u8]>, all: bool },
    /// Run `command` with the shell, in the working directory, for at most
    /// `timeout` if given and within the tools' own limit.
    Shell { command: Box<[u8]>, timeout: Option<Duration> },
}

/// What a call does to the checkout. Calls that only read may run side by
/// side; a call that writes runs alone.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Effect {
    Read,
    Write,
}

/// The effect of `call`. A command may do anything, so it writes.
#[must_use]
pub const fn effect(call: &Call) -> Effect {
    match call {
        Call::Read { .. } | Call::List { .. } | Call::Search { .. } => Effect::Read,
        Call::Write { .. } | Call::Edit { .. } | Call::Shell { .. } => Effect::Write,
    }
}

/// What came of a call: exactly one per call.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Outcome {
    /// A read: `content` is the lines after the first `skipped`, `lines` of
    /// them, of the `total` the file has. They are whole lines, but for a
    /// line longer than what a read answers with, which is `cut` at that
    /// limit.
    Read { content: Box<[u8]>, skipped: u32, lines: u32, total: u32, cut: bool },
    /// A listing: the directory's first entries in name order, and how many
    /// `more` it has.
    Listed { entries: Box<[Entry]>, more: u64 },
    /// The file holds what was written; it was `created` if there was none.
    Written { created: bool },
    /// The edit `replaced` that many occurrences.
    Edited { replaced: u32 },

    /// The kit was not granted the call's family of tools.
    NotGranted,
    /// The path is outside every repository of the checkout, or a symbolic
    /// link on the way to it leads out of its repository.
    Outside,
    /// The path is in a repository the kit may not write.
    ReadOnly,
    /// The path is longer than the tools take.
    TooLong,
    /// Nothing is at the path.
    NotFound,
    /// What is at the path is not a regular file: a directory, a device.
    NotFile,
    /// The path of a write or an edit goes through a symbolic link, which
    /// they do not follow, so that a change lands only in the repository its
    /// path names. Reads follow links inside the checkout.
    Linked,
    /// The path of a write or an edit is in a repository's `.git`, which the
    /// tools do not change.
    Protected,
    /// What is at the path, or a directory on the way to it, is not a
    /// directory.
    NotDirectory,
    /// The file, or the content to write, is `size` bytes: more than the
    /// tools load or store.
    TooLarge { size: u64 },
    /// The file exists and the LLM has not read it, so it may not be changed.
    NotRead,
    /// The file changed since the LLM read it, so it may not be changed
    /// until the LLM reads it again.
    Stale,
    /// The snippet to replace is not in the file (an empty one never is).
    NoMatch,
    /// The snippet to replace is in the file `count` times, and the edit
    /// was for one: `lines` are where the first few start, counting from 1.
    Ambiguous { count: u32, lines: Box<[u32]> },
    /// The edit would leave the file as it is: the snippet and its
    /// replacement are the same.
    Unchanged,
    /// io failed.
    Failed { fault: Fault },
    /// The call did not finish by its deadline.
    TimedOut,
    /// The kit closed while the call was running.
    Cancelled,
    /// The kit has as many calls running as it may.
    Busy,
    /// The tools do not run this call yet: search and shell come next.
    Unsupported,
}

/// An entry of a listed directory.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Entry {
    pub name: Name,
    pub kind: Kind,
}

/// What an entry is, as io found it, without following symbolic links.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Kind {
    File,
    Directory,
    Link,
    /// A device, a socket, a pipe.
    Other,
}

/// Why io failed, as the protocol layer classifies the error.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Fault {
    /// Permission denied.
    Denied,
    /// No space left, or over a quota.
    NoSpace,
    /// Anything else.
    Other,
}
