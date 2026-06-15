//! Pure lease policy, planner, and conflict types.
//!
//! This is the deterministic, side-effect-free half of the lease module: given a
//! current lease, a worker identity, and the current time, it decides the next
//! lease or a [`LeaseConflict`]. It never touches a backend. The runtime layer
//! that applies these decisions lives in [`manager`](super::manager).

use crate::ids::RoleId;
use crate::metadata::Lease;
use chrono::{DateTime, Duration, Utc};
use std::error::Error;
use std::fmt;

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
