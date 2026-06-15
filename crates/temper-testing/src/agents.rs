//! Deterministic fake role behavior for reference-delivery runner tests.
//!
//! These fakes are behavior-only test adapters. They mutate workflow state only
//! through `RoleTools`, exactly like a real agent adapter would, and keep all
//! orchestration in runner workers/stages.

mod architect;
mod engineer;
mod reviewers;
mod support;

use temper_forge::Forge;
use temper_runner::{Agent, AgentRegistry};
use temper_workflow::RoleId;

pub use architect::{ARCHITECT_PLAN_BEGIN, BasicArchitect, ClosingArchitect, FakeArchitect};
pub use engineer::{BasicEngineer, FakeEngineer};
pub(crate) use engineer::{EnginePrep, basic_engineer_service, engineer_service};
pub use reviewers::{FakeHuman, FakeOwner, FakeReviewer, RequestChangesThenApproveReviewer};

pub fn fake_registry<F>() -> AgentRegistry<F>
where
    F: Forge + ?Sized + 'static,
{
    fake_registry_with(FakeArchitect, FakeReviewer)
}

/// Registry for the **basic-delivery** workflow.
///
/// basic-delivery binds only the queue-subscribing roles `architect` and
/// `engineer`; `mechanical` is queue-less (serviced by the mechanical worker,
/// not a role worker) and there is no reviewer/owner/human. Registering only the
/// two role agents keeps this set aligned with the fixture's role shape.
pub fn basic_fake_registry<F>() -> AgentRegistry<F>
where
    F: Forge + ?Sized + 'static,
{
    basic_fake_registry_with(BasicEngineer)
}

/// Registry for the basic-delivery workflow with a caller-supplied engineer.
///
/// The Forgejo worker path swaps in a Forgejo-backed engineer (real PR head + CI
/// sentinel) while keeping the backend-neutral [`BasicArchitect`]; the
/// filesystem/memory path uses the plain [`BasicEngineer`].
pub fn basic_fake_registry_with<F, E>(engineer: E) -> AgentRegistry<F>
where
    F: Forge + ?Sized + 'static,
    E: Agent<F> + 'static,
{
    let mut registry = AgentRegistry::new();
    registry.register(RoleId::new("architect"), BasicArchitect);
    registry.register(RoleId::new("engineer"), engineer);
    registry
}

pub fn fake_registry_with<F, A, R>(architect: A, reviewer: R) -> AgentRegistry<F>
where
    F: Forge + ?Sized + 'static,
    A: Agent<F> + 'static,
    R: Agent<F> + 'static,
{
    let mut registry = AgentRegistry::new();
    registry.register(RoleId::new("architect"), architect);
    registry.register(RoleId::new("engineer"), FakeEngineer);
    registry.register(RoleId::new("reviewer"), reviewer);
    registry.register(RoleId::new("owner"), FakeOwner);
    registry.register(RoleId::new("human"), FakeHuman);
    registry
}
