//! What the notes tell whoever watches the engine (engine-model.md, section
//! 11): a fact for each thing that happened, content-free (kinds and counts,
//! never a name, a description or a body), in a bounded queue the parent
//! drains at its own pace.
//!
//! Facts are outside the boundary's flow control: they are not requests, take
//! no room in `out`, and when the queue is full they are dropped and counted.
//! Nothing the notes decide depends on whether a fact was kept.

use temper_lib::Queue;

use crate::boundary::{Refusal, Wrote};

/// Something that happened.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Fact {
    /// A call was refused at the entrance.
    Refused {
        refusal: Refusal,
    },
    /// A scope is kept from now on: in a free place, or in that of the least
    /// recently used one, which was evicted.
    Kept {
        evicted: bool,
    },
    /// A scope's pages were listed: those its index follows, and those left
    /// out of it.
    Listed {
        pages: u32,
        left_out: u32,
    },
    /// A scope's pages could not be listed.
    Unlisted,
    /// A page was read, for an index or a recall, or could not be.
    Read,
    Unread,
    /// A page was written so.
    Wrote {
        wrote: Wrote,
    },
    /// A call was answered.
    Answered,
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
