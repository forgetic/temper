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

mod capture;
#[cfg(target_os = "linux")]
mod cgroup_v2;
mod command;
#[cfg(any(unix, windows))]
mod immediate_exit;
mod legacy;
#[cfg(target_os = "linux")]
mod linux;
mod model;
mod platform;
mod runtime;
#[cfg(windows)]
mod windows;

pub use capture::*;
#[cfg(target_os = "linux")]
pub use cgroup_v2::*;
pub use command::*;
#[cfg(any(unix, windows))]
pub use immediate_exit::terminate_current_process_immediately;
pub use legacy::{
    ContainmentKind, ProcessContainment, configure_command, configure_descendant_command,
};
#[cfg(target_os = "linux")]
pub use linux::*;
pub use model::*;
pub use platform::*;
pub use runtime::*;
#[cfg(windows)]
pub use windows::*;

#[cfg(test)]
mod tests;
