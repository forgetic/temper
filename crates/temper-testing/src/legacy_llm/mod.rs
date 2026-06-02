//! Test-only legacy reference-delivery LLM adapters.
//!
//! These modules quarantine the old hard-coded reference-delivery prompts and
//! role-specific adapters outside production `temper-agents`. They exist only
//! to keep the historical gated real-agent e2e scenarios available while the
//! production path uses manifest-driven `temper_agents::LlmRoleAgent` values.

use std::sync::Arc;

use temper_agents::ProviderConfig;
use temper_forge::Forge;
use temper_runner::{Agent, AgentRegistry};
use temper_workflow::RoleId;

pub mod architect;
mod common;
pub mod engineer;
pub mod human;
pub mod owner;
mod prompts;
pub mod reviewer;

pub use architect::{ArchitectDecision, LlmArchitect};
pub use engineer::{EngineerDecision, EngineerPrep, LlmEngineer, NoPrep};
pub use human::{HumanDecision, LlmHuman};
pub use owner::{LlmOwner, OwnerDecision};
pub use reviewer::{LlmReviewer, ReviewerDecision};

/// Which behavior variants and backend hooks the test-only legacy registry wires
/// in for reference-delivery e2e scenarios.
pub struct LegacyRealRegistryConfig<F: Forge + ?Sized> {
    /// When `true`, the architect also closes a merged PR's parent issues.
    pub architect_closing: bool,
    /// When `true`, the reviewer requests changes on the first pass and approves
    /// on a later one.
    pub reviewer_request_changes_then_approve: bool,
    /// Backend side effects the engineer runs before opening a PR / addressing a
    /// CI failure.
    pub engineer_prep: Arc<dyn EngineerPrep<F>>,
}

impl<F: Forge + ?Sized> Default for LegacyRealRegistryConfig<F> {
    fn default() -> Self {
        Self {
            architect_closing: false,
            reviewer_request_changes_then_approve: false,
            engineer_prep: Arc::new(NoPrep),
        }
    }
}

/// Builds the quarantined reference-delivery real-agent registry for tests.
pub fn legacy_real_registry_with<F>(
    provider: ProviderConfig,
    config: LegacyRealRegistryConfig<F>,
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
