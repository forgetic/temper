// SPDX-License-Identifier: MPL-2.0

//! Deterministic in-memory worker scheduling registry for the Temper daemon.
//!
//! The registry is a soft scheduling hint: it tracks worker capabilities,
//! health, and local in-flight capacity, but Forge leases/CAS remain the source
//! of truth for work ownership.

mod auth;
pub mod daemon_core;
pub mod dispatch;
mod registry;

pub use auth::WorkerPoolAuthConfig;
pub use daemon_core::{DaemonCore, HeartbeatRecovery, InFlightJob, RecoveredJob, RoleSaturation};
pub use dispatch::{Assignment, DispatchCoordinator, WorkItem};
pub use registry::{
    RegistrationError, RegistryError, WorkerPoolPolicies, WorkerPoolPolicy, WorkerRegistry,
    WorkerSnapshot,
};

#[cfg(test)]
pub(crate) mod test_support;
