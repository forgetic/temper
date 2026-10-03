//! What the brief tells whoever watches the engine (engine-domain.md, section
//! 11): a fact for each thing that happened, content-free (kinds and counts,
//! never a section's bytes), in a bounded queue the parent drains at its own
//! pace.
//!
//! Facts are outside the boundary's flow control: they are not requests,
//! take no room in `out`, and when the queue is full they are dropped and
//! counted. Nothing the brief decides depends on whether a fact was kept.

use skein_lib::Queue;

use crate::boundary::{Kind, Refusal};

/// Something that happened.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Fact {
    /// A brief was refused at the entrance.
    Refused { refusal: Refusal },
    /// A brief began gathering this many sections.
    Gathering { sections: u32 },
    /// A read ended so.
    Read { read: Gathered },
    /// A brief's time ran out with this many reads in flight.
    Expired { waiting: u32 },
    /// A brief was rendered: its sections, how many of them were cut, and
    /// how many are missing.
    Rendered { sections: u32, cut: u32, missing: u32 },
    /// A brief failed: a required section of this kind could not be read.
    Failed { missing: Kind },
}

/// How a read ended.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Gathered {
    /// With content, while its brief was gathering.
    Got,
    /// Failed, or with more than a read may bring.
    Failed,
    Oversized,
    /// After its brief had answered: dropped.
    Late,
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
