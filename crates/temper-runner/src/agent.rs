//! Agent-facing role tools and behavior traits.
//!
//! [`Agent`] implementations decide how a role services a [`WorkItem`], while
//! [`RoleTools`] is the role-scoped boundary for changing workflow state. Agents
//! may read Forge artifacts through the helpers here, but they mutate workflow
//! state only by asking the workflow [`Executor`] to run an authorized
//! transition or by using the documented pull-request creation seam.

use crate::WorkItem;
use async_trait::async_trait;
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::sync::Arc;
use temper_forge::{
    CreateIssue, CreatePullRequest, Forge, ForgeError, Issue, IssueState, ItemNumber, PullRequest,
    Repository, RepositoryId, UpdateIssue,
};
use temper_workflow::{
    ArtifactRef, ArtifactSource, CorrelationLookupPlan, CreateIssuesChild, EnsureOutcome,
    ExecutionContext, ExecutionError, ExecutionReport, Executor, RoleId, TransitionId,
    ValidatedWorkflow, WorkflowMetadata, parse_metadata_block, replace_metadata_block,
};

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
/// workflow transitions, supply runtime PR-create inputs for authorized
/// transitions, use the documented idempotent pull-request creation seam,
/// perform read-only lookups, and close an issue when a workflow-specific
/// adapter needs to project a native lifecycle fact that is not modeled as a
/// workflow label transition.
pub struct RoleTools<'a, F: Forge + ?Sized> {
    workflow: &'a ValidatedWorkflow,
    repo: &'a RepositoryId,
    role: RoleId,
    forge: &'a F,
    context: ExecutionContext,
    executor: Executor<'a, F>,
    observability_tick_id: Option<String>,
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
            workflow,
            repo,
            role,
            forge,
            context: context.clone(),
            executor: Executor::with_context(workflow, forge, context),
            observability_tick_id: None,
        }
    }

    /// Returns tools associated with a production tick id for observability.
    pub fn with_observability_tick_id(mut self, tick_id: impl Into<String>) -> Self {
        self.observability_tick_id = Some(tick_id.into());
        self
    }

    /// Builds the provider-neutral identity for a work item seen by these tools.
    pub fn work_item_identity(&self, item: &crate::WorkItem) -> crate::WorkItemIdentity {
        let identity = crate::WorkItemIdentity::new(
            self.repo,
            &self.role,
            &item.queue,
            item.target,
            &item.kind,
        );
        if let Some(tick_id) = &self.observability_tick_id {
            identity.with_tick_id(tick_id.clone())
        } else {
            identity
        }
    }

    pub(crate) fn execution_context(&self) -> ExecutionContext {
        self.context.clone()
    }

    pub(crate) fn observability_tick_id(&self) -> Option<&str> {
        self.observability_tick_id.as_deref()
    }

    /// Repository these tools operate on.
    pub fn repo(&self) -> &RepositoryId {
        self.repo
    }

    /// Workflow role whose authority is used for transition execution.
    pub fn role(&self) -> &RoleId {
        &self.role
    }

    /// Per-kind index of the first `CreateIssues` effect declared by
    /// `transition`, if any.
    ///
    /// The executor counts effect indices per kind, so the first (and, in
    /// practice, only) `create_issues` effect on a transition is index `0`. The
    /// verdict-routed children-binding path uses this to decide whether the
    /// routed transition can consume workspace-authored children before binding
    /// them through the runtime seam.
    pub fn create_issues_effect_index(&self, transition: &TransitionId) -> Option<usize> {
        let declares_create_issues = self
            .workflow
            .transitions()
            .iter()
            .find(|candidate| &candidate.id == transition)?
            .effects
            .iter()
            .any(|effect| matches!(effect, temper_workflow::Effect::CreateIssues { .. }));
        declares_create_issues.then_some(0)
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

    /// Runs a workflow transition with a runtime pull-request create input and
    /// correlation key for one `CreatePullRequest` effect.
    pub async fn run_with_pull_request_create_at(
        &self,
        target: ArtifactSource,
        transition: &TransitionId,
        effect_index: usize,
        correlation_key: impl Into<String>,
        input: CreatePullRequest,
    ) -> Result<ExecutionReport, ExecutionError> {
        let mut context = self.context.clone();
        context.set_pull_request_create_at(transition.clone(), effect_index, input);
        context.set_pull_request_correlation_key_at(
            transition.clone(),
            effect_index,
            correlation_key,
        );
        Executor::with_context(self.workflow, self.forge, context)
            .execute(self.repo, target, transition, &self.role)
            .await
    }

    /// Runs a workflow transition with a runtime agent-authored body and
    /// correlation key for one `SetBody` effect.
    ///
    /// The body is the workspace work product written onto the target artifact,
    /// supplied through the same keyed runtime-input seam that
    /// [`run_with_pull_request_create_at`](Self::run_with_pull_request_create_at)
    /// uses for a pull-request head.
    pub async fn run_with_set_body_at(
        &self,
        target: ArtifactSource,
        transition: &TransitionId,
        effect_index: usize,
        correlation_key: impl Into<String>,
        body: impl Into<String>,
    ) -> Result<ExecutionReport, ExecutionError> {
        let mut context = self.context.clone();
        context.set_set_body_at(transition.clone(), effect_index, body);
        context.set_set_body_correlation_key_at(transition.clone(), effect_index, correlation_key);
        Executor::with_context(self.workflow, self.forge, context)
            .execute(self.repo, target, transition, &self.role)
            .await
    }

    /// Runs a workflow transition with a runtime agent-authored review body and
    /// correlation key for one `AttachReview` effect.
    ///
    /// The review body is the workspace work product carried into a native
    /// pull-request review; the decision is declared by the effect. This is the
    /// review counterpart of
    /// [`run_with_pull_request_create_at`](Self::run_with_pull_request_create_at).
    pub async fn run_with_attach_review_at(
        &self,
        target: ArtifactSource,
        transition: &TransitionId,
        effect_index: usize,
        correlation_key: impl Into<String>,
        body: impl Into<String>,
    ) -> Result<ExecutionReport, ExecutionError> {
        let mut context = self.context.clone();
        context.set_attach_review_at(transition.clone(), effect_index, body);
        context.set_attach_review_correlation_key_at(
            transition.clone(),
            effect_index,
            correlation_key,
        );
        Executor::with_context(self.workflow, self.forge, context)
            .execute(self.repo, target, transition, &self.role)
            .await
    }

    /// Runs `transition`, binding whichever content-bearing work-product slots
    /// are present into the runtime seam for one `SetBody` / `AttachReview`
    /// effect each.
    ///
    /// This is the verdict-routed counterpart of the pull-request-create path:
    /// a workspace verdict routes to a content-bearing transition, and the
    /// authored body / review body it produced are bound here so the routed
    /// transition's `set_body` / `attach_review` effects can apply. Slots left
    /// `None` bind nothing; the executor then fails any declared effect that has
    /// no input, exactly as it does for an unbound pull-request create.
    pub async fn run_with_workspace_content(
        &self,
        target: ArtifactSource,
        transition: &TransitionId,
        correlation_key: impl Into<String>,
        body: Option<String>,
        review_body: Option<String>,
    ) -> Result<ExecutionReport, ExecutionError> {
        let correlation_key = correlation_key.into();
        let mut context = self.context.clone();
        if let Some(body) = body {
            context.set_set_body_at(transition.clone(), 0, body);
            context.set_set_body_correlation_key_at(transition.clone(), 0, correlation_key.clone());
        }
        if let Some(review_body) = review_body {
            context.set_attach_review_at(transition.clone(), 0, review_body);
            context.set_attach_review_correlation_key_at(transition.clone(), 0, correlation_key);
        }
        Executor::with_context(self.workflow, self.forge, context)
            .execute(self.repo, target, transition, &self.role)
            .await
    }

    /// Runs `transition`, binding the workspace-authored children into the
    /// runtime seam for one `CreateIssues` effect.
    ///
    /// This is the multi-artifact counterpart of the pull-request-create path:
    /// a workspace verdict routes to a transition whose `create_issues` effect
    /// fans out a plan of dependent children, and the children it authored
    /// (titles/bodies/labels and the dependency relations between them) are
    /// bound here so the effect can apply. The children land independently and
    /// idempotently under `base_correlation_key`, each linked back to the target
    /// artifact as parent — exactly as
    /// [`run_with_pull_request_create_at`](Self::run_with_pull_request_create_at)
    /// binds a pull-request head.
    pub async fn run_with_create_issues_at(
        &self,
        target: ArtifactSource,
        transition: &TransitionId,
        effect_index: usize,
        base_correlation_key: impl Into<String>,
        children: impl IntoIterator<Item = CreateIssuesChild>,
    ) -> Result<ExecutionReport, ExecutionError> {
        let mut context = self.context.clone();
        context.set_create_issues_at(transition.clone(), effect_index, children);
        context.set_create_issues_correlation_key_at(
            transition.clone(),
            effect_index,
            base_correlation_key,
        );
        Executor::with_context(self.workflow, self.forge, context)
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

    /// Idempotently opens or finds an issue in `target_repo`.
    ///
    /// The target repository is checked through the same Forge handle that the
    /// worker uses, so authority follows the token's repository visibility and
    /// write permissions rather than this worker's scan shard. The created or
    /// found issue carries `correlation_key` plus an explicit repo-qualified
    /// parent back-reference in its workflow metadata.
    pub async fn ensure_issue_in_repo(
        &self,
        target_repo: &RepositoryId,
        correlation_key: &str,
        parent: ArtifactRef,
        input: CreateIssue,
    ) -> Result<EnsureOutcome<Issue>, ExecutionError> {
        self.ensure_target_repo_visible(target_repo).await?;
        let parent = repo_qualified_parent(self.repo, parent);
        self.executor
            .ensure_issue_with_parent(target_repo, correlation_key, Some(parent), input)
            .await
            .map_err(|error| annotate_target_repo_error(target_repo, error))
    }

    /// Records a metadata fallback dependency from a source issue in this repo
    /// to `dependency`.
    ///
    /// This is the narrow parent→child link used by architect fan-out until a
    /// backend exposes portable cross-repository native dependency links. The
    /// update is compare-and-swap guarded and idempotent: retrying the same link
    /// returns `false` once it is already present.
    pub async fn add_issue_dependency_metadata(
        &self,
        source: ItemNumber,
        dependency: ArtifactRef,
    ) -> Result<bool, ExecutionError> {
        for _ in 0..3 {
            let Some(issue) = self.get_issue(source).await? else {
                return Err(ExecutionError::TargetMissing {
                    target: ArtifactSource::Issue { number: source },
                });
            };
            let mut metadata = parse_metadata_block(&issue.body)
                .map_err(|error| ExecutionError::Backend {
                    message: format!("invalid issue workflow metadata: {error}"),
                })?
                .unwrap_or_default();
            if metadata
                .dependencies
                .iter()
                .any(|candidate| candidate == &dependency)
            {
                return Ok(false);
            }
            metadata.dependencies.push(dependency.clone());
            let body = replace_metadata_block(&issue.body, &metadata).map_err(|error| {
                ExecutionError::Backend {
                    message: format!("could not update issue workflow metadata: {error}"),
                }
            })?;
            match self
                .forge
                .update_issue(
                    &issue.id,
                    UpdateIssue {
                        body: Some(body),
                        expected_version: Some(issue.version),
                        ..UpdateIssue::default()
                    },
                )
                .await
            {
                Ok(_) => return Ok(true),
                Err(ForgeError::Conflict(_)) => continue,
                Err(error) => return Err(error.into()),
            }
        }
        Err(ExecutionError::Backend {
            message: format!(
                "could not add dependency metadata to issue #{source} after concurrent updates"
            ),
        })
    }

    /// Reads the repository these tools are scoped to.
    pub async fn get_repository(&self) -> Result<Option<Repository>, ForgeError> {
        self.forge.get_repository(self.repo).await
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

    /// Closes an issue by repository-scoped number.
    ///
    /// This is a narrow native lifecycle tool rather than a workflow-label
    /// transition. The reference-delivery architect uses it to document the
    /// current engine gap that merging a produced PR does not automatically
    /// close its parent code issue; dependency gates observe the resulting
    /// native closed state. Closing also clears the active `in-progress`
    /// lifecycle label so completed code issues no longer look claimed.
    pub async fn close_issue(&self, number: ItemNumber) -> Result<bool, ForgeError> {
        let Some(issue) = self.get_issue(number).await? else {
            return Ok(false);
        };
        let was_open = issue.state != IssueState::Closed;
        let was_in_progress = issue.labels.iter().any(|label| label == "in-progress");
        if !was_open && !was_in_progress {
            return Ok(false);
        }
        self.forge
            .update_issue(
                &issue.id,
                UpdateIssue {
                    state: was_open.then_some(IssueState::Closed),
                    remove_labels: if was_in_progress {
                        vec!["in-progress".to_string()]
                    } else {
                        Vec::new()
                    },
                    ..UpdateIssue::default()
                },
            )
            .await?;
        Ok(true)
    }

    /// Finds a pull request whose workflow metadata carries `correlation_key`.
    pub async fn find_pull_request_by_correlation(
        &self,
        correlation_key: &str,
    ) -> Result<Option<PullRequest>, ForgeError> {
        let plan = CorrelationLookupPlan::new(correlation_key, &[]);
        let mut seen = BTreeSet::new();
        let mut candidates = Vec::new();
        for query in plan.pull_request_queries() {
            for pull_request in self.forge.list_pull_requests(self.repo, query).await? {
                if seen.insert(pull_request.id.clone()) {
                    candidates.push(pull_request);
                }
            }
        }
        Ok(candidates
            .into_iter()
            .find(|pull_request| metadata_has_correlation_key(&pull_request.body, correlation_key)))
    }

    /// Finds an issue in `target_repo` whose workflow metadata carries
    /// `correlation_key`.
    pub async fn find_issue_in_repo_by_correlation(
        &self,
        target_repo: &RepositoryId,
        correlation_key: &str,
    ) -> Result<Option<Issue>, ForgeError> {
        let plan = CorrelationLookupPlan::new(correlation_key, &[]);
        let mut seen = BTreeSet::new();
        let mut candidates = Vec::new();
        for query in plan.issue_queries() {
            for issue in self.forge.list_issues(target_repo, query).await? {
                if seen.insert(issue.id.clone()) {
                    candidates.push(issue);
                }
            }
        }
        Ok(candidates
            .into_iter()
            .find(|issue| metadata_has_correlation_key(&issue.body, correlation_key)))
    }

    async fn ensure_target_repo_visible(
        &self,
        target_repo: &RepositoryId,
    ) -> Result<(), ExecutionError> {
        match self.forge.get_repository(target_repo).await {
            Ok(Some(_)) => Ok(()),
            Ok(None) => Err(ExecutionError::Backend {
                message: format!(
                    "cannot write target repository `{target_repo}`: repository is not visible to this Forge handle or does not exist"
                ),
            }),
            Err(error) => Err(ExecutionError::Backend {
                message: format!(
                    "cannot verify access to target repository `{target_repo}`: {error}"
                ),
            }),
        }
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

fn repo_qualified_parent(source_repo: &RepositoryId, parent: ArtifactRef) -> ArtifactRef {
    ArtifactRef::in_repo(parent.resolved_repository(source_repo), parent.number)
}

fn annotate_target_repo_error(target_repo: &RepositoryId, error: ExecutionError) -> ExecutionError {
    match error {
        ExecutionError::Backend { message } => ExecutionError::Backend {
            message: format!("cannot ensure issue in target repository `{target_repo}`: {message}"),
        },
        other => other,
    }
}
