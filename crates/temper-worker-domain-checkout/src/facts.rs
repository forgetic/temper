//! What the checkout tells whoever watches the worker (worker-domain.md,
//! section 7): a fact for each thing that happened, content-free (tokens,
//! kinds and counts, never a name, a branch or a message), in a bounded queue
//! the parent drains at its own pace.
//!
//! Facts are outside the boundary's flow control: they are not requests, take
//! no room in `out`, and when the queue is full they are dropped and counted.
//! Nothing the checkout decides depends on whether a fact was kept.

use skein_lib::{Queue, Token};

use crate::boundary::{Prepared, Refusal};
use crate::git::{Done, Kind};

/// Something that happened for the client `client`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[expect(clippy::large_enum_variant, reason = "bounded diagnostics stay inline and are included in worst_case")]
pub enum Fact {
    /// A prepare, a push or a save was refused at the entrance.
    Refused { client: Token, refusal: Refusal },
    /// A prepare was admitted, and its workspace held, found in the cache so.
    Held { client: Token, cached: Cached },
    /// An operation was asked of io.
    Started { client: Token, op: Kind },
    /// The operation in flight ended so.
    Ended { client: Token, done: Done },
    /// The client asked to abort or release while an operation was in
    /// flight, which is cancelled.
    Aborting { client: Token },
    /// The prepare ended.
    Prepared { client: Token, prepared: Prepared },
    /// The push, or the save, ended with the repositories counted so.
    Pushed { client: Token, to: Target, tally: Tally },
    /// The workspace is back in the cache.
    Released { client: Token },
}

/// How a prepare found its workspace in the cache.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Cached {
    /// The workstream's, with the spec's repositories cloned: it is fetched
    /// and checked out afresh.
    Reused,
    /// The workstream's, but holding other repositories, or what it holds is
    /// not known after an operation that broke, ran out of time or was
    /// cancelled: it is made again, empty.
    Rebuilt,
    /// A new one: the cache had room.
    New,
    /// The least recently used workspace no client held, another
    /// workstream's: it is made again, empty, for this one.
    Evicted,
}

/// Where a push went.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Target {
    /// Each writable repository's push branch.
    Push,
    /// The saved-work branch.
    Saved,
}

/// The repositories of a push or a save, by what came of them.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Tally {
    pub landed: u32,
    pub moved: u32,
    pub failed: u32,
    pub refused: u32,
    pub unchanged: u32,
    pub aborted: u32,
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
    #[expect(clippy::large_types_passed_by_value, reason = "a bounded fact moves into its fixed-capacity queue")]
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
