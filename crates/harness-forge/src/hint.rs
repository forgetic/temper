//! Portable change hints for wake-driven runners.
//!
//! A [`ChangeHint`] is an edge-triggered accelerator, not state. Consumers use
//! it only to decide when to re-run their normal Forge reads.

use crate::ids::ItemNumber;
use crate::model::RepositoryPath;
use std::time::Duration;

/// Coarse kind of provider event represented by a wake hint.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ChangeKind {
    Issue,
    PullRequest,
    Label,
    Comment,
    Review,
    Push,
    Ci,
    Unknown,
}

/// Provider-neutral signal that a repository or item may have changed.
///
/// Hints are never trusted as state. They only shorten the wait before a worker
/// performs its existing authoritative Forge scan.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChangeHint {
    pub repo: RepositoryPath,
    pub item: Option<ItemNumber>,
    pub kind: ChangeKind,
}

impl ChangeHint {
    /// Creates a repository-scoped hint.
    pub fn repo(repo: RepositoryPath, kind: ChangeKind) -> Self {
        Self {
            repo,
            item: None,
            kind,
        }
    }

    /// Creates an item-scoped hint.
    pub fn item(repo: RepositoryPath, item: ItemNumber, kind: ChangeKind) -> Self {
        Self {
            repo,
            item: Some(item),
            kind,
        }
    }
}

/// Result of waiting for a companion hint source.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ChangeSourceEvent {
    /// A hint was delivered.
    Hint(ChangeHint),
    /// No hint arrived before the caller's timeout.
    Timeout,
    /// The source was closed and cannot deliver more hints.
    Closed,
}

/// Optional companion surface for backends or adapters that can publish hints.
///
/// This trait is deliberately separate from [`crate::Forge`]. Polling remains
/// mandatory; a closed, lossy, duplicate, stale, or broad source only affects
/// latency because consumers must re-read Forge state after every wake.
pub trait ChangeSource {
    /// Waits up to `timeout` for one hint.
    fn recv_timeout(&mut self, timeout: Duration) -> ChangeSourceEvent;

    /// Attempts to receive one already-available hint without blocking.
    fn try_recv(&mut self) -> ChangeSourceEvent;
}
