//! What the forge sub-model tells whoever watches the engine (engine-model.md,
//! section 11): a fact for each thing that happened, content-free (items,
//! tokens and classifications, never what the forge or the parent said), in a
//! bounded queue the parent drains at its own pace.
//!
//! Facts are outside the boundary's flow control: they are not requests, take
//! no room in `out`, and when the queue is full they are dropped and counted.
//! Nothing the sub-model decides depends on whether a fact was kept.

use temper_lib::{Queue, Time, Token};

use crate::api::Error;
use crate::boundary::Item;

/// Which of the request budget's classes a call is in, first served first
/// (engine-model.md, section 12).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Priority {
    /// The parent's fresh reads, for its writes, its runs and its briefs.
    Fresh,
    /// Writes, and the reads that find what an attempt made.
    Write,
    /// Keeping up: listings and the reads of the items held.
    Keep,
    /// The slow pass over all open items.
    Slow,
}

/// Something that happened.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Fact {
    /// A call went out.
    Sent {
        priority: Priority,
    },
    /// A call failed.
    Failed {
        priority: Priority,
        error: Error,
    },
    /// The forge refused a call for the rate: nothing goes out until `reset`.
    Limited {
        reset: Time,
    },
    /// The budget's window is spent: nothing goes out until it ends.
    Spent {
        until: Time,
    },
    /// `item` entered the working set; did not, for want of room; left it.
    Admitted {
        item: Item,
    },
    Refused {
        item: Item,
    },
    Left {
        item: Item,
    },
    /// A listing of `repository` began.
    Listing {
        repository: u32,
    },
    /// The write `owner` was tried again; a creation of it was found made by
    /// an earlier attempt; it was done; it failed.
    Retried {
        owner: Token,
    },
    Found {
        owner: Token,
    },
    Wrote {
        owner: Token,
    },
    Unwritten {
        owner: Token,
    },
    /// `item`'s record was changed by someone else, and not written over.
    Edited {
        item: Item,
    },
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
