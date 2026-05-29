//! Reusable backend-agnostic runner primitives for Harness.
//!
//! `harness-runner` contains production orchestration building blocks that sit
//! above `harness-workflow` and coordinate only through the portable
//! [`harness_forge::Forge`] interface. It intentionally contains no concrete
//! provider adapters and no agent/provider behavior.
//!
//! Three constraints guide this crate:
//!
//! 1. **Primitives are the deliverable.** Orchestration-shaped logic belongs in
//!    reusable runner primitives; fakes and real adapters plug in at the edges.
//! 2. **Forward-compatible topology.** The same worker logic must run composed
//!    in one process or split one role per process, coordinating only through
//!    the Forge.
//! 3. **Layered testing.** Narrow pure/backend tests should exercise reusable
//!    primitives before broader end-to-end scenarios reuse the same code.
//!
//! The crate now provides:
//!
//! - [`scan`], which reads fresh Forge state and turns active queue members into
//!   role-addressed [`WorkItem`]s without mutating anything.
//! - [`Agent`] and [`RoleTools`], which define the production tool boundary:
//!   agents mutate workflow state only by running authorized transitions or the
//!   idempotent pull-request creation seam through role-scoped tools.
//! - [`Worker`] and [`RoleWorker`], the tickable per-role unit that re-scans on
//!   each tick and delegates behavior to an agent.

pub mod agent;
pub mod scan;
pub mod worker;

pub use agent::{Agent, AgentError, AgentRegistry, RoleTools};
pub use scan::{scan, scan_role, ScanError, WorkItem};
pub use worker::{Progress, RoleWorker, Worker, WorkerError};
