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
//!   next lease or a [`LeaseConflict`]. It never touches a backend. It lives in
//!   the [`planner`] submodule.
//! - [`LeaseManager`] applies those decisions to a [`Forge`](temper_forge::Forge)
//!   by rewriting the target artifact's metadata block, following the same
//!   load-fresh-then-write discipline as [`crate::execute::Executor`]. It lives
//!   in the [`manager`] submodule.
//!
//! Expiry is governed by a [`LeasePolicy`] time-to-live: every grant or
//! heartbeat sets `expires_at` to `now + ttl`. Recovery of *expired* leases is
//! the reconciler's job (see [`crate::reconcile`]); this module only mints and
//! refreshes live leases and refuses to steal one that is still held by another
//! worker.
//!
//! Lease acquisition is a compare-and-swap built on the portable
//! optimistic-concurrency primitive in `temper-forge` (the `Version` token and
//! `expected_version` precondition; see ADR 0013).

mod manager;
mod planner;

pub use manager::{AssignmentClaimRequest, AssignmentMutation, LeaseManager, PreparedAcquire};
pub use planner::{LeaseConflict, LeasePlanner, LeasePolicy};

use crate::ArtifactSource;
use std::error::Error;
use std::fmt;
use temper_forge::ForgeError;

/// Definitive evidence that a recovered attempt no longer owns its durable claim.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RecoveredOwnershipLossReason {
    /// The repository or artifact supplied by the recovered job no longer exists.
    TargetRemoved,
    /// Fresh metadata contains no durable assignment.
    AssignmentAbsent,
    /// Fresh metadata names a different exact assignment.
    AssignmentReplaced,
    /// The exact assignment remains, but its lease is absent.
    LeaseAbsent,
    /// The exact assignment remains, but its lease belongs to another identity.
    LeaseReplaced,
    /// Fresh metadata cannot represent a valid assignment/lease claim.
    MalformedClaim { reason: String },
}

impl fmt::Display for RecoveredOwnershipLossReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TargetRemoved => formatter.write_str("assignment target was removed"),
            Self::AssignmentAbsent => formatter.write_str("durable assignment is absent"),
            Self::AssignmentReplaced => {
                formatter.write_str("durable assignment identity was replaced")
            }
            Self::LeaseAbsent => formatter.write_str("durable assignment lease is absent"),
            Self::LeaseReplaced => formatter.write_str("durable assignment lease was replaced"),
            Self::MalformedClaim { reason } => {
                write!(formatter, "malformed durable claim: {reason}")
            }
        }
    }
}

/// Typed ownership decision returned by a recovered-assignment heartbeat.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RecoveredHeartbeatOutcome {
    /// The exact assignment and lease were refreshed successfully.
    Owned,
    /// Ownership could not be checked or refreshed due to a temporary failure.
    TransientlyUnavailable { reason: String },
    /// Fresh durable state proves that this attempt no longer owns the claim.
    OwnershipLost {
        reason: RecoveredOwnershipLossReason,
    },
}

/// Why a lease operation against a [`Forge`](temper_forge::Forge) failed.
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
    /// Fresh metadata already names a different durable assignment.
    AssignmentConflict { job_id: String },
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
            LeaseError::AssignmentConflict { job_id } => {
                write!(formatter, "artifact is durably assigned to job `{job_id}`")
            }
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
