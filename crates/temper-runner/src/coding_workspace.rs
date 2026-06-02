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
use temper_workflow::{ArtifactKindId, ArtifactSource, ExternalToolId, QueueId, RoleId};

use crate::{ExternalToolBindingError, RunnerConfig};

/// Conventional id for the coding-workspace external tool declaration.
pub const CODING_WORKSPACE_TOOL_ID: &str = "coding_workspace";

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
}

impl CodingWorkspaceOutput {
    /// Builds an output for a same-repository head branch.
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
        }
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
#[derive(Clone, Default)]
pub struct ExternalToolExecutors {
    coding_workspaces: Vec<CodingWorkspaceBinding>,
}

impl ExternalToolExecutors {
    /// Creates an empty executor set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a coding-workspace provider for `role`/`tool`.
    pub fn with_coding_workspace(
        mut self,
        role: RoleId,
        tool: ExternalToolId,
        workspace: Arc<dyn CodingWorkspace>,
    ) -> Self {
        self.add_coding_workspace(role, tool, workspace);
        self
    }

    /// Adds a coding-workspace provider for `role`/`tool`.
    pub fn add_coding_workspace(
        &mut self,
        role: RoleId,
        tool: ExternalToolId,
        workspace: Arc<dyn CodingWorkspace>,
    ) -> &mut Self {
        self.coding_workspaces.push(CodingWorkspaceBinding {
            role,
            tool,
            workspace,
        });
        self
    }

    /// Returns the coding-workspace provider for `role`/`tool`, if one is bound.
    pub fn coding_workspace_for(
        &self,
        role: &RoleId,
        tool: &ExternalToolId,
    ) -> Option<Arc<dyn CodingWorkspace>> {
        self.coding_workspaces
            .iter()
            .find(|binding| &binding.role == role && &binding.tool == tool)
            .map(|binding| Arc::clone(&binding.workspace))
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
        for binding in &self.coding_workspaces {
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
struct CodingWorkspaceBinding {
    role: RoleId,
    tool: ExternalToolId,
    workspace: Arc<dyn CodingWorkspace>,
}
