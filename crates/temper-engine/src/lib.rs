// SPDX-License-Identifier: MPL-2.0

//! Standalone async daemon transport for the Worker/Daemon wire protocol.
//!
//! The crate is organized around the result-application seam and the daemon
//! transport:
//!
//! - [`applier`] — the [`ResultApplier`] trait and its transport-default
//!   implementations ([`NoopApplier`], [`RoleRoutingApplier`]).
//! - [`forge_applier`] — the Forge-backed [`ForgeApplier`].
//! - [`lease_applier`] — the lease-gated [`LeaseApplier`] decorator and the
//!   [`WallClock`] seam.
//! - [`feed`] — translating scanned work items into daemon jobs, enriching
//!   their payloads, and the poll-backstop cadence.
//! - [`daemon`] — the [`Daemon`] handle, its pure state machine, HTTP serving,
//!   and webhook/local-change-source wake wiring.
//! - [`config`], [`mechanical`], [`webhook`] — run config, the mechanical
//!   reconciliation backstop, and webhook intake.
//! - [`engine_config`] — the per-subsystem [`EngineConfig`] bundle.
//!
//! ## Config objects
//!
//! Factories that would otherwise take a long, growing parameter list accept one
//! per-subsystem config object instead. [`EngineConfig`] is the engine's: it
//! bundles the [`DaemonRunConfig`], the forge client config, and the per-role
//! applier tokens, so the engine-service runtime stands the daemon up from a
//! single struct (built by `temper_engine_service::engine_config`). Small
//! factories with a handful of args stay as-is.

use std::time::Duration;

pub mod applier;
pub mod artifact_context;
pub mod config;
pub mod daemon;
pub mod engine_config;
pub mod feed;
pub mod forge_applier;
pub mod lease_applier;
pub mod mechanical;
pub mod pr_freshness;
pub mod trace_journal;
pub mod trace_query;
mod verdict_contract;
mod verdict_validation;
mod webhook;
mod workflow_meta;

/// Default long-poll max-wait, applied when a worker requests none or more.
pub const DEFAULT_MAX_POLL_WAIT_MS: u64 = 30_000;
/// Re-enqueue grace window after a result is applied, suppressing immediate
/// re-feed of the just-applied job.
pub(crate) const APPLY_GRACE: Duration = Duration::from_secs(10);

pub use applier::{
    ApplyOutcome, ClaimContext, ClaimOutcome, NoopApplier, ResultApplier, RoleRoutingApplier,
};
pub use artifact_context::{
    ArtifactContextBundleService, ArtifactContextError, ArtifactContextForge,
    ArtifactContextPolicy, ArtifactContextService, ConfiguredRepositoryCatalog,
    DEFAULT_RELATED_DEPTH, DEFAULT_RELATED_RESULTS, MAX_COMMENT_BYTES, MAX_FORGE_RESPONSE_BYTES,
    MAX_ITEM_BODY_BYTES, MAX_ITEM_COMMENTS, MAX_RELATED_DEPTH, MAX_RELATED_RESULTS,
    resolve_initial_artifact_context, resolve_initial_artifact_context_for_action_with_policy,
    resolve_initial_artifact_context_for_action_with_primary,
    resolve_initial_artifact_context_with_policy,
};
pub use config::{DaemonRunConfig, ParseOutcome, USAGE, parse};
pub use daemon::{CoordinatedMechanical, Daemon, h1_handler, serve};
pub use engine_config::{EngineAgentTraceConfig, EngineConfig};
pub use feed::{
    PollBackstopConfig, RoleFeedMode, RoleFeedTarget, TargetedRoleFeedResult, WorkItemJob,
    enqueue_targeted_role_work, job_from_work_item,
    recover_advanced_pull_request_assignment_from_durable, recovered_job_from_assignment,
    recovered_job_from_assignment_with_artifact_context, run_poll_backstop_tick,
    spawn_coordinated_poll_backstop, spawn_poll_backstop,
};
pub use forge_applier::ForgeApplier;
pub use lease_applier::{LeaseApplier, WallClock, system_clock};
pub use mechanical::{
    MechanicalBackstopConfig, MechanicalScope, MechanicalTrigger, run_mechanical_backstop_tick,
    spawn_coordinated_mechanical_backstop,
};
pub use pr_freshness::check_pull_request_freshness;
pub use trace_journal::{
    AgentTraceJournal, AgentTraceManifest, AgentTraceRun, AgentTraceRunStatus, AgentTraceSummary,
    AuthenticatedWorkerBinding, RetentionProtection, RetentionReport, TraceAuditRecord,
    TraceJournalConfig, TraceJournalError, TraceRecoveryFailure, TraceRecoveryReport,
};
pub use trace_query::{
    AGENT_RUNS_PATH, DEFAULT_EVENT_PAGE_LIMIT, DEFAULT_RUN_PAGE_LIMIT, MAX_EVENT_PAGE_LIMIT,
    MAX_RUN_PAGE_LIMIT, TraceEventPage, TraceExportRecordV1, TraceRunCounts, TraceRunIdentity,
    TraceRunPage, TraceRunSummary,
};
// Public so out-of-crate `ResultApplier` implementations can name the job type
// the trait passes them.
pub use temper_protocol_worker::{JobArtifactSnapshot, JobContext, RepoOutcome, WorkerAuth};
pub use temper_runner::{PullRequestMergeObserver, RepositorySet, RepositoryTarget};
pub use temper_worker_registry::{
    InFlightJob, RecoveredJob, RegistryError, WorkerPoolAuthConfig, WorkerPoolPolicies,
    WorkerPoolPolicy,
};
pub use webhook::*;
