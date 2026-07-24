//! Pure queue evaluation and transition planning (Phase 5).
//!
//! This module is the deterministic, side-effect-free state-machine layer. It
//! answers three questions over already-[classified](crate::classify) artifacts:
//!
//! - **Queue matching**: does an artifact belong to a queue?
//! - **Queue activation**: should a matched queue be serviced now?
//! - **Transition planning**: may a role apply a transition to an artifact, and
//!   if so, what typed effects would it produce?
//!
//! Nothing here touches a Forge backend. A [`Planner`] borrows a
//! [`ValidatedWorkflow`] (never a raw spec), reads a classified artifact's kind,
//! labels, and states, and returns either a [`TransitionPlan`] of typed
//! [`WorkflowEffect`]s plus [`Postcondition`]s, or a [`PlanError`] collecting
//! every [`PlanDiagnostic`]. Applying the plan against a Forge backend is a
//! later phase (see `docs/how-to/implement-workflow-layer-in-phases.md`).
//!
//! Queue matching and activation also work against the compiled
//! [`QueueManifest`](crate::compile::QueueManifest) through the [`QueueQuery`]
//! trait, so the same logic serves the validated model and a compiled runtime
//! table.

mod conditions;
mod dependency;
mod diagnostic;
mod planner;
mod queue;
mod signals;
mod state;
mod types;

pub use dependency::{DependencyReadFailure, DependencyStatus, MechanicalPlan};
pub use diagnostic::{PlanDiagnostic, PlanError};
pub use planner::Planner;
pub use queue::{
    QueueMember, QueueQuery, matches_queue, matches_queue_cheap, matches_queue_condition,
    matches_queue_with, queue_active,
};
pub use signals::{CiState, CiStatus, CiTerminalEvidence, GateSignals, ReviewStatus, SignalNeeds};
pub use types::{Postcondition, TransitionPlan, WorkflowEffect};
