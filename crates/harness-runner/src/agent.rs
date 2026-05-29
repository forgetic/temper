//! Agent-facing role tools and behavior traits.
//!
//! [`Agent`] implementations decide how a role services a [`WorkItem`], while
//! [`RoleTools`] is the role-scoped boundary for changing workflow state. Agents
//! may read Forge artifacts through the helpers here, but they mutate workflow
//! state only by asking the workflow [`Executor`] to run an authorized
//! transition or by using the documented pull-request creation seam.

use crate::WorkItem;
use async_trait::async_trait;
use harness_forge::{
    CreatePullRequest, Forge, ForgeError, Issue, ItemNumber, PullRequest, PullRequestQuery,
    RepositoryId,
};
use harness_workflow::{
    parse_metadata_block, ArtifactSource, EnsureOutcome, ExecutionContext, ExecutionError,
    ExecutionReport, Executor, RoleId, TransitionId, ValidatedWorkflow, WorkflowMetadata,
};
use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::sync::Arc;

/// Error returned by an [`Agent`] while servicing a work item.
#[derive(Debug)]
pub enum AgentError {
    /// A workflow transition or idempotent create failed.
    Execution(ExecutionError),
    /// A read through the Forge backend failed.
    Forge(ForgeError),
    /// Agent-provider or behavior-specific failure.
    Message(String),
}

impl AgentError {
    /// Creates a behavior/provider error from a displayable message.
    pub fn message(message: impl Into<String>) -> Self {
        Self::Message(message.into())
    }
}

impl fmt::Display for AgentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AgentError::Execution(error) => write!(formatter, "workflow execution failed: {error}"),
            AgentError::Forge(error) => write!(formatter, "forge read failed: {error}"),
            AgentError::Message(message) => formatter.write_str(message),
        }
    }
}

impl Error for AgentError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            AgentError::Execution(error) => Some(error),
            AgentError::Forge(error) => Some(error),
            AgentError::Message(_) => None,
        }
    }
}

impl From<ExecutionError> for AgentError {
    fn from(error: ExecutionError) -> Self {
        Self::Execution(error)
    }
}

impl From<ForgeError> for AgentError {
    fn from(error: ForgeError) -> Self {
        Self::Forge(error)
    }
}

/// Role-scoped facade an [`Agent`] uses to observe and change workflow state.
///
/// The facade is intentionally narrower than [`Forge`]: agents can run
/// workflow transitions, use the documented idempotent pull-request creation
/// seam, and perform read-only lookups. They do not receive raw Forge mutation
/// APIs.
pub struct RoleTools<'a, F: Forge + ?Sized> {
    repo: &'a RepositoryId,
    role: RoleId,
    forge: &'a F,
    executor: Executor<'a, F>,
}

impl<'a, F: Forge + ?Sized> RoleTools<'a, F> {
    /// Builds role tools from a workflow, backend, repository, role, and
    /// execution context.
    pub fn new(
        workflow: &'a ValidatedWorkflow,
        forge: &'a F,
        repo: &'a RepositoryId,
        role: RoleId,
        context: ExecutionContext,
    ) -> Self {
        Self {
            repo,
            role,
            forge,
            executor: Executor::with_context(workflow, forge, context),
        }
    }

    /// Repository these tools operate on.
    pub fn repo(&self) -> &RepositoryId {
        self.repo
    }

    /// Workflow role whose authority is used for transition execution.
    pub fn role(&self) -> &RoleId {
        &self.role
    }

    /// Runs a workflow transition for this role against `target`.
    ///
    /// The executor reloads and re-plans against fresh Forge state, so stale or
    /// invalid agent choices fail as validation/precondition errors without
    /// bypassing workflow policy.
    pub async fn run(
        &self,
        target: ArtifactSource,
        transition: &TransitionId,
    ) -> Result<ExecutionReport, ExecutionError> {
        self.executor
            .execute(self.repo, target, transition, &self.role)
            .await
    }

    /// Idempotently opens or finds a pull request for `correlation_key`.
    ///
    /// This wraps [`Executor::ensure_pull_request`], the runtime-keyed creation
    /// seam used when an agent produces a pull request for a specific work item.
    pub async fn open_pull_request(
        &self,
        correlation_key: &str,
        input: CreatePullRequest,
    ) -> Result<EnsureOutcome<PullRequest>, ExecutionError> {
        self.executor
            .ensure_pull_request(self.repo, correlation_key, input)
            .await
    }

    /// Reads an issue by repository-scoped number.
    pub async fn get_issue(&self, number: ItemNumber) -> Result<Option<Issue>, ForgeError> {
        self.forge.get_issue_by_number(self.repo, number).await
    }

    /// Reads a pull request by repository-scoped number.
    pub async fn get_pull_request(
        &self,
        number: ItemNumber,
    ) -> Result<Option<PullRequest>, ForgeError> {
        self.forge
            .get_pull_request_by_number(self.repo, number)
            .await
    }

    /// Finds a pull request whose workflow metadata carries `correlation_key`.
    pub async fn find_pull_request_by_correlation(
        &self,
        correlation_key: &str,
    ) -> Result<Option<PullRequest>, ForgeError> {
        let pull_requests = self
            .forge
            .list_pull_requests(self.repo, PullRequestQuery::default())
            .await?;
        Ok(pull_requests
            .into_iter()
            .find(|pull_request| metadata_has_correlation_key(&pull_request.body, correlation_key)))
    }
}

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

fn metadata_has_correlation_key(body: &str, correlation_key: &str) -> bool {
    matches!(
        parse_metadata_block(body),
        Ok(Some(WorkflowMetadata {
            correlation_key: Some(ref key),
            ..
        })) if key == correlation_key
    )
}
