//! Backend-applying lease manager.
//!
//! This is the runtime half of the lease module: it applies the pure
//! [`LeasePlanner`](super::LeasePlanner) decisions to a [`Forge`] by rewriting
//! the target artifact's metadata block, following the same
//! load-fresh-then-write discipline as [`crate::execute::Executor`]. Every write
//! is conditional on the version captured at load time (ADR 0013), so a racing
//! mutation surfaces as [`LeaseError::Contended`] rather than clobbering.

use super::{LeaseError, LeasePlanner, LeasePolicy};
use crate::ArtifactSource;
use crate::ids::RoleId;
use crate::metadata::{Lease, WorkflowMetadata, parse_metadata_block, replace_metadata_block};
use chrono::{DateTime, Utc};
use temper_forge::{
    Forge, ForgeError, IssueId, PullRequestId, RepositoryId, UpdateIssue, UpdatePullRequest,
    Version,
};

/// A loaded artifact's mutable handle, its current metadata, and the
/// optimistic-concurrency version captured at load time.
///
/// The captured [`Version`] is what makes the eventual write a compare-and-swap:
/// the manager writes the new lease conditionally on this version, so a peer
/// that mutated the artifact in between causes the write to fail rather than
/// clobber.
enum LoadedLease {
    Issue {
        id: IssueId,
        body: String,
        metadata: WorkflowMetadata,
        version: Version,
    },
    PullRequest {
        id: PullRequestId,
        body: String,
        metadata: WorkflowMetadata,
        version: Version,
    },
}

impl LoadedLease {
    fn metadata(&self) -> &WorkflowMetadata {
        match self {
            LoadedLease::Issue { metadata, .. } | LoadedLease::PullRequest { metadata, .. } => {
                metadata
            }
        }
    }

    /// The version token captured when the artifact was loaded.
    fn version(&self) -> Version {
        match self {
            LoadedLease::Issue { version, .. } | LoadedLease::PullRequest { version, .. } => {
                *version
            }
        }
    }
}

/// A planned lease acquisition captured against a specific load-time version.
///
/// Produced by [`LeaseManager::prepare_acquire`] and applied with
/// [`LeaseManager::commit`]. Splitting load+plan from the write lets a caller —
/// or a deterministic test — interleave two acquirers explicitly: both can
/// `prepare_acquire` against the same "no lease" snapshot, but only the first
/// `commit` wins; the second fails with [`LeaseError::Contended`] because its
/// captured version is now stale. [`LeaseManager::acquire`] is the common-case
/// convenience that chains the two.
pub struct PreparedAcquire {
    target: ArtifactSource,
    loaded: LoadedLease,
    lease: Lease,
}

impl PreparedAcquire {
    /// The lease that will be written if this acquisition wins the race.
    pub fn lease(&self) -> &Lease {
        &self.lease
    }

    /// The optimistic-concurrency version captured at load time. The commit is
    /// conditional on this token.
    pub fn version(&self) -> Version {
        self.loaded.version()
    }
}

/// Applies lease decisions to a [`Forge`] by rewriting metadata blocks.
///
/// Bound to a backend and a [`LeasePlanner`]. Every operation loads fresh state,
/// plans against it, and writes the new lease into the artifact's metadata block
/// through a single *conditional* body update, so a single manager is reusable
/// across many operations and never trusts a stale read.
///
/// The write is conditional on the version captured at load time (see ADR 0013):
/// the manager passes that version as the update's `expected_version`, so the
/// backend rejects the write with [`ForgeError::Conflict`] — surfaced as
/// [`LeaseError::Contended`] — if the artifact changed in between. `acquire`,
/// `heartbeat`, and `release` are all conditional, because each one follows the
/// load-fresh-then-write discipline and each benefits from atomicity: a peer
/// that stole an expired lease, the reconciler clearing one, or a racing
/// acquirer all move the version, so a stale write is refused rather than
/// silently overwriting. `acquire` is the operation whose lost-update window
/// this primarily closes.
pub struct LeaseManager<'a, F: Forge + ?Sized> {
    forge: &'a F,
    planner: LeasePlanner,
}

impl<'a, F: Forge + ?Sized> LeaseManager<'a, F> {
    /// Creates a lease manager bound to a backend and policy.
    pub fn new(forge: &'a F, policy: LeasePolicy) -> Self {
        Self {
            forge,
            planner: LeasePlanner::new(policy),
        }
    }

    /// Returns the lease planner this manager applies.
    pub fn planner(&self) -> &LeasePlanner {
        &self.planner
    }

    /// Acquires or refreshes the lease on `target` for `worker` in `role`.
    ///
    /// Loads fresh state, plans the grant, and writes it back conditionally on
    /// the load-time version. Two acquirers that both observe "no lease" cannot
    /// both succeed: the first write advances the version, so the second
    /// observes [`LeaseError::Contended`]. This is the convenience that chains
    /// [`LeaseManager::prepare_acquire`] and [`LeaseManager::commit`].
    pub async fn acquire(
        &self,
        repo_id: &RepositoryId,
        target: ArtifactSource,
        role: RoleId,
        worker: impl Into<String>,
        now: DateTime<Utc>,
    ) -> Result<Lease, LeaseError> {
        let prepared = self
            .prepare_acquire(repo_id, target, role, worker, now)
            .await?;
        self.commit(prepared).await
    }

    /// Loads fresh state and plans a lease grant without writing it.
    ///
    /// Captures the artifact's version at load time inside the returned
    /// [`PreparedAcquire`], so a later [`commit`](LeaseManager::commit) is a
    /// compare-and-swap against that exact snapshot. Returns a
    /// [`LeaseError::Conflict`] when the planner refuses the grant (a live lease
    /// held by another worker).
    pub async fn prepare_acquire(
        &self,
        repo_id: &RepositoryId,
        target: ArtifactSource,
        role: RoleId,
        worker: impl Into<String>,
        now: DateTime<Utc>,
    ) -> Result<PreparedAcquire, LeaseError> {
        let loaded = self.load(repo_id, target).await?;
        let lease = self
            .planner
            .acquire(loaded.metadata().lease.as_ref(), role, worker, now)?;
        Ok(PreparedAcquire {
            target,
            loaded,
            lease,
        })
    }

    /// Writes a prepared lease grant conditionally on its captured version.
    ///
    /// Succeeds only if the artifact has not changed since
    /// [`prepare_acquire`](LeaseManager::prepare_acquire) loaded it; otherwise
    /// fails with [`LeaseError::Contended`] and the caller should re-prepare
    /// against fresh state.
    pub async fn commit(&self, prepared: PreparedAcquire) -> Result<Lease, LeaseError> {
        let PreparedAcquire {
            target,
            loaded,
            lease,
        } = prepared;
        self.write_lease(&loaded, Some(lease.clone()), target)
            .await?;
        Ok(lease)
    }

    /// Extends `worker`'s lease on `target` with a fresh heartbeat and expiry.
    pub async fn heartbeat(
        &self,
        repo_id: &RepositoryId,
        target: ArtifactSource,
        worker: &str,
        now: DateTime<Utc>,
    ) -> Result<Lease, LeaseError> {
        let loaded = self.load(repo_id, target).await?;
        let lease = self
            .planner
            .heartbeat(loaded.metadata().lease.as_ref(), worker, now)?;
        self.write_lease(&loaded, Some(lease.clone()), target)
            .await?;
        Ok(lease)
    }

    /// Releases `worker`'s lease on `target`, clearing it from the metadata.
    pub async fn release(
        &self,
        repo_id: &RepositoryId,
        target: ArtifactSource,
        worker: &str,
    ) -> Result<(), LeaseError> {
        let loaded = self.load(repo_id, target).await?;
        if loaded.metadata().lease.is_none() {
            return Ok(());
        }
        let lease = self
            .planner
            .release(loaded.metadata().lease.as_ref(), worker)?;
        self.write_lease(&loaded, lease, target).await
    }

    /// Forcibly clears any lease on `target`, regardless of which worker holds
    /// it.
    ///
    /// This is the reconciler's authority path for an expired lease: unlike
    /// [`release`](LeaseManager::release) — which refuses a peer's lease so a
    /// live worker cannot be evicted by another — `clear` drops whatever lease
    /// is present, because the recovery layer has already judged the holder
    /// gone. It loads fresh state and writes conditionally on the captured
    /// version (compare-and-swap), so a racing mutation surfaces as
    /// [`LeaseError::Contended`] rather than clobbering. Clearing an artifact
    /// that already has no lease is a no-op, so re-running a recovery report
    /// never thrashes the metadata or fails on an already-requeued artifact.
    pub async fn clear(
        &self,
        repo_id: &RepositoryId,
        target: ArtifactSource,
    ) -> Result<(), LeaseError> {
        let loaded = self.load(repo_id, target).await?;
        if loaded.metadata().lease.is_none() {
            return Ok(());
        }
        self.write_lease(&loaded, None, target).await
    }

    /// Loads the artifact, its body, and parsed metadata from fresh state.
    async fn load(
        &self,
        repo_id: &RepositoryId,
        target: ArtifactSource,
    ) -> Result<LoadedLease, LeaseError> {
        match target {
            ArtifactSource::Issue { number } => {
                let issue = self
                    .forge
                    .get_issue_by_number(repo_id, number)
                    .await?
                    .ok_or(LeaseError::TargetMissing { target })?;
                let metadata = parse_metadata(&issue.body)?;
                Ok(LoadedLease::Issue {
                    id: issue.id,
                    body: issue.body,
                    metadata,
                    version: issue.version,
                })
            }
            ArtifactSource::PullRequest { number } => {
                let pull_request = self
                    .forge
                    .get_pull_request_by_number(repo_id, number)
                    .await?
                    .ok_or(LeaseError::TargetMissing { target })?;
                let metadata = parse_metadata(&pull_request.body)?;
                Ok(LoadedLease::PullRequest {
                    id: pull_request.id,
                    body: pull_request.body,
                    metadata,
                    version: pull_request.version,
                })
            }
        }
    }

    /// Writes `lease` into the artifact's metadata block via a *conditional*
    /// body update, keyed on the version captured when `loaded` was read.
    ///
    /// A [`ForgeError::Conflict`] from the backend means the artifact changed
    /// since the load; it is surfaced as [`LeaseError::Contended`] for `target`
    /// so callers can tell a lost compare-and-swap apart from other backend
    /// failures.
    async fn write_lease(
        &self,
        loaded: &LoadedLease,
        lease: Option<Lease>,
        target: ArtifactSource,
    ) -> Result<(), LeaseError> {
        let (body, mut metadata) = match loaded {
            LoadedLease::Issue { body, metadata, .. }
            | LoadedLease::PullRequest { body, metadata, .. } => (body, metadata.clone()),
        };
        metadata.lease = lease;
        let new_body = replace_metadata_block(body, &metadata).map_err(|error| {
            LeaseError::MalformedMetadata {
                reason: error.to_string(),
            }
        })?;
        let expected_version = Some(loaded.version());
        let result = match loaded {
            LoadedLease::Issue { id, .. } => self
                .forge
                .update_issue(
                    id,
                    UpdateIssue {
                        body: Some(new_body),
                        expected_version,
                        ..UpdateIssue::default()
                    },
                )
                .await
                .map(|_| ()),
            LoadedLease::PullRequest { id, .. } => self
                .forge
                .update_pull_request(
                    id,
                    UpdatePullRequest {
                        body: Some(new_body),
                        expected_version,
                        ..UpdatePullRequest::default()
                    },
                )
                .await
                .map(|_| ()),
        };
        result.map_err(|error| match error {
            ForgeError::Conflict(_) => LeaseError::Contended { target },
            other => LeaseError::Backend {
                message: other.to_string(),
            },
        })
    }
}

/// Parses metadata from a body, mapping a malformed block to a lease error.
fn parse_metadata(body: &str) -> Result<WorkflowMetadata, LeaseError> {
    Ok(parse_metadata_block(body)
        .map_err(|error| LeaseError::MalformedMetadata {
            reason: error.to_string(),
        })?
        .unwrap_or_default())
}
