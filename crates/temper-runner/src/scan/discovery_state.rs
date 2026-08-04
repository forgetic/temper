//! Long-lived, bounded authority for terminal candidate discovery.
//!
//! Per-pass role and mechanical workers are routinely reconstructed. This
//! clone-shared owner keeps their repository-scoped sweep cursors and retained
//! exact recovery targets outside those workers. A fresh owner is deliberately
//! cold: process restart starts a new authoritative sweep rather than claiming
//! authority from a newest-only page.

use super::ArtifactAddress;
use chrono::{DateTime, Utc};
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::sync::{Arc, Mutex};
use temper_forge::{
    CandidateContinuation, CandidateLabelSelection, CandidateLifecycle, IssueId, PullRequestId,
    RepositoryId,
};

/// Explicit memory limits for shared terminal discovery state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TerminalDiscoveryPolicy {
    pub max_repositories: usize,
    pub max_buckets_per_repository: usize,
    pub max_retained_targets_per_repository: usize,
    pub max_workflow_fingerprint_bytes: usize,
}

impl TerminalDiscoveryPolicy {
    pub const fn new(
        max_repositories: usize,
        max_buckets_per_repository: usize,
        max_retained_targets_per_repository: usize,
        max_workflow_fingerprint_bytes: usize,
    ) -> Self {
        Self {
            max_repositories,
            max_buckets_per_repository,
            max_retained_targets_per_repository,
            max_workflow_fingerprint_bytes,
        }
    }
}

impl Default for TerminalDiscoveryPolicy {
    fn default() -> Self {
        Self::new(64, 8, 256, 256)
    }
}

/// Stable identity of one terminal issue or pull-request query bucket.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum TerminalDiscoveryBucket {
    Issues(Vec<String>),
    PullRequests(Vec<String>),
}

impl TerminalDiscoveryBucket {
    pub fn issues(labels: CandidateLabelSelection) -> Result<Self, TerminalDiscoveryStateError> {
        normalized_labels(labels).map(Self::Issues)
    }

    pub fn pull_requests(
        labels: CandidateLabelSelection,
    ) -> Result<Self, TerminalDiscoveryStateError> {
        normalized_labels(labels).map(Self::PullRequests)
    }

    fn labels(&self) -> &[String] {
        match self {
            Self::Issues(labels) | Self::PullRequests(labels) => labels,
        }
    }
}

/// Typed portable continuation retained for a terminal bucket.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TerminalDiscoveryContinuation {
    Issue(CandidateContinuation<IssueId>),
    PullRequest(CandidateContinuation<PullRequestId>),
}

impl TerminalDiscoveryContinuation {
    fn repository_id(&self) -> &RepositoryId {
        match self {
            Self::Issue(cursor) => &cursor.repository_id,
            Self::PullRequest(cursor) => &cursor.repository_id,
        }
    }

    fn labels(&self) -> &CandidateLabelSelection {
        match self {
            Self::Issue(cursor) => &cursor.labels,
            Self::PullRequest(cursor) => &cursor.labels,
        }
    }

    fn boundary(&self) -> DateTime<Utc> {
        match self {
            Self::Issue(cursor) => cursor.boundary.updated_at,
            Self::PullRequest(cursor) => cursor.boundary.updated_at,
        }
    }

    fn advances_from(&self, previous: &Self) -> bool {
        match (previous, self) {
            (Self::Issue(previous), Self::Issue(next)) => {
                next.boundary == previous.boundary && next.after > previous.after
            }
            (Self::PullRequest(previous), Self::PullRequest(next)) => {
                next.boundary == previous.boundary && next.after > previous.after
            }
            _ => false,
        }
    }

    fn matches_bucket(&self, bucket: &TerminalDiscoveryBucket) -> bool {
        matches!(
            (self, bucket),
            (Self::Issue(_), TerminalDiscoveryBucket::Issues(_))
                | (
                    Self::PullRequest(_),
                    TerminalDiscoveryBucket::PullRequests(_)
                )
        )
    }
}

/// Successful provider page update. Failed requests never construct or commit
/// this value; use [`TerminalDiscoveryState::record_failed_page`] instead.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TerminalDiscoveryPageCommit {
    pub continuation: Option<TerminalDiscoveryContinuation>,
    pub exhausted: bool,
    pub overflow: bool,
    /// Boundary observed by the backend. Required on a first exhausted page,
    /// whose portable result has no continuation carrying the boundary.
    pub sweep_boundary: Option<DateTime<Utc>>,
    pub retained_targets: Vec<ArtifactAddress>,
}

/// Observable state for one terminal bucket.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TerminalDiscoveryBucketSnapshot {
    pub continuation: Option<TerminalDiscoveryContinuation>,
    pub sweep_boundary: Option<DateTime<Utc>>,
    pub complete: bool,
    pub overflow: bool,
    pub failed: bool,
}

/// Repository-scoped discovery authority returned to role/mechanical consumers.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TerminalDiscoverySnapshot {
    pub repository_id: RepositoryId,
    pub workflow_fingerprint: String,
    pub cache_reused: bool,
    pub authoritative: bool,
    pub retained_overflow: bool,
    pub retained_targets: Vec<ArtifactAddress>,
    pub buckets: BTreeMap<TerminalDiscoveryBucket, TerminalDiscoveryBucketSnapshot>,
}

/// Result of committing a successful page.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TerminalDiscoveryCommitOutcome {
    Advanced,
    Complete,
    RestartedNonAdvancing,
}

/// Bounded, clone-shared terminal discovery authority.
#[derive(Clone)]
pub struct TerminalDiscoveryState {
    inner: Arc<Mutex<DiscoveryState>>,
    policy: TerminalDiscoveryPolicy,
}

impl Default for TerminalDiscoveryState {
    fn default() -> Self {
        Self::new(TerminalDiscoveryPolicy::default())
    }
}

impl TerminalDiscoveryState {
    pub fn new(policy: TerminalDiscoveryPolicy) -> Self {
        Self {
            inner: Arc::new(Mutex::new(DiscoveryState::default())),
            policy,
        }
    }

    pub const fn policy(&self) -> TerminalDiscoveryPolicy {
        self.policy
    }

    pub fn repository_count(&self) -> usize {
        self.inner
            .lock()
            .expect("terminal discovery state mutex")
            .repositories
            .len()
    }

    /// Begins or resumes one repository authority-building sweep.
    ///
    /// A workflow fingerprint or bucket-set change invalidates all cursors and
    /// completion state while preserving bounded exact recovery targets.
    pub fn begin(
        &self,
        repository_id: &RepositoryId,
        workflow_fingerprint: impl Into<String>,
        buckets: impl IntoIterator<Item = TerminalDiscoveryBucket>,
    ) -> Result<TerminalDiscoverySnapshot, TerminalDiscoveryStateError> {
        let workflow_fingerprint = workflow_fingerprint.into();
        self.validate_fingerprint(&workflow_fingerprint)?;
        let buckets = buckets.into_iter().collect::<BTreeSet<_>>();
        if buckets.len() > self.policy.max_buckets_per_repository {
            return Err(TerminalDiscoveryStateError::BucketCapacity {
                requested: buckets.len(),
                maximum: self.policy.max_buckets_per_repository,
            });
        }

        let mut state = self.inner.lock().expect("terminal discovery state mutex");
        let existed = state.repositories.contains_key(repository_id);
        if !existed && state.repositories.len() >= self.policy.max_repositories {
            return Err(TerminalDiscoveryStateError::RepositoryCapacity {
                maximum: self.policy.max_repositories,
            });
        }
        let repository = state
            .repositories
            .entry(repository_id.clone())
            .or_insert_with(|| RepositoryDiscovery::new(&workflow_fingerprint, &buckets));
        let cache_reused = existed
            && repository.workflow_fingerprint == workflow_fingerprint
            && repository.bucket_keys() == buckets;
        if !cache_reused {
            repository.reset_sweep(&workflow_fingerprint, &buckets);
        }
        Ok(repository.snapshot(repository_id, cache_reused))
    }

    /// Commits one fully received and decoded page atomically.
    pub fn commit_page(
        &self,
        repository_id: &RepositoryId,
        workflow_fingerprint: &str,
        bucket: &TerminalDiscoveryBucket,
        page: TerminalDiscoveryPageCommit,
    ) -> Result<TerminalDiscoveryCommitOutcome, TerminalDiscoveryStateError> {
        validate_page_shape(&page)?;
        let mut state = self.inner.lock().expect("terminal discovery state mutex");
        let repository =
            matching_repository_mut(&mut state, repository_id, workflow_fingerprint, bucket)?;
        if let Some(continuation) = &page.continuation {
            validate_continuation(repository_id, bucket, continuation)?;
        }

        let bucket_state = repository
            .buckets
            .get(bucket)
            .expect("matching bucket exists");
        let continuation_boundary = page
            .continuation
            .as_ref()
            .map(TerminalDiscoveryContinuation::boundary);
        if continuation_boundary
            .zip(page.sweep_boundary)
            .is_some_and(|(continuation, reported)| continuation != reported)
        {
            return Err(TerminalDiscoveryStateError::InvalidPage);
        }
        let observed_boundary = continuation_boundary
            .or(page.sweep_boundary)
            .or(bucket_state.sweep_boundary)
            .ok_or(TerminalDiscoveryStateError::InvalidPage)?;
        let nonadvancing = bucket_state
            .continuation
            .as_ref()
            .zip(page.continuation.as_ref())
            .is_some_and(|(previous, next)| !next.advances_from(previous))
            || bucket_state
                .sweep_boundary
                .is_some_and(|previous| previous != observed_boundary);
        if nonadvancing {
            repository
                .buckets
                .get_mut(bucket)
                .expect("matching bucket exists")
                .reset();
            repository.retain_targets(
                page.retained_targets,
                self.policy.max_retained_targets_per_repository,
            );
            return Ok(TerminalDiscoveryCommitOutcome::RestartedNonAdvancing);
        }

        let complete = {
            let bucket_state = repository
                .buckets
                .get_mut(bucket)
                .expect("matching bucket exists");
            bucket_state.sweep_boundary = Some(observed_boundary);
            bucket_state.continuation = page.continuation;
            bucket_state.complete = page.exhausted;
            bucket_state.overflow = page.overflow;
            bucket_state.failed = false;
            bucket_state.complete
        };
        repository.retain_targets(
            page.retained_targets,
            self.policy.max_retained_targets_per_repository,
        );
        Ok(if complete {
            TerminalDiscoveryCommitOutcome::Complete
        } else {
            TerminalDiscoveryCommitOutcome::Advanced
        })
    }

    /// Records a failed provider page without advancing its last committed
    /// continuation. A retry therefore starts at exactly the failed page.
    pub fn record_failed_page(
        &self,
        repository_id: &RepositoryId,
        workflow_fingerprint: &str,
        bucket: &TerminalDiscoveryBucket,
    ) -> Result<(), TerminalDiscoveryStateError> {
        let mut state = self.inner.lock().expect("terminal discovery state mutex");
        let repository =
            matching_repository_mut(&mut state, repository_id, workflow_fingerprint, bucket)?;
        let bucket = repository
            .buckets
            .get_mut(bucket)
            .expect("matching bucket exists");
        bucket.failed = true;
        bucket.complete = false;
        Ok(())
    }

    /// Retains one exact target from a webhook or local mutation. Retention is
    /// deterministic: when full, the greatest address is dropped regardless of
    /// insertion order, and overflow remains observable.
    pub fn retain_exact_target(
        &self,
        repository_id: &RepositoryId,
        target: ArtifactAddress,
    ) -> Result<bool, TerminalDiscoveryStateError> {
        let mut state = self.inner.lock().expect("terminal discovery state mutex");
        let repository = state
            .repositories
            .get_mut(repository_id)
            .ok_or_else(|| TerminalDiscoveryStateError::UnknownRepository(repository_id.clone()))?;
        Ok(repository.retain_target(target, self.policy.max_retained_targets_per_repository))
    }

    pub fn remove_exact_target(
        &self,
        repository_id: &RepositoryId,
        target: ArtifactAddress,
    ) -> bool {
        self.inner
            .lock()
            .expect("terminal discovery state mutex")
            .repositories
            .get_mut(repository_id)
            .is_some_and(|repository| repository.retained_targets.remove(&target))
    }

    /// Invalidates all sweep authority for provider anomalies or explicit local
    /// repository mutations. Retained exact targets survive the new sweep.
    pub fn invalidate_repository(&self, repository_id: &RepositoryId) -> bool {
        let mut state = self.inner.lock().expect("terminal discovery state mutex");
        let Some(repository) = state.repositories.get_mut(repository_id) else {
            return false;
        };
        for bucket in repository.buckets.values_mut() {
            bucket.reset();
        }
        true
    }

    pub fn snapshot(&self, repository_id: &RepositoryId) -> Option<TerminalDiscoverySnapshot> {
        self.inner
            .lock()
            .expect("terminal discovery state mutex")
            .repositories
            .get(repository_id)
            .map(|repository| repository.snapshot(repository_id, true))
    }

    fn validate_fingerprint(&self, fingerprint: &str) -> Result<(), TerminalDiscoveryStateError> {
        if fingerprint.is_empty() || fingerprint.len() > self.policy.max_workflow_fingerprint_bytes
        {
            return Err(TerminalDiscoveryStateError::InvalidWorkflowFingerprint {
                maximum: self.policy.max_workflow_fingerprint_bytes,
            });
        }
        Ok(())
    }
}

#[derive(Default)]
struct DiscoveryState {
    repositories: BTreeMap<RepositoryId, RepositoryDiscovery>,
}

struct RepositoryDiscovery {
    workflow_fingerprint: String,
    buckets: BTreeMap<TerminalDiscoveryBucket, BucketState>,
    retained_targets: BTreeSet<ArtifactAddress>,
    retained_overflow: bool,
}

impl RepositoryDiscovery {
    fn new(fingerprint: &str, buckets: &BTreeSet<TerminalDiscoveryBucket>) -> Self {
        Self {
            workflow_fingerprint: fingerprint.to_string(),
            buckets: buckets
                .iter()
                .cloned()
                .map(|bucket| (bucket, BucketState::default()))
                .collect(),
            retained_targets: BTreeSet::new(),
            retained_overflow: false,
        }
    }

    fn bucket_keys(&self) -> BTreeSet<TerminalDiscoveryBucket> {
        self.buckets.keys().cloned().collect()
    }

    fn reset_sweep(&mut self, fingerprint: &str, buckets: &BTreeSet<TerminalDiscoveryBucket>) {
        self.workflow_fingerprint = fingerprint.to_string();
        self.buckets = buckets
            .iter()
            .cloned()
            .map(|bucket| (bucket, BucketState::default()))
            .collect();
    }

    fn retain_targets(&mut self, targets: Vec<ArtifactAddress>, maximum: usize) {
        for target in targets {
            self.retain_target(target, maximum);
        }
    }

    fn retain_target(&mut self, target: ArtifactAddress, maximum: usize) -> bool {
        self.retained_targets.insert(target);
        if self.retained_targets.len() > maximum {
            self.retained_overflow = true;
            if let Some(greatest) = self.retained_targets.iter().next_back().copied() {
                self.retained_targets.remove(&greatest);
            }
        }
        self.retained_targets.contains(&target)
    }

    fn snapshot(
        &self,
        repository_id: &RepositoryId,
        cache_reused: bool,
    ) -> TerminalDiscoverySnapshot {
        TerminalDiscoverySnapshot {
            repository_id: repository_id.clone(),
            workflow_fingerprint: self.workflow_fingerprint.clone(),
            cache_reused,
            authoritative: self.buckets.values().all(|bucket| bucket.complete),
            retained_overflow: self.retained_overflow,
            retained_targets: self.retained_targets.iter().copied().collect(),
            buckets: self
                .buckets
                .iter()
                .map(|(key, state)| (key.clone(), state.snapshot()))
                .collect(),
        }
    }
}

#[derive(Default)]
struct BucketState {
    continuation: Option<TerminalDiscoveryContinuation>,
    sweep_boundary: Option<DateTime<Utc>>,
    complete: bool,
    overflow: bool,
    failed: bool,
}

impl BucketState {
    fn reset(&mut self) {
        *self = Self::default();
    }

    fn snapshot(&self) -> TerminalDiscoveryBucketSnapshot {
        TerminalDiscoveryBucketSnapshot {
            continuation: self.continuation.clone(),
            sweep_boundary: self.sweep_boundary,
            complete: self.complete,
            overflow: self.overflow,
            failed: self.failed,
        }
    }
}

fn normalized_labels(
    labels: CandidateLabelSelection,
) -> Result<Vec<String>, TerminalDiscoveryStateError> {
    labels
        .normalized()
        .map_err(|error| TerminalDiscoveryStateError::InvalidBucket(error.to_string()))?
        .ok_or_else(|| {
            TerminalDiscoveryStateError::InvalidBucket(
                "terminal discovery buckets must be label-filtered".to_string(),
            )
        })
}

fn matching_repository_mut<'a>(
    state: &'a mut DiscoveryState,
    repository_id: &RepositoryId,
    workflow_fingerprint: &str,
    bucket: &TerminalDiscoveryBucket,
) -> Result<&'a mut RepositoryDiscovery, TerminalDiscoveryStateError> {
    let repository = state
        .repositories
        .get_mut(repository_id)
        .ok_or_else(|| TerminalDiscoveryStateError::UnknownRepository(repository_id.clone()))?;
    if repository.workflow_fingerprint != workflow_fingerprint {
        return Err(TerminalDiscoveryStateError::StaleWorkflowFingerprint);
    }
    if !repository.buckets.contains_key(bucket) {
        return Err(TerminalDiscoveryStateError::UnknownBucket);
    }
    Ok(repository)
}

fn validate_page_shape(
    page: &TerminalDiscoveryPageCommit,
) -> Result<(), TerminalDiscoveryStateError> {
    if page.exhausted == page.overflow
        || (page.overflow && page.continuation.is_none())
        || (page.exhausted && page.continuation.is_some())
    {
        return Err(TerminalDiscoveryStateError::InvalidPage);
    }
    Ok(())
}

fn validate_continuation(
    repository_id: &RepositoryId,
    bucket: &TerminalDiscoveryBucket,
    continuation: &TerminalDiscoveryContinuation,
) -> Result<(), TerminalDiscoveryStateError> {
    let labels = continuation
        .labels()
        .normalized()
        .map_err(|error| TerminalDiscoveryStateError::InvalidBucket(error.to_string()))?;
    if continuation.repository_id() != repository_id
        || match continuation {
            TerminalDiscoveryContinuation::Issue(cursor) => {
                cursor.lifecycle != CandidateLifecycle::Terminal || cursor.after > cursor.boundary
            }
            TerminalDiscoveryContinuation::PullRequest(cursor) => {
                cursor.lifecycle != CandidateLifecycle::Terminal || cursor.after > cursor.boundary
            }
        }
        || !continuation.matches_bucket(bucket)
        || labels.as_deref() != Some(bucket.labels())
    {
        return Err(TerminalDiscoveryStateError::ContinuationScope);
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TerminalDiscoveryStateError {
    InvalidWorkflowFingerprint { maximum: usize },
    RepositoryCapacity { maximum: usize },
    BucketCapacity { requested: usize, maximum: usize },
    InvalidBucket(String),
    UnknownRepository(RepositoryId),
    UnknownBucket,
    StaleWorkflowFingerprint,
    InvalidPage,
    ContinuationScope,
}

impl fmt::Display for TerminalDiscoveryStateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidWorkflowFingerprint { maximum } => write!(
                formatter,
                "workflow fingerprint must contain 1..={maximum} bytes"
            ),
            Self::RepositoryCapacity { maximum } => {
                write!(
                    formatter,
                    "terminal discovery repository capacity {maximum} reached"
                )
            }
            Self::BucketCapacity { requested, maximum } => write!(
                formatter,
                "terminal discovery requested {requested} buckets, maximum is {maximum}"
            ),
            Self::InvalidBucket(message) => write!(formatter, "invalid terminal bucket: {message}"),
            Self::UnknownRepository(repository) => {
                write!(
                    formatter,
                    "terminal discovery repository {repository} is not initialized"
                )
            }
            Self::UnknownBucket => {
                write!(formatter, "terminal discovery bucket is not initialized")
            }
            Self::StaleWorkflowFingerprint => {
                write!(formatter, "stale terminal discovery workflow fingerprint")
            }
            Self::InvalidPage => write!(
                formatter,
                "invalid terminal discovery page shape or sweep boundary"
            ),
            Self::ContinuationScope => {
                write!(formatter, "terminal discovery continuation scope mismatch")
            }
        }
    }
}

impl Error for TerminalDiscoveryStateError {}
