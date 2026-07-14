// SPDX-License-Identifier: MPL-2.0

//! The **engine** service: the orchestrator that routes work, applies results,
//! and lands PRs.
//!
//! [`run`] is the entry point used by both the slim `temper-engine` binary and
//! the unified binary's `temper serve engine` path. The reusable wiring
//! ([`resolve_repositories`], [`role_feed_targets`], [`ensure_workflow_labels`],
//! [`result_applier`]) is `pub` so the unified binary's standalone (all-in-one)
//! mode composes the same engine without duplicating it.
//!
//! ## Config objects
//!
//! [`engine_config`] adapts a [`Resolved`](temper_config::Resolved) deployment
//! into the engine tier's per-subsystem [`EngineConfig`](temper_engine::EngineConfig)
//! bundle (daemon config + forge config + per-role applier tokens). The runtime
//! then builds the daemon from that one struct. Per the per-subsystem
//! config-object rule, factories with long parameter lists take a config object;
//! the smaller `forgejo_config`/`daemon_run_config` adapters stay as-is and feed
//! into it.

mod adapt;
mod applier;
mod repositories;
mod runtime;
mod startup_recovery;

pub use adapt::{
    daemon_run_config, engine_config, forgejo_config, worker_pool_auth_config, workflow_role_limits,
};
pub use applier::result_applier;
pub use repositories::{ensure_workflow_labels, resolve_repositories, role_feed_targets};
pub use runtime::{attach_trace_query, run, run_async, start_trace_journal};
pub use startup_recovery::{RecoveredClaim, converge_startup_orphans, stage_startup_assignments};
