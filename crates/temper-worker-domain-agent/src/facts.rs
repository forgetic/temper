//! What the agent child domain tells whoever watches the worker
//! (worker-domain.md, section 7): a fact for each step of an agent's life,
//! content-free (the client's token and classifications, never what the run
//! said), in a bounded queue the parent drains at its own pace.
//!
//! Facts are outside the boundary's flow control: they are not requests, take
//! no room in `out`, and when the queue is full they are dropped and counted.
//! Nothing the agent child domain decides depends on whether a fact was kept.
//! The run's own facts are not these: they go to the client as they are
//! (`Request::Told`), for the parent to forward.

use temper_lib::{Queue, Token};

use crate::boundary::{End, Fault};

/// Something that happened to the agent spawned for `client`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Fact {
    /// Its process spawned, and its run was started.
    Started { client: Token },
    /// Its run said how it finishes.
    Finished { client: Token },
    /// The client stopped it: its run was cancelled.
    Cancelled { client: Token },
    /// Its wall time ran out: its run was cancelled.
    Overdue { client: Token },
    /// It failed, for `fault`: the watchdog fired, it broke the rules, or it
    /// exited or outlived its wall time without saying how its run finishes.
    /// The client is told only while its run was live.
    Faulted { client: Token, fault: Fault },
    /// Its tree was told to exit, past the grace or for a fault.
    Terminated { client: Token },
    /// Its tree was killed, past the grace after it was terminated.
    Killed { client: Token },
    /// It has gone, for `end`, or was refused at the entrance.
    Gone { client: Token, end: End },
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
