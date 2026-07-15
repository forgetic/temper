// SPDX-License-Identifier: MPL-2.0

//! Serialization-only DTOs for the Temper Worker/Daemon Wire Protocol v1.
//!
//! This crate intentionally has no Temper runtime dependencies. It provides the
//! stable JSON shapes that workers and daemons can share without coupling Smith
//! or other worker implementations to Temper runner, workflow, backend, daemon,
//! deployment, or Forge crates.
//!
//! # Multi-repo co-development jobs (ADR 0023)
//!
//! The protocol is still `v1` (pre-1.0 alpha): we revise it in place rather than
//! bumping the version, even for wire-incompatible changes.
//!
//! A coding job's checkout target is a [`WorkspaceManifest`] -- an ordered set
//! of repositories laid out as siblings so their inter-repo path dependencies
//! resolve. The worker assembles them all, runs one agent turn over the combined
//! root, and reports one [`RepoOutcome`] per *writable* repository that produced
//! a diff. The daemon opens one pull request per outcome. A single-repo job is
//! the degenerate manifest of one writable primary repo.

mod activity;
mod assignment;
mod auth;
mod context;
mod job;
mod lifecycle;
mod message;
mod result;
mod workspace;

pub use activity::{WorkerActivityAcknowledgement, WorkerActivityBatch};
pub use assignment::{Artifact, Assign, Capability, Capacity, Poll, Register};
pub use auth::{WORKER_AUTHORIZATION_HEADER, WORKER_AUTHORIZATION_SCHEME, WorkerAuth};
pub use context::{ContextOutcome, ContextResponse, FetchContext};
pub use job::{
    JobArtifactSnapshot, JobContext, PullRequestFreshness, PullRequestFreshnessResponse,
    PullRequestFreshnessStatus,
};
pub use lifecycle::{
    ErrorCode, Heartbeat, HeartbeatState, JobHeartbeat, LeaseAck, LeaseAckDisposition,
    ProtocolError, Release, ReleaseDisposition,
};
pub use message::{WORKER_PROTOCOL_VERSION, WorkerProtocolMessage};
pub use result::{Branch, Failure, FailureClass, JobChild, JobResult, RepoOutcome, ResultStatus};
pub use temper_protocol_context::{
    ARTIFACT_CONTEXT_VERSION, ArtifactContextBundle, ArtifactContextDiagnostic,
    ArtifactContextDiagnosticCode, ArtifactContextTruncation, ArtifactIndexEntry,
    ArtifactReference, ArtifactRelation, ArtifactRelationType, ArtifactRepository,
    ArtifactSnapshot, ArtifactSummary, ArtifactType, ForgeContextErrorCode, ForgeContextOperation,
    ForgeContextResult, ForgeGetItemOperation, ForgeGetItemResult, ForgeItemComment,
    ForgeListRelatedOperation, ForgeListRelatedResult, ForgeRelatedEdge, ForgeRelationType,
    W3cTraceContext, W3cTraceContextError,
};
pub use workspace::{RepoAccess, WorkspaceManifest, WorkspaceRepo};

#[cfg(test)]
mod tests;
