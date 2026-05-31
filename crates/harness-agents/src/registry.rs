//! Building an [`AgentRegistry`] of real, LLM-backed role agents.
//!
//! This mirrors `harness-testing`'s `fake_registry`/`fake_registry_with`: one
//! place that maps every workflow [`RoleId`] to its agent, constructing the
//! **shared** [`ProviderConfig`] once and handing a clone to each role. Selecting
//! real agents over fakes is then just choosing this builder for the registry,
//! exactly as the worker's `--agents real|fake` flag does (Phase B2).
//!
//! The behavior variants the reference-delivery scenarios need are exposed
//! through [`RealRegistryConfig`] — the architect's closing behavior and the
//! reviewer's request-changes-then-approve path — matching the fakes' variant
//! seam one-to-one. The engineer's backend-specific PR-head/CI side effects are
//! carried by an injected [`EngineerPrep`] (default [`NoPrep`]), so this crate
//! stays backend-agnostic.

use std::sync::Arc;

use harness_forge::Forge;
use harness_runner::{Agent, AgentRegistry};
use harness_workflow::RoleId;

use crate::architect::LlmArchitect;
use crate::engineer::{EngineerPrep, LlmEngineer, NoPrep};
use crate::human::LlmHuman;
use crate::owner::LlmOwner;
use crate::provider::ProviderConfig;
use crate::reviewer::LlmReviewer;

/// Which behavior variants and backend hooks the real registry wires in.
///
/// Defaults reproduce the happy-path topology (non-closing architect, approving
/// reviewer, no engineer prep — i.e. the in-memory/filesystem backends). The
/// Forgejo worker overrides `engineer_prep` and the scenarios that need them set
/// `architect_closing` / `reviewer_request_changes_then_approve`.
pub struct RealRegistryConfig<F: Forge + ?Sized> {
    /// When `true`, the architect also closes a merged PR's parent issues
    /// (`dependency_chain` scenario); mirrors `ClosingArchitect`.
    pub architect_closing: bool,
    /// When `true`, the reviewer requests changes on the first pass and approves
    /// on a later one; mirrors `RequestChangesThenApproveReviewer`.
    pub reviewer_request_changes_then_approve: bool,
    /// Backend side effects the engineer runs before opening a PR / addressing a
    /// CI failure (real git head, CI sentinel commit). [`NoPrep`] on
    /// memory/filesystem.
    pub engineer_prep: Arc<dyn EngineerPrep<F>>,
}

impl<F: Forge + ?Sized> Default for RealRegistryConfig<F> {
    fn default() -> Self {
        Self {
            architect_closing: false,
            reviewer_request_changes_then_approve: false,
            engineer_prep: Arc::new(NoPrep),
        }
    }
}

/// Builds a registry of real agents for every reference-delivery role, with the
/// default (happy-path) behavior variants and no engineer prep.
///
/// `F` is the Forge type the agents act over (`dyn Forge` in the worker).
pub fn real_registry<F>(provider: ProviderConfig) -> AgentRegistry<F>
where
    F: Forge + ?Sized + 'static,
{
    real_registry_with(provider, RealRegistryConfig::default())
}

/// Builds a registry of real agents with explicit behavior variants and engineer
/// prep — the production-shaped entry point the worker's `--agents real` path
/// calls.
pub fn real_registry_with<F>(
    provider: ProviderConfig,
    config: RealRegistryConfig<F>,
) -> AgentRegistry<F>
where
    F: Forge + ?Sized + 'static,
{
    let architect = if config.architect_closing {
        LlmArchitect::closing(provider.clone())
    } else {
        LlmArchitect::new(provider.clone())
    };
    let reviewer = if config.reviewer_request_changes_then_approve {
        LlmReviewer::request_changes_then_approve(provider.clone())
    } else {
        LlmReviewer::new(provider.clone())
    };
    let engineer = LlmEngineer::with_prep(provider.clone(), config.engineer_prep);

    let mut registry = AgentRegistry::new();
    registry.insert(
        RoleId::new("architect"),
        Arc::new(architect) as Arc<dyn Agent<F>>,
    );
    registry.insert(
        RoleId::new("engineer"),
        Arc::new(engineer) as Arc<dyn Agent<F>>,
    );
    registry.insert(
        RoleId::new("reviewer"),
        Arc::new(reviewer) as Arc<dyn Agent<F>>,
    );
    registry.insert(
        RoleId::new("owner"),
        Arc::new(LlmOwner::new(provider.clone())) as Arc<dyn Agent<F>>,
    );
    registry.insert(
        RoleId::new("human"),
        Arc::new(LlmHuman::new(provider)) as Arc<dyn Agent<F>>,
    );
    registry
}
