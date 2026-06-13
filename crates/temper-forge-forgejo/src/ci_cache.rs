// SPDX-License-Identifier: MPL-2.0
//! Idle-read cache for the web-UI CI fallback (ADR 0019 cost mitigation).
//!
//! The web-UI CI read (`crate::ci_ui`) is expensive: it logs into Forgejo with a
//! username/password (a server-side bcrypt, ~hundreds of ms) and scrapes the
//! Actions page plus a live-view POST per run. The mechanical backstop reads CI
//! for every gated pull request on every tick, so on an **idle** repo — nothing
//! pushed, every run long since terminal — that cost repeats every tick for no
//! new information.
//!
//! This cache memoizes the last web-UI read keyed by the CI **target identity**
//! (the pull request / commit) plus its **head SHA**. A cached read is reused
//! only when it is *terminal* — every job has completed — because a terminal run
//! at an unchanged head SHA cannot change. A still-running or queued read is
//! never cached as reusable, so a settling CI is always re-read; and a new push
//! changes the head SHA, which changes the key, so the next read is a miss. The
//! result: idle ticks skip the login+scrape entirely while responsiveness to
//! real change (new SHA, or a run still in flight) is preserved.
//!
//! Correctness rests on the same re-read-everything contract the rest of the
//! runtime relies on (ADR 0009): the cache only ever *skips redundant work*; it
//! never invents a verdict. A miss falls through to the live web-UI read.

use std::collections::HashMap;
use std::sync::Mutex;

use temper_forge::{CiJob, CiJobStatus, RepositoryId};

use crate::ci_match::Target;

/// Identity of a CI read for caching: the resolved target plus its head SHA.
///
/// Two reads with the same key observe the same underlying runs, so a terminal
/// result for one is a terminal result for the other. The head SHA is the change
/// token — a new push mints a new SHA and therefore a new key.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct CiReadKey {
    /// `forgejo:<owner>/<repo>` — scopes the cache per repository.
    pub repo_id: String,
    /// The pull-request id when reading a PR's CI, else the bare commit SHA;
    /// distinguishes targets within a repo.
    pub target: String,
    /// The target's head SHA (PR head or commit). The change token: an unchanged
    /// SHA with a terminal result is safe to reuse; a changed SHA is a miss.
    pub head_sha: String,
}

impl CiReadKey {
    /// Derives a cache key from a resolved CI [`Target`], or `None` when the
    /// target has no concrete head SHA to change-track.
    ///
    /// Without a head SHA there is no change token, so such a read is never
    /// cached — it always falls through to a live read. The target identity is
    /// the PR id when present (the usual mechanical-gate case), else the bare
    /// commit SHA.
    pub(crate) fn from_target(repo_id: &RepositoryId, target: &Target) -> Option<Self> {
        let head_sha = target
            .pr_head_sha
            .as_deref()
            .filter(|sha| !sha.is_empty())
            .or(target.commit_sha.as_deref().filter(|sha| !sha.is_empty()))?;
        let identity = target
            .pr_id
            .as_ref()
            .map(|pr| pr.as_str().to_string())
            .or_else(|| target.commit_sha.clone())
            .unwrap_or_default();
        Some(Self {
            repo_id: repo_id.as_str().to_string(),
            target: identity,
            head_sha: head_sha.to_string(),
        })
    }
}

/// A cached web-UI CI read.
#[derive(Clone, Debug)]
struct CachedCiRead {
    jobs: Vec<CiJob>,
    /// Whether every job had completed when this was cached. Only a terminal read
    /// is reused; a non-terminal one is re-read so a settling CI is observed.
    terminal: bool,
}

/// Per-backend memo of terminal web-UI CI reads, keyed by [`CiReadKey`].
///
/// Interior-mutable like [`crate::VersionCache`]; shared via `Arc` so cloning the
/// backend shares one cache. A poisoned mutex is unrecoverable and panics, the
/// same as the version cache.
#[derive(Debug, Default)]
pub(crate) struct CiReadCache {
    reads: Mutex<HashMap<CiReadKey, CachedCiRead>>,
}

impl CiReadCache {
    /// Returns the cached jobs for `key` when a *terminal* read is stored.
    ///
    /// `None` means "read it": either nothing is cached, or the cached read was
    /// still in flight (non-terminal) and must be re-observed.
    pub(crate) fn get_terminal(&self, key: &CiReadKey) -> Option<Vec<CiJob>> {
        let reads = self.reads.lock().expect("ci read cache mutex poisoned");
        reads
            .get(key)
            .filter(|entry| entry.terminal)
            .map(|entry| entry.jobs.clone())
    }

    /// Records a web-UI read for `key`, computing terminality from the jobs.
    ///
    /// Storing a non-terminal read (rather than skipping it) lets a later tick
    /// see the same key is "known but not settled" without a separate structure;
    /// [`Self::get_terminal`] still forces a re-read until it settles.
    pub(crate) fn store(&self, key: CiReadKey, jobs: Vec<CiJob>) {
        let terminal = is_terminal(&jobs);
        let mut reads = self.reads.lock().expect("ci read cache mutex poisoned");
        reads.insert(key, CachedCiRead { jobs, terminal });
    }
}

/// A read is terminal when it has at least one job and every job has completed.
///
/// An empty read is **not** terminal: "no runs yet" is a transient pre-CI state
/// (the runner has not picked the push up), so it must be re-read rather than
/// frozen as "done with nothing".
fn is_terminal(jobs: &[CiJob]) -> bool {
    !jobs.is_empty() && jobs.iter().all(|job| job.status == CiJobStatus::Completed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use temper_forge::{CiJobConclusion, CiJobId, RepositoryId};

    fn job(status: CiJobStatus, conclusion: Option<CiJobConclusion>) -> CiJob {
        CiJob {
            id: CiJobId::new("forgejo:acme/widgets:actions:1:0:1"),
            repo_id: RepositoryId::new("forgejo:acme/widgets"),
            pull_request_id: None,
            commit_sha: "abc1234".to_string(),
            name: "build".to_string(),
            status,
            conclusion,
            url: None,
            created_at: chrono::DateTime::<chrono::Utc>::from_timestamp(1, 0).unwrap(),
            started_at: None,
            completed_at: None,
            updated_at: chrono::DateTime::<chrono::Utc>::from_timestamp(1, 0).unwrap(),
        }
    }

    fn key(head_sha: &str) -> CiReadKey {
        CiReadKey {
            repo_id: "forgejo:acme/widgets".to_string(),
            target: "forgejo:acme/widgets:pull:7".to_string(),
            head_sha: head_sha.to_string(),
        }
    }

    #[test]
    fn terminal_read_is_reused_for_same_key() {
        let cache = CiReadCache::default();
        cache.store(
            key("sha-a"),
            vec![job(CiJobStatus::Completed, Some(CiJobConclusion::Success))],
        );
        let hit = cache.get_terminal(&key("sha-a"));
        assert!(hit.is_some(), "a terminal read should be reused");
        assert_eq!(hit.unwrap().len(), 1);
    }

    #[test]
    fn running_read_is_not_reused() {
        let cache = CiReadCache::default();
        cache.store(key("sha-a"), vec![job(CiJobStatus::Running, None)]);
        assert!(
            cache.get_terminal(&key("sha-a")).is_none(),
            "a still-running read must be re-observed, not reused"
        );
    }

    #[test]
    fn changed_head_sha_is_a_miss() {
        let cache = CiReadCache::default();
        cache.store(
            key("sha-a"),
            vec![job(CiJobStatus::Completed, Some(CiJobConclusion::Success))],
        );
        assert!(
            cache.get_terminal(&key("sha-b")).is_none(),
            "a new head SHA mints a new key, so it must miss and re-read"
        );
    }

    #[test]
    fn empty_read_is_not_terminal() {
        let cache = CiReadCache::default();
        cache.store(key("sha-a"), vec![]);
        assert!(
            cache.get_terminal(&key("sha-a")).is_none(),
            "no runs yet is a transient pre-CI state, not a terminal result"
        );
    }

    #[test]
    fn mixed_statuses_are_not_terminal() {
        let cache = CiReadCache::default();
        cache.store(
            key("sha-a"),
            vec![
                job(CiJobStatus::Completed, Some(CiJobConclusion::Success)),
                job(CiJobStatus::Running, None),
            ],
        );
        assert!(
            cache.get_terminal(&key("sha-a")).is_none(),
            "if any job is still running the read is not terminal"
        );
    }
}
