//! What the views tell whoever watches the engine (engine-model.md, section
//! 11): a fact for each thing that happened, content-free (kinds and counts,
//! never a token or a report's bytes), in a bounded queue the parent drains
//! at its own pace.
//!
//! Facts are outside the boundary's flow control: they are not requests, take
//! no room in `out`, and when the queue is full they are dropped and counted.
//! Nothing the views decide depends on whether a fact was kept.

use temper_lib::Queue;

use crate::boundary::{End, Refusal};

/// Something that happened.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Fact {
    /// A run is followed from now on, or is not, for want of room.
    Followed,
    Unfollowed,
    /// A report was streamed to `watchers` watchers, and its trace kept so
    /// much of it.
    Reported {
        watchers: u32,
        kept: Kept,
    },
    /// A report was dropped, for `dropped`.
    Dropped {
        dropped: Dropped,
    },
    /// An item's phase change was streamed to `watchers` watchers.
    Changed {
        watchers: u32,
    },
    /// A watch was taken, refused at the entrance, or ended.
    Watching,
    Refused {
        refusal: Refusal,
    },
    Ended {
        end: End,
    },
    /// A watcher's backlog overflowed: `missed` chunks waiting were dropped,
    /// which it is told of with its next delivery.
    Overflowed {
        missed: u32,
    },
    /// A delivery of `chunks` chunks went to a watcher.
    Delivered {
        chunks: u32,
    },
    /// A batch of `records` records went to the store, which kept them, or
    /// failed to.
    Appending {
        records: u32,
    },
    Appended {
        done: bool,
    },
    /// A sweep asked the store to expire what is past its retention, which
    /// it did, or failed to.
    Expiring,
    Expired {
        done: bool,
    },
}

/// What a report's trace kept.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Kept {
    /// What the run's policy said: nothing, its shape, or its content too.
    Nothing,
    Shape,
    Content,
    /// Nothing, for want of room in the batch while the store is behind.
    Lost,
}

/// Why a report was dropped.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Dropped {
    /// Its run is not followed.
    Unfollowed,
    /// Its content is past the limits.
    Oversized,
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
