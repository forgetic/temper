//! What the tools tell whoever watches the agent (agent-model.md, section 7):
//! a fact for each thing that happened, content-free (tokens, counts and
//! classifications, never a path, a file or what a command wrote), in a
//! bounded queue the parent drains at its own pace.
//!
//! Facts are outside the boundary's flow control: they are not requests, take
//! no room in `out`, and when the queue is full they are dropped and counted.
//! Nothing the tools decide depends on whether a fact was kept.

use temper_lib::{Queue, Token};

use crate::boundary::Refusal;
use crate::call::{Exit, Fault, Outcome, Tool};

/// Something that happened to the kit of the session `session`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Fact {
    /// The kit opened.
    Opened { session: Token },
    /// No kit was opened.
    Refused { session: Token, refusal: Refusal },
    /// A call passed the entrance, and runs.
    Started { session: Token, tool: Tool },
    /// A call was answered, so, with an outcome carrying `bytes` of payload
    /// (what was read, listed, found, or kept of a command's output). A call
    /// refused at the entrance is answered without having started.
    Answered { session: Token, tool: Tool, verdict: Verdict, bytes: u64 },
    /// The kit is closing, with `running` calls to settle.
    Closing { session: Token, running: u32 },
    /// The kit closed.
    Closed { session: Token },
}

/// How a call ended, without what it said: an outcome's kind.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Verdict {
    Read,
    Listed,
    Found,
    Written { created: bool },
    Edited { replaced: u32 },
    Exited { exit: Exit },
    NotGranted,
    Outside,
    ReadOnly,
    TooLong,
    NotFound,
    NotFile,
    Linked,
    Protected,
    NotDirectory,
    TooLarge,
    NotRead,
    Stale,
    NoMatch,
    Ambiguous { count: u32 },
    Unchanged,
    Failed { fault: Fault },
    TimedOut,
    Cancelled,
    Busy,
}

/// The fact of `outcome` answering a call of `tool` from `session`.
pub(crate) fn answered(session: Token, tool: Tool, outcome: &Outcome) -> Fact {
    let (verdict, bytes) = match outcome {
        Outcome::Read { content, .. } => (Verdict::Read, len(content)),
        Outcome::Listed { entries, .. } => {
            let mut bytes: u64 = 0;
            for entry in entries {
                bytes = bytes.saturating_add(len(entry.name.as_bytes()));
            }
            (Verdict::Listed, bytes)
        }
        Outcome::Found { hits, .. } => {
            let mut bytes: u64 = 0;
            for hit in hits {
                bytes = bytes.saturating_add(len(&hit.path)).saturating_add(len(&hit.text));
            }
            (Verdict::Found, bytes)
        }
        Outcome::Written { created } => (Verdict::Written { created: *created }, 0),
        Outcome::Edited { replaced } => (Verdict::Edited { replaced: *replaced }, 0),
        Outcome::Exited { exit, head, tail, dropped: _ } => {
            (Verdict::Exited { exit: *exit }, len(head).saturating_add(len(tail)))
        }
        Outcome::NotGranted => (Verdict::NotGranted, 0),
        Outcome::Outside => (Verdict::Outside, 0),
        Outcome::ReadOnly => (Verdict::ReadOnly, 0),
        Outcome::TooLong => (Verdict::TooLong, 0),
        Outcome::NotFound => (Verdict::NotFound, 0),
        Outcome::NotFile => (Verdict::NotFile, 0),
        Outcome::Linked => (Verdict::Linked, 0),
        Outcome::Protected => (Verdict::Protected, 0),
        Outcome::NotDirectory => (Verdict::NotDirectory, 0),
        Outcome::TooLarge { .. } => (Verdict::TooLarge, 0),
        Outcome::NotRead => (Verdict::NotRead, 0),
        Outcome::Stale => (Verdict::Stale, 0),
        Outcome::NoMatch => (Verdict::NoMatch, 0),
        Outcome::Ambiguous { count, .. } => (Verdict::Ambiguous { count: *count }, 0),
        Outcome::Unchanged => (Verdict::Unchanged, 0),
        Outcome::Failed { fault } => (Verdict::Failed { fault: *fault }, 0),
        Outcome::TimedOut => (Verdict::TimedOut, 0),
        Outcome::Cancelled => (Verdict::Cancelled, 0),
        Outcome::Busy => (Verdict::Busy, 0),
    };
    Fact::Answered { session, tool, verdict, bytes }
}

fn len(bytes: &[u8]) -> u64 {
    u64::try_from(bytes.len()).unwrap_or(u64::MAX)
}

/// The facts not yet drained, and how many did not fit.
#[derive(Debug)]
pub(crate) struct Facts {
    queue: Queue<Fact>,
    lost: u64,
}

impl Facts {
    pub(crate) fn with_capacity(capacity: u32) -> Facts {
        Facts { queue: Queue::with_capacity(capacity), lost: 0 }
    }

    /// Keeps `fact` if there is room for it, and counts it otherwise.
    pub(crate) fn push(&mut self, fact: Fact) {
        if self.queue.try_push(fact).is_err() {
            self.lost = self.lost.saturating_add(1);
        }
    }

    pub(crate) fn pop(&mut self) -> Option<Fact> {
        self.queue.pop()
    }

    pub(crate) fn lost(&self) -> u64 {
        self.lost
    }
}
