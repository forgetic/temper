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
//! - [`daemon`] — the [`Daemon`] handle, its pure state machine, and HTTP
//!   serving.
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
pub mod config;
pub mod daemon;
pub mod engine_config;
pub mod feed;
pub mod forge_applier;
pub mod lease_applier;
pub mod mechanical;
mod webhook;
mod workflow_meta;

/// Default long-poll max-wait, applied when a worker requests none or more.
pub const DEFAULT_MAX_POLL_WAIT_MS: u64 = 30_000;
/// Re-enqueue grace window after a result is applied, suppressing immediate
/// re-feed of the just-applied job.
pub(crate) const APPLY_GRACE: Duration = Duration::from_secs(10);

pub use applier::{NoopApplier, ResultApplier, RoleRoutingApplier};
pub use config::{DaemonRunConfig, ParseOutcome, USAGE, parse};
pub use daemon::{Daemon, HintedMechanical, h1_handler, serve};
pub use engine_config::EngineConfig;
pub use feed::{
    PollBackstopConfig, RoleFeedMode, RoleFeedTarget, WorkItemJob, job_from_work_item,
    run_poll_backstop_tick, spawn_poll_backstop,
};
pub use forge_applier::ForgeApplier;
pub use lease_applier::{LeaseApplier, WallClock, system_clock};
pub use mechanical::{
    MechanicalBackstopConfig, MechanicalScope, MechanicalTrigger, run_mechanical_backstop_tick,
    spawn_mechanical_backstop,
};
// Public so out-of-crate `ResultApplier` implementations can name the job type
// the trait passes them.
pub use temper_protocol_worker::{JobArtifactSnapshot, JobContext, RepoOutcome};
pub use temper_runner::{RepositorySet, RepositoryTarget};
pub use temper_worker_registry::InFlightJob;
pub use webhook::*;
