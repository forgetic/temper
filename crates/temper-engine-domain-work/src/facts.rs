//! What the hub tells whoever watches the engine (engine-domain.md, section
//! 11): a fact for each step of an item's lifecycle, content-free (items'
//! names, attempts and classifications, never what a run said or what was
//! written), in a bounded queue the parent drains at its own pace.
//!
//! Facts are outside the boundary's flow control: they are not requests, take
//! no room in `out`, and when the queue is full they are dropped and counted.
//! Nothing the hub decides depends on whether a fact was kept. An item refused
//! at the entrance tells nothing: it was not taken in.

use skein_lib::Queue;

use crate::boundary::{Class, Hold, Item};

/// Something that happened to `item`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Fact {
    /// It was taken in.
    Taken { item: Item },
    /// A run was claimed for it: the attempt `attempt`.
    Claimed { item: Item, attempt: u64 },
    /// Its attempt is on a worker.
    Placed { item: Item, attempt: u64 },
    /// Its attempt ended with an outcome.
    Ended { item: Item, attempt: u64 },
    /// Its attempt parked.
    Parked { item: Item, attempt: u64 },
    /// Its attempt failed, in `class`.
    Failed { item: Item, attempt: u64, class: Class },
    /// Its attempt was refused before anything ran.
    Refused { item: Item, attempt: u64 },
    /// An outcome's or an action's writes were made.
    Applied { item: Item },
    /// An outcome or an action was stale: nothing was made.
    Stale { item: Item },
    /// It is held for a person.
    Held { item: Item, why: Hold },
    /// A person released it.
    Released { item: Item },
    /// Its step is done.
    Done { item: Item },
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
