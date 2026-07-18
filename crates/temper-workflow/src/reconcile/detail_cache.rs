//! Bounded native-dependency detail retained between reconciliation passes.
//!
//! Candidate discovery always supplies fresh summary fields. This cache keeps
//! only the enrichment omitted from those summaries: native dependency item
//! numbers. A canonical summary fingerprint fences reuse, while forced refresh
//! and unseen-entry retention bounds guarantee convergence and memory use. The
//! production 15-minute refresh combined with the normal mechanical cadence
//! bounds a missed dependency hint to roughly 17 minutes plus execution time.

use crate::ArtifactSource;
use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use temper_forge::{Issue, ItemNumber, PullRequest, RepositoryId};

/// Production policy for reconciliation dependency detail retention.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReconciliationDetailCachePolicy {
    /// Maximum entries retained across all repositories and artifact types.
    pub max_entries: usize,
    /// Maximum age of reused native dependency detail.
    pub forced_refresh_after: Duration,
    /// How long an entry not encountered by a candidate pass may remain.
    pub evict_unseen_after: Duration,
}

impl ReconciliationDetailCachePolicy {
    /// Creates an injectable cache policy.
    pub const fn new(
        max_entries: usize,
        forced_refresh_after: Duration,
        evict_unseen_after: Duration,
    ) -> Self {
        Self {
            max_entries,
            forced_refresh_after,
            evict_unseen_after,
        }
    }
}

impl Default for ReconciliationDetailCachePolicy {
    fn default() -> Self {
        Self::new(
            2_048,
            Duration::from_secs(15 * 60),
            Duration::from_secs(30 * 60),
        )
    }
}

/// Detail-cache activity attributable to one reconciliation pass.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ReconciliationDetailCacheStats {
    pub hits: u64,
    pub misses: u64,
    pub forced_refreshes: u64,
    pub invalidations: u64,
    pub evictions: u64,
}

impl ReconciliationDetailCacheStats {
    /// Adds invalidation events to this pass without overflowing.
    pub fn add_invalidations(&mut self, count: usize) {
        self.invalidations = self
            .invalidations
            .saturating_add(u64::try_from(count).unwrap_or(u64::MAX));
    }

    /// Adds eviction events to this pass without overflowing.
    pub fn add_evictions(&mut self, count: usize) {
        self.evictions = self
            .evictions
            .saturating_add(u64::try_from(count).unwrap_or(u64::MAX));
    }
}

/// Thread-safe cache shared by long-lived mechanical runtime owners.
///
/// Clones share the same bounded state. Constructing a new value starts empty,
/// which makes process startup and worker restart an authoritative cold fill.
#[derive(Clone)]
pub struct ReconciliationDetailCache {
    inner: Arc<Mutex<CacheState>>,
    policy: ReconciliationDetailCachePolicy,
}

impl Default for ReconciliationDetailCache {
    fn default() -> Self {
        Self::new(ReconciliationDetailCachePolicy::default())
    }
}

impl ReconciliationDetailCache {
    /// Creates an empty cache with `policy`.
    pub fn new(policy: ReconciliationDetailCachePolicy) -> Self {
        Self {
            inner: Arc::new(Mutex::new(CacheState::default())),
            policy,
        }
    }

    /// Returns the configured bounds.
    pub const fn policy(&self) -> ReconciliationDetailCachePolicy {
        self.policy
    }

    /// Number of currently retained entries.
    pub fn len(&self) -> usize {
        self.inner.lock().expect("detail cache mutex").entries.len()
    }

    /// Whether no dependency detail is retained.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Evicts entries that have not appeared in a candidate pass within the
    /// configured retention age.
    pub(crate) fn begin_pass(
        &self,
        now: DateTime<Utc>,
        stats: &mut ReconciliationDetailCacheStats,
    ) {
        let mut state = self.inner.lock().expect("detail cache mutex");
        let before = state.entries.len();
        let retention = self.policy.evict_unseen_after;
        state
            .entries
            .retain(|_, entry| !elapsed_at_least(now, entry.last_seen_at, retention));
        stats.add_evictions(before.saturating_sub(state.entries.len()));
    }

    pub(crate) fn issue_dependencies(
        &self,
        repo_id: &RepositoryId,
        issue: &Issue,
        now: DateTime<Utc>,
        stats: &mut ReconciliationDetailCacheStats,
    ) -> Option<Vec<ItemNumber>> {
        self.dependencies(
            CacheKey::new(
                repo_id,
                ArtifactSource::Issue {
                    number: issue.number,
                },
            ),
            SummaryFingerprint::from_issue(issue),
            now,
            stats,
        )
    }

    pub(crate) fn pull_request_dependencies(
        &self,
        repo_id: &RepositoryId,
        pull_request: &PullRequest,
        now: DateTime<Utc>,
        stats: &mut ReconciliationDetailCacheStats,
    ) -> Option<Vec<ItemNumber>> {
        self.dependencies(
            CacheKey::new(
                repo_id,
                ArtifactSource::PullRequest {
                    number: pull_request.number,
                },
            ),
            SummaryFingerprint::from_pull_request(pull_request),
            now,
            stats,
        )
    }

    fn dependencies(
        &self,
        key: CacheKey,
        fingerprint: SummaryFingerprint,
        now: DateTime<Utc>,
        stats: &mut ReconciliationDetailCacheStats,
    ) -> Option<Vec<ItemNumber>> {
        let mut state = self.inner.lock().expect("detail cache mutex");
        let Some(entry) = state.entries.get_mut(&key) else {
            stats.misses = stats.misses.saturating_add(1);
            return None;
        };
        if entry.fingerprint != fingerprint {
            stats.misses = stats.misses.saturating_add(1);
            return None;
        }
        entry.last_seen_at = now;
        if elapsed_at_least(
            now,
            entry.detail_refreshed_at,
            self.policy.forced_refresh_after,
        ) {
            stats.forced_refreshes = stats.forced_refreshes.saturating_add(1);
            return None;
        }
        stats.hits = stats.hits.saturating_add(1);
        Some(entry.dependencies.clone())
    }

    /// Seeds or replaces an issue entry from a successful full detail read.
    pub fn store_issue(&self, repo_id: &RepositoryId, issue: &Issue, now: DateTime<Utc>) -> usize {
        self.store_issue_dependencies(repo_id, issue, issue.dependencies.clone(), now)
    }

    pub(crate) fn store_issue_dependencies(
        &self,
        repo_id: &RepositoryId,
        summary: &Issue,
        dependencies: Vec<ItemNumber>,
        now: DateTime<Utc>,
    ) -> usize {
        self.store(
            CacheKey::new(
                repo_id,
                ArtifactSource::Issue {
                    number: summary.number,
                },
            ),
            SummaryFingerprint::from_issue(summary),
            dependencies,
            now,
        )
    }

    /// Seeds or replaces a pull-request entry from a successful full detail read.
    pub fn store_pull_request(
        &self,
        repo_id: &RepositoryId,
        pull_request: &PullRequest,
        now: DateTime<Utc>,
    ) -> usize {
        self.store_pull_request_dependencies(
            repo_id,
            pull_request,
            pull_request.dependencies.clone(),
            now,
        )
    }

    pub(crate) fn store_pull_request_dependencies(
        &self,
        repo_id: &RepositoryId,
        summary: &PullRequest,
        dependencies: Vec<ItemNumber>,
        now: DateTime<Utc>,
    ) -> usize {
        self.store(
            CacheKey::new(
                repo_id,
                ArtifactSource::PullRequest {
                    number: summary.number,
                },
            ),
            SummaryFingerprint::from_pull_request(summary),
            dependencies,
            now,
        )
    }

    fn store(
        &self,
        key: CacheKey,
        fingerprint: SummaryFingerprint,
        mut dependencies: Vec<ItemNumber>,
        now: DateTime<Utc>,
    ) -> usize {
        dependencies.sort();
        dependencies.dedup();
        let mut state = self.inner.lock().expect("detail cache mutex");
        if self.policy.max_entries == 0 {
            return 0;
        }
        let mut evictions = 0;
        if !state.entries.contains_key(&key) && state.entries.len() >= self.policy.max_entries {
            if let Some(lru_key) = state
                .entries
                .iter()
                .min_by(|(left_key, left), (right_key, right)| {
                    left.last_seen_at
                        .cmp(&right.last_seen_at)
                        .then_with(|| left_key.cmp(right_key))
                })
                .map(|(key, _)| key.clone())
            {
                state.entries.remove(&lru_key);
                evictions = 1;
            }
        }
        state.entries.insert(
            key,
            CacheEntry {
                dependencies,
                fingerprint,
                detail_refreshed_at: now,
                last_seen_at: now,
            },
        );
        evictions
    }

    /// Invalidates one typed artifact entry, returning the number removed.
    pub fn invalidate(&self, repo_id: &RepositoryId, source: ArtifactSource) -> usize {
        usize::from(
            self.inner
                .lock()
                .expect("detail cache mutex")
                .entries
                .remove(&CacheKey::new(repo_id, source))
                .is_some(),
        )
    }

    /// Conservatively invalidates every entry for a repository.
    pub fn invalidate_repository(&self, repo_id: &RepositoryId) -> usize {
        let mut state = self.inner.lock().expect("detail cache mutex");
        let before = state.entries.len();
        state.entries.retain(|key, _| &key.repo_id != repo_id);
        before.saturating_sub(state.entries.len())
    }
}

#[derive(Default)]
struct CacheState {
    entries: BTreeMap<CacheKey, CacheEntry>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct CacheKey {
    repo_id: RepositoryId,
    source: ArtifactSource,
}

impl CacheKey {
    fn new(repo_id: &RepositoryId, source: ArtifactSource) -> Self {
        Self {
            repo_id: repo_id.clone(),
            source,
        }
    }
}

struct CacheEntry {
    dependencies: Vec<ItemNumber>,
    fingerprint: SummaryFingerprint,
    detail_refreshed_at: DateTime<Utc>,
    last_seen_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SummaryFingerprint([u8; 32]);

impl SummaryFingerprint {
    fn from_issue(issue: &Issue) -> Self {
        let mut fingerprint = FingerprintBuilder::new(b"issue");
        fingerprint.field(issue.id.as_str().as_bytes());
        fingerprint.field(issue.repo_id.as_str().as_bytes());
        fingerprint.number(issue.number.get());
        fingerprint.number(issue.version.get());
        fingerprint.timestamp(issue.updated_at);
        fingerprint.number(match issue.state {
            temper_forge::IssueState::Open => 0,
            temper_forge::IssueState::Closed => 1,
        });
        fingerprint.strings(issue.labels.clone());
        fingerprint.strings(
            issue
                .assignees
                .iter()
                .map(|assignee| assignee.as_str().to_string())
                .collect(),
        );
        fingerprint.field(issue.body.as_bytes());
        fingerprint.finish()
    }

    fn from_pull_request(pull_request: &PullRequest) -> Self {
        let mut fingerprint = FingerprintBuilder::new(b"pull_request");
        fingerprint.field(pull_request.id.as_str().as_bytes());
        fingerprint.field(pull_request.repo_id.as_str().as_bytes());
        fingerprint.number(pull_request.number.get());
        fingerprint.number(pull_request.version.get());
        fingerprint.timestamp(pull_request.updated_at);
        fingerprint.number(match pull_request.state {
            temper_forge::PullRequestState::Open => 0,
            temper_forge::PullRequestState::Closed => 1,
            temper_forge::PullRequestState::Merged => 2,
        });
        fingerprint.strings(pull_request.labels.clone());
        fingerprint.strings(
            pull_request
                .assignees
                .iter()
                .map(|assignee| assignee.as_str().to_string())
                .collect(),
        );
        fingerprint.field(pull_request.body.as_bytes());
        fingerprint.finish()
    }
}

struct FingerprintBuilder(Sha256);

impl FingerprintBuilder {
    fn new(artifact_type: &[u8]) -> Self {
        let mut builder = Self(Sha256::new());
        builder.field(artifact_type);
        builder
    }

    fn field(&mut self, value: &[u8]) {
        self.0
            .update(u64::try_from(value.len()).unwrap_or(u64::MAX).to_be_bytes());
        self.0.update(value);
    }

    fn number(&mut self, value: u64) {
        self.field(&value.to_be_bytes());
    }

    fn timestamp(&mut self, value: DateTime<Utc>) {
        self.field(&value.timestamp().to_be_bytes());
        self.field(&value.timestamp_subsec_nanos().to_be_bytes());
    }

    fn strings(&mut self, values: Vec<String>) {
        let values = canonical(values);
        self.number(u64::try_from(values.len()).unwrap_or(u64::MAX));
        for value in values {
            self.field(value.as_bytes());
        }
    }

    fn finish(self) -> SummaryFingerprint {
        SummaryFingerprint(self.0.finalize().into())
    }
}

fn canonical<T: Ord>(mut values: Vec<T>) -> Vec<T> {
    values.sort();
    values.dedup();
    values
}

fn elapsed_at_least(now: DateTime<Utc>, then: DateTime<Utc>, age: Duration) -> bool {
    now.signed_duration_since(then)
        .to_std()
        .is_ok_and(|elapsed| elapsed >= age)
}
