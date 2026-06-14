//! Lease acquisition, heartbeat, and release (Phase 7).
//!
//! A claim is a *lease*, not permanent ownership: it records who holds an
//! artifact and until when, so a crashed worker's claim can be detected and
//! recovered instead of blocking the artifact forever. The lease record itself
//! lives in the artifact's [metadata block](crate::metadata::Lease); this module
//! adds the rules for granting, extending, and clearing one.
//!
//! The module is split into a pure layer and a runtime layer, matching the rest
//! of the crate:
//!
//! - [`LeasePlanner`] is deterministic and side-effect-free. Given the current
//!   lease (if any), a worker identity, and the current time, it decides the
//!   next lease or a [`LeaseConflict`]. It never touches a backend.
//! - [`LeaseManager`] applies those decisions to a [`Forge`] by rewriting the
//!   target artifact's metadata block, following the same load-fresh-then-write
//!   discipline as [`crate::execute::Executor`].
//!
//! Expiry is governed by a [`LeasePolicy`] time-to-live: every grant or
//! heartbeat sets `expires_at` to `now + ttl`. Recovery of *expired* leases is
//! the reconciler's job (see [`crate::reconcile`]); this module only mints and
//! refreshes live leases and refuses to steal one that is still held by another
//! worker.

use crate::ArtifactSource;
use crate::ids::RoleId;
use crate::metadata::{Lease, WorkflowMetadata, parse_metadata_block, replace_metadata_block};
use chrono::{DateTime, Duration, Utc};
use std::error::Error;
use std::fmt;
use temper_forge::{
    Forge, ForgeError, IssueId, PullRequestId, RepositoryId, UpdateIssue, UpdatePullRequest,
    Version,
};

/// How long a freshly granted or refreshed lease lives.
///
/// Every [`LeasePlanner`] grant or heartbeat sets `expires_at = now + ttl`. A
/// shorter ttl reclaims crashed work sooner but needs more frequent heartbeats;
/// a longer ttl tolerates slower workers but delays recovery.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeasePolicy {
    ttl: Duration,
}

impl LeasePolicy {
    /// Creates a policy whose leases live for `ttl` after each heartbeat.
    pub fn new(ttl: Duration) -> Self {
        Self { ttl }
    }

    /// Returns the configured time-to-live.
    pub fn ttl(&self) -> Duration {
        self.ttl
    }

    /// Returns the expiry instant for a lease granted or refreshed at `from`.
    fn expiry(&self, from: DateTime<Utc>) -> DateTime<Utc> {
        from + self.ttl
    }
}

/// Why a lease operation could not be planned.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LeaseConflict {
    /// The artifact is held by a different worker whose lease has not expired.
    HeldByOther {
        /// Worker that currently holds the unexpired lease.
        holder: String,
        /// Worker that attempted the operation.
        worker: String,
    },
    /// A heartbeat targeted an artifact with no active lease for the worker.
    NotHeld {
        /// Worker that attempted to heartbeat.
        worker: String,
    },
}

impl fmt::Display for LeaseConflict {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LeaseConflict::HeldByOther { holder, worker } => write!(
                formatter,
                "worker `{worker}` cannot take a lease held by `{holder}`"
            ),
            LeaseConflict::NotHeld { worker } => {
                write!(formatter, "worker `{worker}` holds no lease to update")
            }
        }
    }
}

impl Error for LeaseConflict {}

/// Pure decision layer for lease operations.
///
/// Bound to a [`LeasePolicy`] for expiry, it computes the next lease (or a
/// [`LeaseConflict`]) from the current lease, a worker identity, and the current
/// time. It never reads or writes a backend, so it is fully deterministic and
/// trivially testable.
#[derive(Clone, Copy, Debug)]
pub struct LeasePlanner {
    policy: LeasePolicy,
}

impl LeasePlanner {
    /// Creates a planner bound to a lease policy.
    pub fn new(policy: LeasePolicy) -> Self {
        Self { policy }
    }

    /// Returns the lease policy.
    pub fn policy(&self) -> LeasePolicy {
        self.policy
    }

    /// Plans acquiring (or refreshing) a lease for `worker` in `role`.
    ///
    /// Grants when no lease exists or the existing lease has expired (the
    /// expired holder is reclaimed). Refreshes in place when `worker` already
    /// holds an unexpired lease, preserving the original `claimed_at` so the
    /// claim's start time survives heartbeats. Fails with
    /// [`LeaseConflict::HeldByOther`] when a *different* worker holds an
    /// unexpired lease.
    pub fn acquire(
        &self,
        current: Option<&Lease>,
        role: RoleId,
        worker: impl Into<String>,
        now: DateTime<Utc>,
    ) -> Result<Lease, LeaseConflict> {
        let worker = worker.into();
        match current {
            Some(held) if !held.is_expired(now) && held.worker != worker => {
                Err(LeaseConflict::HeldByOther {
                    holder: held.worker.clone(),
                    worker,
                })
            }
            Some(held) if !held.is_expired(now) => Ok(Lease {
                role,
                worker,
                claimed_at: held.claimed_at,
                heartbeat_at: now,
                expires_at: self.policy.expiry(now),
            }),
            _ => Ok(Lease {
                role,
                worker,
                claimed_at: now,
                heartbeat_at: now,
                expires_at: self.policy.expiry(now),
            }),
        }
    }

    /// Plans a heartbeat that extends `worker`'s lease.
    ///
    /// The holding worker may extend even a just-expired lease (the worker is
    /// demonstrably alive); the reconciler, not a peer, is the authority that
    /// reclaims an expired lease, and it does so by clearing the metadata. Fails
    /// with [`LeaseConflict::NotHeld`] when there is no lease and
    /// [`LeaseConflict::HeldByOther`] when a different worker holds it.
    pub fn heartbeat(
        &self,
        current: Option<&Lease>,
        worker: &str,
        now: DateTime<Utc>,
    ) -> Result<Lease, LeaseConflict> {
        match current {
            None => Err(LeaseConflict::NotHeld {
                worker: worker.to_string(),
            }),
            Some(held) if held.worker != worker => Err(LeaseConflict::HeldByOther {
                holder: held.worker.clone(),
                worker: worker.to_string(),
            }),
            Some(held) => Ok(Lease {
                role: held.role.clone(),
                worker: held.worker.clone(),
                claimed_at: held.claimed_at,
                heartbeat_at: now,
                expires_at: self.policy.expiry(now),
            }),
        }
    }

    /// Plans releasing `worker`'s lease.
    ///
    /// Returns `Ok(None)` (the artifact has no lease afterwards) when the worker
    /// holds the lease or when there is already no lease, so release is
    /// idempotent and safe to retry after a crash. Fails with
    /// [`LeaseConflict::HeldByOther`] when a different worker holds it, so a peer
    /// cannot drop someone else's claim — that is the reconciler's role.
    pub fn release(
        &self,
        current: Option<&Lease>,
        worker: &str,
    ) -> Result<Option<Lease>, LeaseConflict> {
        match current {
            None => Ok(None),
            Some(held) if held.worker == worker => Ok(None),
            Some(held) => Err(LeaseConflict::HeldByOther {
                holder: held.worker.clone(),
                worker: worker.to_string(),
            }),
        }
    }
}

/// Why a lease operation against a [`Forge`] failed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LeaseError {
    /// The target artifact does not exist in the backend.
    TargetMissing { target: ArtifactSource },
    /// The artifact body held a metadata block that could not be parsed.
    MalformedMetadata { reason: String },
    /// The lease decision was rejected (held by another worker, or not held).
    Conflict(LeaseConflict),
    /// A conditional metadata write lost a race: the artifact's version changed
    /// between the load and the write, so another worker (or the reconciler)
    /// mutated it first. The planner's decision was made against a now-stale
    /// snapshot; the caller should re-load and re-plan. This is what closes the
    /// lease-acquisition lost-update window (see ADR 0013): two acquirers over
    /// the same "no lease" snapshot cannot both win, because the second write
    /// fails its compare-and-swap.
    Contended { target: ArtifactSource },
    /// A backend operation failed.
    Backend { message: String },
}

impl fmt::Display for LeaseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LeaseError::TargetMissing { target } => {
                write!(formatter, "target artifact {target:?} does not exist")
            }
            LeaseError::MalformedMetadata { reason } => {
                write!(formatter, "could not parse workflow metadata: {reason}")
            }
            LeaseError::Conflict(conflict) => write!(formatter, "lease conflict: {conflict}"),
            LeaseError::Contended { target } => write!(
                formatter,
                "lease write for {target:?} lost a race: the artifact changed since it was read"
            ),
            LeaseError::Backend { message } => write!(formatter, "backend error: {message}"),
        }
    }
}

impl Error for LeaseError {}

impl From<ForgeError> for LeaseError {
    fn from(error: ForgeError) -> Self {
        LeaseError::Backend {
            message: error.to_string(),
        }
    }
}

impl From<LeaseConflict> for LeaseError {
    fn from(conflict: LeaseConflict) -> Self {
        LeaseError::Conflict(conflict)
    }
}

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
