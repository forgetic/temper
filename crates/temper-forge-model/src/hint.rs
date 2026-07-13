//! Portable change hints for wake-driven runners.
//!
//! A [`ChangeHint`] is an edge-triggered accelerator, not state. Consumers use
//! it only to decide when to re-run their normal Forge reads.

use crate::ids::ItemNumber;
use crate::model::RepositoryPath;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Artifact namespace for an item-addressed hint.
///
/// Issue and pull-request numbers may overlap in provider APIs, so the number
/// must never be interpreted without this explicit kind.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HintArtifactKind {
    Issue,
    PullRequest,
}

/// Scope addressed by a change hint.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HintTarget {
    /// The event cannot safely be associated with one artifact.
    Repository,
    /// One explicitly typed issue or pull request changed.
    Artifact {
        kind: HintArtifactKind,
        number: ItemNumber,
    },
}

/// Provider-neutral kind of mutation represented by a wake hint.
///
/// Artifact identity is carried independently by [`HintTarget`]. In particular,
/// a comment, review, or CI event does not imply whether its number belongs to
/// the issue or pull-request namespace.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Created,
    Edited,
    Body,
    Title,
    State,
    Label,
    Dependency,
    Assignee,
    Comment,
    Review,
    Push,
    Ci,
    Unknown,
}

/// Provider-neutral signal that a repository or artifact may have changed.
///
/// Hints are never trusted as state. They only shorten the wait before a worker
/// performs its existing authoritative Forge scan.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ChangeHint {
    pub repo: RepositoryPath,
    pub target: HintTarget,
    pub change: ChangeKind,
}

impl ChangeHint {
    /// Creates a repository-scoped hint.
    pub fn repository(repo: RepositoryPath, change: ChangeKind) -> Self {
        Self {
            repo,
            target: HintTarget::Repository,
            change,
        }
    }

    /// Creates an explicitly typed artifact-scoped hint.
    pub fn artifact(
        repo: RepositoryPath,
        kind: HintArtifactKind,
        number: ItemNumber,
        change: ChangeKind,
    ) -> Self {
        Self {
            repo,
            target: HintTarget::Artifact { kind, number },
            change,
        }
    }

    /// Creates an issue-scoped hint.
    pub fn issue(repo: RepositoryPath, number: ItemNumber, change: ChangeKind) -> Self {
        Self::artifact(repo, HintArtifactKind::Issue, number, change)
    }

    /// Creates a pull-request-scoped hint.
    pub fn pull_request(repo: RepositoryPath, number: ItemNumber, change: ChangeKind) -> Self {
        Self::artifact(repo, HintArtifactKind::PullRequest, number, change)
    }

    /// Returns the typed artifact address, when this hint is item-scoped.
    pub fn artifact_target(&self) -> Option<(HintArtifactKind, ItemNumber)> {
        match self.target {
            HintTarget::Repository => None,
            HintTarget::Artifact { kind, number } => Some((kind, number)),
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

impl<T: ChangeSource + ?Sized> ChangeSource for Box<T> {
    fn recv_timeout(&mut self, timeout: Duration) -> ChangeSourceEvent {
        (**self).recv_timeout(timeout)
    }

    fn try_recv(&mut self) -> ChangeSourceEvent {
        (**self).try_recv()
    }
}
