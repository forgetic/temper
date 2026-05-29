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
//! The first primitive is [`scan`], which reads fresh Forge state and turns
//! active queue members into role-addressed [`WorkItem`]s without mutating
//! anything.

pub mod scan;

pub use scan::{scan, scan_role, ScanError, WorkItem};
