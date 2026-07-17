//! Prepared, descendant-complete OS process containment.
//!
//! [`ContainmentFactory::prepare`] establishes the backend ownership boundary
//! before [`PreparedContainment::spawn`] can execute a payload. Cleanup is one
//! shared fail-closed state machine: inspection uncertainty emits
//! [`CleanupSnapshot::Blocked`] and keeps all waiters pending until recursive
//! emptiness can be proven.
//!
//! The legacy configure/spawn/attach process-group surface remains exported as
//! a temporary migration adapter. It is explicitly non-descendant-complete and
//! cannot be selected by the prepared backend model.
//!
//! This crate is the sole process-containment OS FFI boundary. Its public
//! surface is safe; platform-specific unsafe operations stay inside the
//! prepared backends and the temporary `legacy` adapter.

#[cfg(target_os = "linux")]
mod cgroup_v2;
mod command;
mod legacy;
mod model;
mod runtime;

#[cfg(target_os = "linux")]
pub use cgroup_v2::*;
pub use command::*;
pub use legacy::{
    ContainmentKind, ProcessContainment, configure_command, configure_descendant_command,
};
pub use model::*;
pub use runtime::*;

#[cfg(test)]
mod tests;
