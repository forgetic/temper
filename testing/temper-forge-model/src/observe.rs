//! What happened on the forge, content and all, for a referee
//! (testing-pyramid.md, 5.2): every change a call, CI or another party made,
//! in a bounded queue a world drains at its own pace.
//!
//! Observations are outside the boundary: they are not requests, take no room
//! in `out`, and when the queue is full they are dropped and counted. Nothing
//! the forge does depends on whether one was kept. A world's setup is not
//! observed.

use alloc::boxed::Box;

use temper_lib::Queue;

use crate::api::{Check, Kind, Verdict};

/// A change on the forge, in `repository`, made by the user `by`.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Observation {
    /// `branch` moved from `from` to `to`, or was made at `to` if it was
    /// nowhere.
    Moved {
        repository: Box<[u8]>,
        branch: Box<[u8]>,
        from: Option<u64>,
        to: u64,
        by: u64,
    },
    /// `branch`, which was at `at`, was deleted.
    Deleted {
        repository: Box<[u8]>,
        branch: Box<[u8]>,
        at: u64,
        by: u64,
    },
    /// The item `number` was opened, carrying `labels`.
    Opened {
        repository: Box<[u8]>,
        number: u64,
        kind: Kind,
        title: Box<[u8]>,
        body: Box<[u8]>,
        labels: Box<[Box<[u8]>]>,
        by: u64,
    },
    Closed {
        repository: Box<[u8]>,
        number: u64,
        by: u64,
    },
    Reopened {
        repository: Box<[u8]>,
        number: u64,
        by: u64,
    },
    /// The item `number`'s labels became `labels`.
    Labelled {
        repository: Box<[u8]>,
        number: u64,
        labels: Box<[Box<[u8]>]>,
        by: u64,
    },
    /// The label `label` was defined.
    Defined {
        repository: Box<[u8]>,
        label: Box<[u8]>,
        by: u64,
    },
    /// The comment `id` was posted on the item `number`.
    Commented {
        repository: Box<[u8]>,
        number: u64,
        id: u64,
        body: Box<[u8]>,
        by: u64,
    },
    /// The comment `id` on the item `number` now says `body`.
    Edited {
        repository: Box<[u8]>,
        number: u64,
        id: u64,
        body: Box<[u8]>,
        by: u64,
    },
    /// The comment `id` on the item `number` was deleted.
    Removed {
        repository: Box<[u8]>,
        number: u64,
        id: u64,
        by: u64,
    },
    /// The pull request `number` was reviewed at `commit`.
    Reviewed {
        repository: Box<[u8]>,
        number: u64,
        commit: u64,
        verdict: Verdict,
        body: Box<[u8]>,
        by: u64,
    },
    /// `context` on `commit` is `state`.
    Reported {
        repository: Box<[u8]>,
        commit: u64,
        context: Box<[u8]>,
        state: Check,
        by: u64,
    },
    /// The pull request `number` merged its head `head` as `commit` on its
    /// base.
    Merged {
        repository: Box<[u8]>,
        number: u64,
        head: u64,
        commit: u64,
        by: u64,
    },
    /// The wiki page `name` now holds `content`, at `revision`, or was
    /// deleted.
    Wiki {
        repository: Box<[u8]>,
        name: Box<[u8]>,
        content: Option<Box<[u8]>>,
        revision: u64,
        by: u64,
    },
}

/// The observations not yet drained, and how many did not fit.
#[derive(Debug)]
pub(crate) struct Observations {
    queue: Queue<Observation>,
    lost: u64,
}

impl Observations {
    pub(crate) fn with_capacity(capacity: u32) -> Observations {
        Observations { queue: Queue::with_capacity(capacity), lost: 0 }
    }

    /// Keeps `observation` if there is room for it, and counts it otherwise.
    pub(crate) fn push(&mut self, observation: Observation) {
        if self.queue.try_push(observation).is_err() {
            self.lost = self.lost.saturating_add(1);
        }
    }

    pub(crate) fn pop(&mut self) -> Option<Observation> {
        self.queue.pop()
    }

    pub(crate) fn lost(&self) -> u64 {
        self.lost
    }
}
