//! Narrow external coding-workspace seam.
//!
//! This module models the first executable non-workflow external tool. A coding
//! workspace may prepare a git/workspace checkout, delegate edits, commit a PR
//! head, and report that head back to an agent adapter. It does **not** receive a
//! [`RoleTools`](crate::RoleTools) handle and therefore cannot mutate workflow or
//! Forge state; agents still open PRs and run transitions only through
//! [`RoleTools`](crate::RoleTools).

use std::error::Error;
use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use temper_forge::RepositoryId;
use temper_workflow::{
    ArtifactKindId, ArtifactSource, CreateIssuesChild, ExternalToolId, QueueId, RoleId, VerdictId,
};

use crate::{ExternalToolBindingError, RunnerConfig};

/// Conventional id for the coding-workspace external tool declaration.
pub const CODING_WORKSPACE_TOOL_ID: &str = "coding_workspace";

/// How a workspace should prepare its checkout for one invocation.
///
/// This is a small, role-agnostic capability hint the runner threads to the
/// provider per request. The engine never maps tool ids to roles; the operator
/// wiring chooses a checkout per declared external-tool id (see
/// `temper-worker`'s `configure_external_tool_executors`), and the default keeps
/// today's write-in-checkout behavior so an unrecognized id is never surprised
/// by a read-only tree.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum WorkspaceCheckout {
    /// Writable checkout at `base_branch`, branch pushed on the head path. This
    /// is the engineer/coding-workspace behavior and the default.
    #[default]
    Writable,
    /// Read-only checkout at `base_branch`; the command analyses but never
    /// commits or pushes (e.g. an architect triaging an intake issue).
    ReadOnly,
    /// Read-only checkout with both the pull request's head **and** base fetched
    /// so the command can compute the real diff (`git diff base...head`); never
    /// commits or pushes (e.g. a reviewer judging a PR).
    PullRequestReadOnly,
}

impl WorkspaceCheckout {
    /// Whether this checkout permits committing and pushing a branch (the head
    /// path). Read-only checkouts return a verdict-only result instead.
    pub fn is_writable(self) -> bool {
        matches!(self, WorkspaceCheckout::Writable)
    }

    /// Whether this checkout needs the pull request's head fetched in addition
    /// to its base so the command can compute the real diff.
    pub fn needs_pull_request_head(self) -> bool {
        matches!(self, WorkspaceCheckout::PullRequestReadOnly)
    }
}

/// Repository information a workspace may use to prepare a checkout.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CodingWorkspaceRepository {
    pub id: RepositoryId,
    pub owner: String,
    pub name: String,
    pub default_branch: String,
}

/// Work item information and serialized artifact context supplied to a workspace.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CodingWorkspaceWorkItem {
    pub role: RoleId,
    pub queue: QueueId,
    pub kind: ArtifactKindId,
    pub target: ArtifactSource,
    /// The same JSON context sent to the LLM role decision engine.
    pub context_json: String,
}

/// User-authored guidance relevant to workspace execution.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CodingWorkspaceGuidance {
    pub role_guidance: Option<String>,
    pub tool_guidance: Option<String>,
    pub tool_constraints: Vec<String>,
}

/// Request to prepare and commit a PR head for one work item.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CodingWorkspaceRequest {
    pub repository: CodingWorkspaceRepository,
    pub work_item: CodingWorkspaceWorkItem,
    /// Branch the PR should target.
    pub base_branch: String,
    /// Deterministic, caller-provided branch suggestion. Providers may return a
    /// different branch, but should keep retries idempotent for the same work.
    pub branch_hint: String,
    /// Correlation key the workflow runtime will use when opening/finding the PR.
    pub correlation_key: String,
    pub guidance: CodingWorkspaceGuidance,
    /// How the provider should prepare its checkout for this invocation
    /// (writable head path, read-only analysis, or read-only with the PR diff).
    /// Defaults to [`WorkspaceCheckout::Writable`], today's behavior.
    pub checkout: WorkspaceCheckout,
}

/// Head and metadata produced by a coding workspace.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CodingWorkspaceOutput {
    /// Source branch containing the committed implementation diff.
    pub branch: String,
    /// Target/base branch the workspace prepared against.
    pub base_branch: String,
    /// Short implementation summary for the PR body and logs.
    pub summary: String,
    /// Files changed by the committed branch, for safety checks and diagnostics.
    pub changed_files: Vec<String>,
    /// Labels the created PR should carry in addition to workflow metadata.
    pub labels: Vec<String>,
    /// Optional typed verdict selecting the outcome transition via the action's
    /// declared `outcomes` map. `None` keeps today's behavior: the action's
    /// own transition runs and the workspace head produces a pull request.
    pub verdict: Option<VerdictId>,
    /// Optional agent-authored body for a `set_body` effect on the routed
    /// transition (e.g. an architect rewriting an intake into a crisp code
    /// issue). Consumed through the keyed runtime-input seam; `None` when the
    /// routed transition declares no `set_body`.
    pub body: Option<String>,
    /// Optional agent-authored review body for an `attach_review` effect on the
    /// routed transition (a reviewer's native review prose). Consumed through
    /// the keyed runtime-input seam; `None` when the routed transition declares
    /// no `attach_review`.
    pub review_body: Option<String>,
    /// Workspace-authored dependent child artifacts for a `create_issues` effect
    /// on the routed transition (e.g. an architect's `needs_breakdown` verdict
    /// fanning an intake out into authored code issues). Consumed through the
    /// keyed runtime-input seam exactly as `body` / `review_body` are; empty when
    /// the routed transition declares no `create_issues`.
    pub children: Vec<CreateIssuesChild>,
}

impl CodingWorkspaceOutput {
    /// Builds an output for a same-repository head branch with no verdict (the
    /// default "produce a head" behavior).
    pub fn new(
        branch: impl Into<String>,
        base_branch: impl Into<String>,
        summary: impl Into<String>,
        changed_files: Vec<String>,
        labels: Vec<String>,
    ) -> Self {
        Self {
            branch: branch.into(),
            base_branch: base_branch.into(),
            summary: summary.into(),
            changed_files,
            labels,
            verdict: None,
            body: None,
            review_body: None,
            children: Vec::new(),
        }
    }

    /// Returns this output carrying a typed verdict that selects the outcome
    /// transition declared by the action.
    pub fn with_verdict(mut self, verdict: VerdictId) -> Self {
        self.verdict = Some(verdict);
        self
    }

    /// Returns this output carrying an agent-authored body for a `set_body`
    /// effect on the routed transition.
    pub fn with_body(mut self, body: impl Into<String>) -> Self {
        self.body = Some(body.into());
        self
    }

    /// Returns this output carrying an agent-authored review body for an
    /// `attach_review` effect on the routed transition.
    pub fn with_review_body(mut self, review_body: impl Into<String>) -> Self {
        self.review_body = Some(review_body.into());
        self
    }

    /// Returns this output carrying workspace-authored children for a
    /// `create_issues` effect on the routed transition.
    pub fn with_children(mut self, children: impl IntoIterator<Item = CreateIssuesChild>) -> Self {
        self.children = children.into_iter().collect();
        self
    }
}

/// Error returned by a coding-workspace provider.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CodingWorkspaceError {
    message: String,
}

impl CodingWorkspaceError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for CodingWorkspaceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for CodingWorkspaceError {}

/// Executable coding-workspace provider.
#[async_trait]
pub trait CodingWorkspace: Send + Sync {
    /// Prepare/check out the repository, apply or delegate edits, commit a PR
    /// head, and return the branch plus a short summary.
    async fn produce_head(
        &self,
        request: CodingWorkspaceRequest,
    ) -> Result<CodingWorkspaceOutput, CodingWorkspaceError>;
}

/// Executable external-tool providers available to role agents.
///
/// Registration and lookup are role-agnostic and keyed by executor id (the
/// declared external-tool id), not by a single hardcoded `coding_workspace`
/// seam. `coding_workspace` is simply one configured workspace executor.
#[derive(Clone, Default)]
pub struct ExternalToolExecutors {
    workspaces: Vec<WorkspaceBinding>,
}

impl ExternalToolExecutors {
    /// Creates an empty executor set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a workspace provider for `role` under executor id `tool` with
    /// the default [`WorkspaceCheckout::Writable`] capability.
    pub fn with_workspace(
        mut self,
        role: RoleId,
        tool: ExternalToolId,
        workspace: Arc<dyn CodingWorkspace>,
    ) -> Self {
        self.add_workspace(role, tool, workspace);
        self
    }

    /// Registers a workspace provider for `role` under executor id `tool` with
    /// an explicit checkout capability.
    pub fn with_workspace_checkout(
        mut self,
        role: RoleId,
        tool: ExternalToolId,
        workspace: Arc<dyn CodingWorkspace>,
        checkout: WorkspaceCheckout,
    ) -> Self {
        self.add_workspace_checkout(role, tool, workspace, checkout);
        self
    }

    /// Registers a workspace provider for `role` under executor id `tool` with
    /// the default [`WorkspaceCheckout::Writable`] capability.
    pub fn add_workspace(
        &mut self,
        role: RoleId,
        tool: ExternalToolId,
        workspace: Arc<dyn CodingWorkspace>,
    ) -> &mut Self {
        self.add_workspace_checkout(role, tool, workspace, WorkspaceCheckout::default())
    }

    /// Registers a workspace provider for `role` under executor id `tool` with
    /// an explicit checkout capability.
    pub fn add_workspace_checkout(
        &mut self,
        role: RoleId,
        tool: ExternalToolId,
        workspace: Arc<dyn CodingWorkspace>,
        checkout: WorkspaceCheckout,
    ) -> &mut Self {
        self.workspaces.push(WorkspaceBinding {
            role,
            tool,
            workspace,
            checkout,
        });
        self
    }

    /// Returns the workspace provider registered for `role` under executor id
    /// `tool`, if one is bound.
    pub fn workspace_for(
        &self,
        role: &RoleId,
        tool: &ExternalToolId,
    ) -> Option<Arc<dyn CodingWorkspace>> {
        self.binding_for(role, tool)
            .map(|binding| Arc::clone(&binding.workspace))
    }

    /// Returns the checkout capability registered for `role` under executor id
    /// `tool`, if one is bound.
    pub fn checkout_for(&self, role: &RoleId, tool: &ExternalToolId) -> Option<WorkspaceCheckout> {
        self.binding_for(role, tool).map(|binding| binding.checkout)
    }

    fn binding_for(&self, role: &RoleId, tool: &ExternalToolId) -> Option<&WorkspaceBinding> {
        self.workspaces
            .iter()
            .find(|binding| &binding.role == role && &binding.tool == tool)
    }

    /// Validates executable providers against the workflow declaration and
    /// runner metadata binding. This does not require every metadata-only binding
    /// to have an executable provider; it only rejects executable providers that
    /// exceed declared/bound authority.
    pub fn validate(
        &self,
        compiled: &temper_workflow::CompiledWorkflow,
        config: &RunnerConfig,
    ) -> Result<(), ExternalToolBindingError> {
        for binding in &self.workspaces {
            let role = compiled.role(&binding.role).ok_or_else(|| {
                ExternalToolBindingError::UnknownRole {
                    role: binding.role.clone(),
                }
            })?;
            if !role
                .external_tools
                .iter()
                .any(|manifest| manifest.id == binding.tool)
            {
                return Err(ExternalToolBindingError::UndeclaredTool {
                    role: binding.role.clone(),
                    tool: binding.tool.clone(),
                });
            }
            if !config.has_external_tool_binding(&binding.role, &binding.tool) {
                return Err(ExternalToolBindingError::ExecutableWithoutBinding {
                    role: binding.role.clone(),
                    tool: binding.tool.clone(),
                });
            }
        }
        Ok(())
    }
}

#[derive(Clone)]
struct WorkspaceBinding {
    role: RoleId,
    tool: ExternalToolId,
    workspace: Arc<dyn CodingWorkspace>,
    checkout: WorkspaceCheckout,
}
