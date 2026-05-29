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
//! The fake/real boundary is explicit: this crate owns composition, drivers,
//! role-scoped tools, and worker scheduling; tests may plug in fake agents,
//! fake outside-world producers, or memory identity handles, while production
//! plugs in real agents, authenticated Forge handles, and poll/webhook drivers.
//!
//! The crate now provides:
//!
//! - [`RunnerConfig`], which keeps repository, identity, PR-create, lease, and
//!   polling settings independent of process topology.
//! - [`scan`], which reads fresh Forge state and turns active queue members into
//!   role-addressed [`WorkItem`]s without mutating anything.
//! - [`Agent`] and [`RoleTools`], which define the production tool boundary:
//!   agents mutate workflow state only by running authorized transitions or the
//!   idempotent pull-request creation seam through role-scoped tools.
//! - [`Worker`], [`RoleWorker`], and [`MechanicalWorker`]: role workers re-scan
//!   judgment queues and delegate to agents, while the mechanical worker runs
//!   reconcile → apply once per tick without spawning agents.
//! - [`FixpointDriver`] and [`Stage`]/[`InProcessStage`], which compose workers
//!   into a deterministic in-process world for layered scenarios while keeping
//!   per-role Forge identity a handle-construction concern.
//!
//! The runner owns recovery coordination state. In a single-process composition
//! the command journal value and lease manager live with the worker set. In a
//! multi-process composition the journal is per-process fast-recovery state,
//! while leases remain durable in Forge metadata; a mechanical process can
//! reconstruct from Forge state, so correctness does not depend on an in-memory
//! journal surviving a restart.

pub mod agent;
pub mod config;
pub mod driver;
pub mod scan;
pub mod stage;
pub mod worker;

pub use agent::{Agent, AgentError, AgentRegistry, RoleTools};
pub use config::{PullRequestCreateBinding, RoleBinding, RunnerConfig};
pub use driver::{DriveError, FixpointDriver, ManualClock, RunReport, WorkerRunReport};
pub use scan::{scan, scan_role, ScanError, WorkItem};
pub use stage::{
    run_scenario, run_scenario_with_budget, BoxError, InProcessStage, InProcessWorkerContext,
    InProcessWorkerFactory, Scenario, ScenarioError, ScenarioFuture, ScenarioStep, Stage,
    StageError, DEFAULT_SCENARIO_BUDGET,
};
pub use worker::{MechanicalWorker, Progress, RoleWorker, Worker, WorkerError};
