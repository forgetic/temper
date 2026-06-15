//! Agent-facing role tools and behavior traits.
//!
//! [`Agent`] implementations decide how a role services a [`WorkItem`], while
//! [`RoleTools`] is the role-scoped boundary for changing workflow state. Agents
//! may read Forge artifacts through the helpers here, but they mutate workflow
//! state only by asking the workflow [`Executor`](temper_workflow::Executor) to
//! run an authorized transition or by using the documented pull-request creation
//! seam.

mod error;
mod tools;

use crate::WorkItem;
use async_trait::async_trait;
use std::collections::BTreeMap;
use std::sync::Arc;
use temper_forge::Forge;
use temper_workflow::RoleId;

pub use error::AgentError;
pub use tools::RoleTools;

/// Behavior adapter for a workflow role.
///
/// An agent services one work item and returns whether it changed workflow
/// state. It may run several transitions and observe their results, but it must
/// tolerate stale items and return when no more progress is possible.
#[async_trait]
pub trait Agent<F: Forge + ?Sized>: Send + Sync {
    /// Services a work item through the role-scoped tool boundary.
    async fn service(&self, item: &WorkItem, tools: &RoleTools<'_, F>) -> Result<bool, AgentError>;
}

/// Registry mapping workflow roles to agent implementations.
#[derive(Clone)]
pub struct AgentRegistry<F: Forge + ?Sized> {
    agents: BTreeMap<RoleId, Arc<dyn Agent<F>>>,
}

impl<F: Forge + ?Sized> AgentRegistry<F> {
    /// Creates an empty registry.
    pub fn new() -> Self {
        Self {
            agents: BTreeMap::new(),
        }
    }

    /// Inserts an already type-erased agent for `role`.
    pub fn insert(&mut self, role: RoleId, agent: Arc<dyn Agent<F>>) -> Option<Arc<dyn Agent<F>>> {
        self.agents.insert(role, agent)
    }

    /// Constructs and inserts an agent for `role`.
    pub fn register<A>(&mut self, role: RoleId, agent: A) -> Option<Arc<dyn Agent<F>>>
    where
        A: Agent<F> + 'static,
    {
        self.insert(role, Arc::new(agent))
    }

    /// Returns the agent registered for `role`.
    pub fn get(&self, role: &RoleId) -> Option<&Arc<dyn Agent<F>>> {
        self.agents.get(role)
    }

    /// Returns whether an agent is registered for `role`.
    pub fn contains_role(&self, role: &RoleId) -> bool {
        self.agents.contains_key(role)
    }
}

impl<F: Forge + ?Sized> Default for AgentRegistry<F> {
    fn default() -> Self {
        Self::new()
    }
}
