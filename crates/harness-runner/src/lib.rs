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
//! - [`Worker`], [`RoleWorker`], and [`MechanicalWorker`]: role workers re-scan
//!   judgment queues and delegate to agents, while the mechanical worker runs
//!   reconcile → apply once per tick without spawning agents.
//!
//! The runner owns recovery coordination state. In a single-process composition
//! the command journal value and lease manager live with the worker set. In a
//! multi-process composition the journal is per-process fast-recovery state,
//! while leases remain durable in Forge metadata; a mechanical process can
//! reconstruct from Forge state, so correctness does not depend on an in-memory
//! journal surviving a restart.

pub mod agent;
pub mod scan;
pub mod worker;

pub use agent::{Agent, AgentError, AgentRegistry, RoleTools};
pub use scan::{scan, scan_role, ScanError, WorkItem};
pub use worker::{MechanicalWorker, Progress, RoleWorker, Worker, WorkerError};
