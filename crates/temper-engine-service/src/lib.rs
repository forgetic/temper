// SPDX-License-Identifier: MPL-2.0

//! The **engine** service: the orchestrator that routes work, applies results,
//! and lands PRs.
//!
//! [`run`] is the entry point used by both the slim `temper-engine` binary and
//! the unified binary's `temper daemon --service engine`. The reusable wiring
//! ([`resolve_repositories`], [`role_feed_targets`], [`ensure_workflow_labels`],
//! [`result_applier`]) is `pub` so the unified binary's standalone (all-in-one)
//! mode composes the same engine without duplicating it.

mod adapt;
mod applier;
mod repositories;
mod runtime;

pub use adapt::{daemon_run_config, forgejo_config};
pub use applier::result_applier;
pub use repositories::{ensure_workflow_labels, resolve_repositories, role_feed_targets};
pub use runtime::{run, run_async};
