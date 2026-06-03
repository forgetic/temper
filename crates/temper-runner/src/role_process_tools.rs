//! Shared action execution helpers for process-backed role decisions.

use std::sync::Arc;

use temper_forge::{BranchRef, CreatePullRequest, Forge, ItemNumber};
use temper_workflow::{
    render_metadata_block, ArtifactKindId, ArtifactRef, ArtifactSource, Effect, ExecutionError,
    RoleManifest, ToolManifest, TransitionId, WorkflowMetadata,
};

use crate::{
    AgentError, BoundExternalTool, CodingWorkspace, CodingWorkspaceGuidance,
    CodingWorkspaceRepository, CodingWorkspaceRequest, CodingWorkspaceWorkItem,
    ExternalToolExecutors, RoleTools, WorkItem, WorkItemIdentity, CODING_WORKSPACE_TOOL_ID,
};

pub(crate) async fn build_work_item_context<F: Forge + ?Sized>(
    item: &WorkItem,
    tools: &RoleTools<'_, F>,
) -> Result<serde_json::Value, AgentError> {
    let artifact = match item.target {
        ArtifactSource::Issue { number } => tools.get_issue(number).await?.map(|issue| {
            serde_json::json!({
                "type": "issue",
                "number": number.get(),
                "title": issue.title,
                "body": issue.body,
                "labels": issue.labels,
                "state": format!("{:?}", issue.state),
            })
        }),
        ArtifactSource::PullRequest { number } => tools.get_pull_request(number).await?.map(|pr| {
            serde_json::json!({
                "type": "pull_request",
                "number": number.get(),
                "title": pr.title,
                "body": pr.body,
                "labels": pr.labels,
                "state": format!("{:?}", pr.state),
            })
        }),
    };

    let identity = WorkItemIdentity::new(
        tools.repo(),
        tools.role(),
        &item.queue,
        item.target,
        &item.kind,
    );

    Ok(serde_json::json!({
        "repository": tools.repo().as_str(),
        "role": tools.role().as_str(),
        "queue": item.queue.as_str(),
        "kind": item.kind.as_str(),
        "artifact": artifact,
        "observability": identity.to_json(),
    }))
}

pub(crate) async fn run_process_action<F: Forge + ?Sized>(
    manifest: &RoleManifest,
    bound_external_tools: &[BoundExternalTool],
    external_tool_executors: &ExternalToolExecutors,
    item: &WorkItem,
    tools: &RoleTools<'_, F>,
    tool: &ToolManifest,
    work_item_context: &serde_json::Value,
) -> Result<bool, AgentError> {
    if tool_creates_pull_request(tool) && declares_coding_workspace(manifest) {
        let Some(workspace) =
            coding_workspace(manifest, bound_external_tools, external_tool_executors)
        else {
            eprintln!(
                "temper-runner: role '{}' cannot run '{}' because coding_workspace is declared but not executable-bound",
                manifest.id, tool.name
            );
            return Ok(false);
        };
        return run_pull_request_create_tool(
            manifest,
            bound_external_tools,
            item,
            tools,
            tool,
            work_item_context,
            workspace,
        )
        .await;
    }
    run_or_ignore_stale(tools, item.target, &tool.transition).await
}

async fn run_pull_request_create_tool<F: Forge + ?Sized>(
    manifest: &RoleManifest,
    bound_external_tools: &[BoundExternalTool],
    item: &WorkItem,
    tools: &RoleTools<'_, F>,
    tool: &ToolManifest,
    work_item_context: &serde_json::Value,
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
    let context_json = serde_json::to_string_pretty(work_item_context)
        .unwrap_or_else(|_| work_item_context.to_string());
    let request = CodingWorkspaceRequest {
        repository: CodingWorkspaceRepository {
            id: repository.id.clone(),
            owner: repository.owner,
            name: repository.name,
            default_branch: repository.default_branch,
        },
        work_item: CodingWorkspaceWorkItem {
            role: manifest.id.clone(),
            queue: item.queue.clone(),
            kind: item.kind.clone(),
            target: item.target,
            context_json,
        },
        base_branch: base_branch.clone(),
        branch_hint: pr_branch_hint(&item.kind, number),
        correlation_key: correlation_key.clone(),
        guidance: workspace_guidance(manifest, bound_external_tools),
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
        .run_with_pull_request_create_at(item.target, &tool.transition, 0, correlation_key, input)
        .await
    {
        Ok(_) => Ok(true),
        Err(error) if stale_execution(&error) => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn run_or_ignore_stale<'a, F: Forge + ?Sized + 'a>(
    tools: &'a RoleTools<'_, F>,
    target: ArtifactSource,
    transition: &'a TransitionId,
) -> impl std::future::Future<Output = Result<bool, AgentError>> + 'a {
    async move {
        match tools.run(target, transition).await {
            Ok(_) => Ok(true),
            Err(error) if stale_execution(&error) => Ok(false),
            Err(error) => Err(error.into()),
        }
    }
}

fn stale_execution(error: &ExecutionError) -> bool {
    matches!(
        error,
        ExecutionError::Precondition { .. }
            | ExecutionError::TargetMissing { .. }
            | ExecutionError::Classification(_)
    )
}

fn declares_coding_workspace(manifest: &RoleManifest) -> bool {
    manifest
        .external_tools
        .iter()
        .any(|tool| tool.id.as_str() == CODING_WORKSPACE_TOOL_ID)
}

fn coding_workspace(
    manifest: &RoleManifest,
    bound_external_tools: &[BoundExternalTool],
    external_tool_executors: &ExternalToolExecutors,
) -> Option<Arc<dyn CodingWorkspace>> {
    let declared = bound_external_tools
        .iter()
        .find(|tool| tool.id.as_str() == CODING_WORKSPACE_TOOL_ID)?;
    external_tool_executors.coding_workspace_for(&manifest.id, &declared.id)
}

fn workspace_guidance(
    manifest: &RoleManifest,
    bound_external_tools: &[BoundExternalTool],
) -> CodingWorkspaceGuidance {
    let role_guidance = manifest
        .charter
        .iter()
        .chain(manifest.prompt_extension.guidance.iter())
        .cloned()
        .collect::<Vec<_>>()
        .join("\n\n");
    let tool_guidance = manifest.prompt_extension.tool_guidance.clone().or_else(|| {
        bound_external_tools
            .iter()
            .find(|tool| tool.id.as_str() == CODING_WORKSPACE_TOOL_ID)
            .and_then(|tool| tool.guidance.clone())
    });
    let tool_constraints = bound_external_tools
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
    repo: temper_forge::RepositoryId,
    code_number: ItemNumber,
    issue_title: &str,
    output: crate::CodingWorkspaceOutput,
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
