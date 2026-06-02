//! Manifest-driven LLM workflow role agent.
//!
//! [`LlmRoleAgent`] is the generic replacement path for hard-coded role prompt
//! constants and role-specific decision enums: it owns a compiled
//! [`RoleManifest`], asks a decision engine for one `{ action, reason }` JSON
//! decision using the manifest's rendered prompt, validates that action against
//! the manifest's tool list, and runs only the matching workflow transition
//! through [`RoleTools`].

use std::sync::Arc;

use async_trait::async_trait;
use harness_forge::{BranchRef, CreatePullRequest, Forge, ItemNumber};
use harness_runner::{
    Agent, AgentError, BoundExternalTool, CODING_WORKSPACE_TOOL_ID, CodingWorkspace,
    CodingWorkspaceGuidance, CodingWorkspaceRepository, CodingWorkspaceRequest,
    CodingWorkspaceWorkItem, ExternalToolExecutors, RoleTools, WorkItem,
};
use harness_workflow::{
    ArtifactKindId, ArtifactRef, ArtifactSource, Effect, RoleManifest, ToolManifest,
    WorkflowMetadata, render_metadata_block,
};
use serde::Deserialize;

use crate::common::{build_context, run_or_ignore_stale};
use crate::decision::{DecisionError, run_decision};
use crate::provider::ProviderConfig;

const NO_ACTION: &str = "no_action";
const EXTERNAL_TOOL_SECTION: &str = "User-declared external tools";

/// Generic workflow-role decision returned by a model or injected test seam.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
pub struct RoleDecision {
    /// One manifest tool name, or [`no_action`](NO_ACTION).
    pub action: String,
    /// Short rationale for logs and operator debugging. The generic adapter does
    /// not use this to grant authority.
    #[serde(default)]
    pub reason: String,
}

impl RoleDecision {
    /// Builds a decision that deliberately makes no workflow mutation.
    pub fn no_action(reason: impl Into<String>) -> Self {
        Self {
            action: NO_ACTION.to_string(),
            reason: reason.into(),
        }
    }

    /// Builds a decision for an action name.
    pub fn action(action: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            action: action.into(),
            reason: reason.into(),
        }
    }
}

/// Mockable seam for obtaining one generic role decision.
#[async_trait]
pub trait RoleDecisionEngine: Send + Sync {
    /// Decide from the system prompt and user context the adapter constructed.
    async fn decide(
        &self,
        system_prompt: &str,
        user_context: &str,
    ) -> Result<RoleDecision, DecisionError>;
}

/// Provider-backed decision engine that runs the real `pi` SDK path.
pub struct ProviderRoleDecisionEngine {
    provider: ProviderConfig,
}

impl ProviderRoleDecisionEngine {
    /// Builds a decision engine backed by the configured LLM provider.
    pub fn new(provider: ProviderConfig) -> Self {
        Self { provider }
    }
}

#[async_trait]
impl RoleDecisionEngine for ProviderRoleDecisionEngine {
    async fn decide(
        &self,
        system_prompt: &str,
        user_context: &str,
    ) -> Result<RoleDecision, DecisionError> {
        run_decision::<RoleDecision>(&self.provider, system_prompt, user_context).await
    }
}

/// Generic LLM agent for one compiled workflow role.
pub struct LlmRoleAgent {
    manifest: RoleManifest,
    bound_external_tools: Vec<BoundExternalTool>,
    external_tool_executors: ExternalToolExecutors,
    decision_engine: Arc<dyn RoleDecisionEngine>,
}

impl LlmRoleAgent {
    /// Builds a provider-backed agent for `manifest`.
    pub fn new(manifest: RoleManifest, provider: ProviderConfig) -> Self {
        Self::with_bound_external_tools(manifest, provider, Vec::new())
    }

    /// Builds a provider-backed agent with external tools validated by the runner.
    pub fn with_bound_external_tools(
        manifest: RoleManifest,
        provider: ProviderConfig,
        bound_external_tools: Vec<BoundExternalTool>,
    ) -> Self {
        Self::with_bound_external_tools_and_executors(
            manifest,
            provider,
            bound_external_tools,
            ExternalToolExecutors::new(),
        )
    }

    /// Builds a provider-backed agent with external tool metadata plus
    /// executable providers validated by the runner.
    pub fn with_bound_external_tools_and_executors(
        manifest: RoleManifest,
        provider: ProviderConfig,
        bound_external_tools: Vec<BoundExternalTool>,
        external_tool_executors: ExternalToolExecutors,
    ) -> Self {
        Self::with_decision_engine_external_tools_and_executors(
            manifest,
            Arc::new(ProviderRoleDecisionEngine::new(provider)) as Arc<dyn RoleDecisionEngine>,
            bound_external_tools,
            external_tool_executors,
        )
    }

    /// Builds an agent with an injected decision engine, for hermetic tests or
    /// alternate providers.
    pub fn with_decision_engine(
        manifest: RoleManifest,
        decision_engine: Arc<dyn RoleDecisionEngine>,
    ) -> Self {
        Self::with_decision_engine_and_external_tools(manifest, decision_engine, Vec::new())
    }

    /// Builds an agent with an injected decision engine and runner-bound tools.
    pub fn with_decision_engine_and_external_tools(
        manifest: RoleManifest,
        decision_engine: Arc<dyn RoleDecisionEngine>,
        bound_external_tools: Vec<BoundExternalTool>,
    ) -> Self {
        Self::with_decision_engine_external_tools_and_executors(
            manifest,
            decision_engine,
            bound_external_tools,
            ExternalToolExecutors::new(),
        )
    }

    /// Builds an agent with an injected decision engine, runner-bound metadata,
    /// and executable external-tool providers.
    pub fn with_decision_engine_external_tools_and_executors(
        manifest: RoleManifest,
        decision_engine: Arc<dyn RoleDecisionEngine>,
        bound_external_tools: Vec<BoundExternalTool>,
        external_tool_executors: ExternalToolExecutors,
    ) -> Self {
        let bound_external_tools = declared_bound_tools(&manifest, bound_external_tools);
        Self {
            manifest,
            bound_external_tools,
            external_tool_executors,
            decision_engine,
        }
    }

    /// Returns the compiled role manifest this agent enforces.
    pub fn manifest(&self) -> &RoleManifest {
        &self.manifest
    }

    /// Returns the declared-and-bound external tools visible to the model.
    pub fn bound_external_tools(&self) -> &[BoundExternalTool] {
        &self.bound_external_tools
    }

    async fn decide(&self, item: &WorkItem, context: &str) -> Result<RoleDecision, AgentError> {
        let system_prompt = self.runtime_system_prompt();
        match self.decision_engine.decide(&system_prompt, context).await {
            Ok(decision) => Ok(decision),
            Err(DecisionError::Provider(error)) => Err(AgentError::message(error.to_string())),
            Err(error) => {
                eprintln!(
                    "harness-agents: LLM decision failed for role '{}' on {:?} queue '{}', treating as no-action: {error}",
                    self.manifest.id,
                    item.target,
                    item.queue.as_str()
                );
                Ok(RoleDecision::no_action("decision failed"))
            }
        }
    }

    fn tool_for_action(&self, action: &str) -> Option<&ToolManifest> {
        self.manifest.tools.iter().find(|tool| tool.name == action)
    }

    fn coding_workspace(&self) -> Option<Arc<dyn CodingWorkspace>> {
        let declared = self
            .bound_external_tools
            .iter()
            .find(|tool| tool.id.as_str() == CODING_WORKSPACE_TOOL_ID)?;
        self.external_tool_executors
            .coding_workspace_for(&self.manifest.id, &declared.id)
    }

    fn declares_coding_workspace(&self) -> bool {
        self.manifest
            .external_tools
            .iter()
            .any(|tool| tool.id.as_str() == CODING_WORKSPACE_TOOL_ID)
    }

    fn runtime_system_prompt(&self) -> String {
        if self.manifest.external_tools.is_empty() {
            return self.manifest.prompt.render();
        }
        let mut prompt = self.manifest.prompt.clone();
        if let Some(section) = prompt.section_mut(EXTERNAL_TOOL_SECTION) {
            section.lines = runtime_external_tool_lines(&self.bound_external_tools);
        }
        prompt.render()
    }

    fn user_context(&self, work_item_context: &str) -> String {
        let work_item = serde_json::from_str::<serde_json::Value>(work_item_context)
            .unwrap_or_else(|_| serde_json::Value::String(work_item_context.to_string()));
        let allowed_actions = std::iter::once(NO_ACTION.to_string())
            .chain(self.manifest.tools.iter().map(|tool| tool.name.clone()))
            .collect::<Vec<_>>();
        let authorized_actions = self
            .manifest
            .tools
            .iter()
            .map(|tool| {
                serde_json::json!({
                    "action": tool.name,
                    "transition": tool.transition.as_str(),
                    "artifact": tool.artifact.as_str(),
                    "requires_gates": tool
                        .requires_gates
                        .iter()
                        .map(|gate| gate.as_str())
                        .collect::<Vec<_>>(),
                })
            })
            .collect::<Vec<_>>();
        let available_external_tools = self
            .bound_external_tools
            .iter()
            .map(|tool| {
                serde_json::json!({
                    "id": tool.id.as_str(),
                    "provider": tool.provider.as_str(),
                    "description": tool.description.as_str(),
                    "required": tool.required,
                    "constraints": &tool.constraints,
                    "guidance": tool.guidance.as_deref(),
                })
            })
            .collect::<Vec<_>>();
        let context = serde_json::json!({
            "work_item": work_item,
            "allowed_actions": allowed_actions,
            "authorized_actions": authorized_actions,
            "available_external_tools": available_external_tools,
        });
        serde_json::to_string_pretty(&context).unwrap_or_else(|_| context.to_string())
    }

    async fn run_tool<F: Forge + ?Sized>(
        &self,
        item: &WorkItem,
        tools: &RoleTools<'_, F>,
        tool: &ToolManifest,
        work_item_context: &str,
    ) -> Result<bool, AgentError> {
        if tool_creates_pull_request(tool) && self.declares_coding_workspace() {
            let Some(workspace) = self.coding_workspace() else {
                eprintln!(
                    "harness-agents: role '{}' cannot run '{}' because coding_workspace is declared but not executable-bound",
                    self.manifest.id, tool.name
                );
                return Ok(false);
            };
            return self
                .run_pull_request_create_tool(item, tools, tool, work_item_context, workspace)
                .await;
        }
        run_or_ignore_stale(tools, item.target, tool.transition.as_str()).await
    }

    async fn run_pull_request_create_tool<F: Forge + ?Sized>(
        &self,
        item: &WorkItem,
        tools: &RoleTools<'_, F>,
        tool: &ToolManifest,
        work_item_context: &str,
        workspace: Arc<dyn CodingWorkspace>,
    ) -> Result<bool, AgentError> {
        if create_pull_request_count(tool) != 1 {
            return Err(AgentError::message(format!(
                "coding workspace supports exactly one CreatePullRequest effect for action '{}'",
                tool.name
            )));
        }
        let ArtifactSource::Issue { number } = item.target else {
            return Ok(false);
        };
        let Some(issue) = tools.get_issue(number).await? else {
            return Ok(false);
        };
        let repository = tools
            .get_repository()
            .await?
            .ok_or_else(|| AgentError::message(format!("repository {} not found", tools.repo())))?;
        let base_branch = if repository.default_branch.trim().is_empty() {
            "main".to_string()
        } else {
            repository.default_branch.clone()
        };
        let correlation_key = pr_correlation_key(&item.kind, number);
        let request = CodingWorkspaceRequest {
            repository: CodingWorkspaceRepository {
                id: repository.id.clone(),
                owner: repository.owner,
                name: repository.name,
                default_branch: repository.default_branch,
            },
            work_item: CodingWorkspaceWorkItem {
                role: self.manifest.id.clone(),
                queue: item.queue.clone(),
                kind: item.kind.clone(),
                target: item.target,
                context_json: work_item_context.to_string(),
            },
            base_branch: base_branch.clone(),
            branch_hint: pr_branch_hint(&item.kind, number),
            correlation_key: correlation_key.clone(),
            guidance: self.workspace_guidance(),
        };
        let output = workspace
            .produce_head(request)
            .await
            .map_err(|error| AgentError::message(format!("coding workspace failed: {error}")))?;
        if output.branch.trim().is_empty() {
            return Err(AgentError::message(
                "coding workspace returned an empty PR head branch",
            ));
        }
        if output.changed_files.is_empty() {
            return Err(AgentError::message(
                "coding workspace returned no changed files for PR head",
            ));
        }
        let input = workspace_pull_request_input(
            tools.repo().clone(),
            number,
            &issue.title,
            output,
            base_branch,
        );
        match tools
            .run_with_pull_request_create_at(
                item.target,
                &tool.transition,
                0,
                correlation_key,
                input,
            )
            .await
        {
            Ok(_) => Ok(true),
            Err(error) if crate::common::stale_execution(&error) => Ok(false),
            Err(error) => Err(error.into()),
        }
    }

    fn workspace_guidance(&self) -> CodingWorkspaceGuidance {
        let role_guidance = self
            .manifest
            .charter
            .iter()
            .chain(self.manifest.prompt_extension.guidance.iter())
            .cloned()
            .collect::<Vec<_>>()
            .join("\n\n");
        let tool_guidance = self
            .manifest
            .prompt_extension
            .tool_guidance
            .clone()
            .or_else(|| {
                self.bound_external_tools
                    .iter()
                    .find(|tool| tool.id.as_str() == CODING_WORKSPACE_TOOL_ID)
                    .and_then(|tool| tool.guidance.clone())
            });
        let tool_constraints = self
            .bound_external_tools
            .iter()
            .find(|tool| tool.id.as_str() == CODING_WORKSPACE_TOOL_ID)
            .map(|tool| tool.constraints.clone())
            .unwrap_or_default();
        CodingWorkspaceGuidance {
            role_guidance: (!role_guidance.trim().is_empty()).then_some(role_guidance),
            tool_guidance,
            tool_constraints,
        }
    }
}

fn tool_creates_pull_request(tool: &ToolManifest) -> bool {
    create_pull_request_count(tool) > 0
}

fn create_pull_request_count(tool: &ToolManifest) -> usize {
    tool.effects
        .iter()
        .filter(|effect| matches!(effect, Effect::CreatePullRequest { .. }))
        .count()
}

fn pr_correlation_key(kind: &ArtifactKindId, number: ItemNumber) -> String {
    format!("pr-for-{}-{}", safe_fragment(kind.as_str()), number.get())
}

fn pr_branch_hint(kind: &ArtifactKindId, number: ItemNumber) -> String {
    format!("agent/{}", pr_correlation_key(kind, number))
}

fn safe_fragment(value: &str) -> String {
    let safe: String = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '-'
            }
        })
        .collect();
    if safe.is_empty() {
        "work".to_string()
    } else {
        safe
    }
}

fn workspace_pull_request_input(
    repo: harness_forge::RepositoryId,
    code_number: ItemNumber,
    issue_title: &str,
    output: harness_runner::CodingWorkspaceOutput,
    default_base_branch: String,
) -> CreatePullRequest {
    let metadata = WorkflowMetadata {
        kind: Some(ArtifactKindId::new("implementation_pr")),
        parents: vec![ArtifactRef::same_repo(code_number)],
        ..WorkflowMetadata::default()
    };
    let summary = output.summary.trim();
    let body = format!(
        "Workspace-produced implementation for issue #{code_number}.\n\nSummary: {}\n\n{}",
        if summary.is_empty() {
            "(none)"
        } else {
            summary
        },
        render_metadata_block(&metadata)
    );
    CreatePullRequest {
        title: format!("Implement #{code_number}: {issue_title}"),
        body,
        source: BranchRef {
            repository_id: repo.clone(),
            branch: output.branch,
        },
        target: BranchRef {
            repository_id: repo,
            branch: if output.base_branch.trim().is_empty() {
                default_base_branch
            } else {
                output.base_branch
            },
        },
        labels: output.labels,
        assignees: Vec::new(),
    }
}

fn declared_bound_tools(
    manifest: &RoleManifest,
    bound_external_tools: Vec<BoundExternalTool>,
) -> Vec<BoundExternalTool> {
    manifest
        .external_tools
        .iter()
        .filter_map(|declared| {
            bound_external_tools
                .iter()
                .find(|tool| tool.id == declared.id)
                .cloned()
        })
        .collect()
}

fn runtime_external_tool_lines(tools: &[BoundExternalTool]) -> Vec<String> {
    let mut lines = vec![
        "Only the external tools listed in this section are bound and available for this run."
            .to_string(),
        "Declared tools not listed here are unavailable; do not claim to use them.".to_string(),
        "External tools do not grant workflow or Forge mutation authority beyond the authorized workflow actions above.".to_string(),
    ];
    if tools.is_empty() {
        lines.push("(no external tools are bound for this run)".to_string());
    } else {
        for tool in tools {
            lines.push(format!(
                "{} via {}: {}",
                tool.id, tool.provider, tool.description
            ));
            if !tool.constraints.is_empty() {
                lines.push(format!(
                    "{} constraints: {}",
                    tool.id,
                    tool.constraints.join("; ")
                ));
            }
            if tool.id.as_str() == CODING_WORKSPACE_TOOL_ID {
                lines.push(format!(
                    "{} rule: implementation PR creation must use this workspace-produced branch/head; do not choose PR-opening actions for code work without it.",
                    tool.id
                ));
            }
            if let Some(guidance) = &tool.guidance {
                lines.push(format!("{} guidance: {guidance}", tool.id));
            }
        }
    }
    lines
}

#[async_trait]
impl<F: Forge + ?Sized> Agent<F> for LlmRoleAgent {
    async fn service(&self, item: &WorkItem, tools: &RoleTools<'_, F>) -> Result<bool, AgentError> {
        let work_item_context = build_context(item, tools).await?;
        let context = self.user_context(&work_item_context);
        let decision = self.decide(item, &context).await?;

        if decision.action == NO_ACTION {
            return Ok(false);
        }

        let Some(tool) = self.tool_for_action(&decision.action) else {
            eprintln!(
                "harness-agents: role '{}' returned unauthorized action '{}', treating as no-action",
                self.manifest.id, decision.action
            );
            return Ok(false);
        };

        self.run_tool(item, tools, tool, &work_item_context).await
    }
}

#[cfg(test)]
#[path = "role_tests.rs"]
mod role_tests;

#[cfg(test)]
#[path = "role_external_tool_tests.rs"]
mod role_external_tool_tests;
