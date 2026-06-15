//! Shared action execution helpers for process-backed role decisions.

mod logging;
mod resolve;

use temper_forge_model::Forge;
use temper_workflow::{
    ArtifactSource, ExecutionError, ExecutionReport, RoleManifest, ToolManifest, TransitionId,
};

use crate::role_process_tools::logging::{
    log_action_dispatch, log_transition_custom, log_transition_error, log_transition_success,
    log_verdict_route,
};
use crate::role_process_tools::resolve::{
    ResolvedWorkspace, action_is_workspace_backed, allowed_verdicts, create_pull_request_count,
    workspace_executor, workspace_executor_hint, workspace_guidance,
};
use crate::workspace_request::{
    pr_branch_hint, pr_correlation_key, target_number, workspace_content_key,
    workspace_pull_request_input,
};
use crate::{
    AgentError, BoundExternalTool, CodingWorkspaceOutput, CodingWorkspaceRepository,
    CodingWorkspaceRequest, CodingWorkspaceWorkItem, ExternalToolExecutors, RoleTools, WorkItem,
    WorkItemIdentity,
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

    let identity = tools.work_item_identity(item);

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
    let identity = tools.work_item_identity(item);
    // An action is workspace-backed by declaration: it declares `outcomes` (the
    // verdict→transition routing) and/or creates a pull-request head. Dispatch
    // resolves the executor generically (any declared external tool the runner
    // bound a workspace for) rather than gating on the `coding_workspace` id.
    let needs_workspace = action_is_workspace_backed(tool);
    let workspace = needs_workspace
        .then(|| workspace_executor(manifest, bound_external_tools, external_tool_executors))
        .flatten();
    let executor_id = needs_workspace
        .then(|| {
            workspace
                .as_ref()
                .map(|resolved| resolved.tool_id)
                .or_else(|| workspace_executor_hint(manifest))
        })
        .flatten();
    log_action_dispatch(
        &identity,
        tool,
        needs_workspace,
        executor_id,
        needs_workspace.then_some(workspace.is_some()),
        if needs_workspace && workspace.is_none() {
            "no_op"
        } else {
            "dispatching"
        },
        if needs_workspace && workspace.is_none() {
            Some("required_executor_unavailable")
        } else {
            None
        },
    );
    if needs_workspace {
        let Some(resolved) = workspace else {
            return Ok(false);
        };
        return run_workspace_action(
            manifest,
            bound_external_tools,
            item,
            tools,
            tool,
            work_item_context,
            resolved,
        )
        .await;
    }
    run_or_ignore_stale(tools, item.target, &tool.transition, &identity).await
}

/// Runs a workspace-backed action: invoke the bound workspace, then route on
/// the returned verdict through the action's `outcomes`.
///
/// The action is workspace-backed by declaration (see
/// [`action_is_workspace_backed`]). With no verdict the action's own transition
/// runs and produces a pull-request head (the engineer `open_pr` default); a
/// verdict selects the declared outcome transition instead — an escalation, a
/// content-bearing rewrite (`set_body` / `attach_review`), or another mechanical
/// transition. The pull-request-create effect-shape checks apply only on the
/// no-verdict head route, so a verdict-routed review/triage action that declares
/// no `create_pull_request` effect dispatches its workspace and routes normally.
async fn run_workspace_action<F: Forge + ?Sized>(
    manifest: &RoleManifest,
    bound_external_tools: &[BoundExternalTool],
    item: &WorkItem,
    tools: &RoleTools<'_, F>,
    tool: &ToolManifest,
    work_item_context: &serde_json::Value,
    resolved: ResolvedWorkspace<'_>,
) -> Result<bool, AgentError> {
    let ResolvedWorkspace {
        tool_id: executor_tool_id,
        workspace,
        checkout,
    } = resolved;
    let identity = tools.work_item_identity(item);
    let identity = &identity;

    let number = target_number(item.target);
    let repository = match tools.get_repository().await? {
        Some(repository) => repository,
        None => {
            log_transition_custom(
                identity,
                &tool.transition,
                "failed",
                false,
                Some("repository_missing"),
                "not_checked",
            );
            return Err(AgentError::message(format!(
                "repository {} not found",
                tools.repo()
            )));
        }
    };
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
        guidance: workspace_guidance(manifest, bound_external_tools, executor_tool_id),
        allowed_verdicts: allowed_verdicts(tool),
        checkout,
    };
    let output = match workspace.produce_head(request).await {
        Ok(output) => output,
        Err(error) => {
            log_transition_custom(
                identity,
                &tool.transition,
                "failed",
                false,
                Some("external_executor_failed"),
                "not_checked",
            );
            return Err(AgentError::message(format!(
                "coding workspace failed: {error}"
            )));
        }
    };

    // Resolve the transition the workspace verdict routes to, if any. With no
    // verdict the action's own transition runs and the head produces a PR (the
    // default "produce a head" behavior). With a verdict, the engine runs the
    // declared outcome transition instead — e.g. the engineer escalates with
    // `needs_architect` instead of looping on an empty diff, or the reviewer
    // routes `approve` / `changes` / `escalate`.
    let routed = match &output.verdict {
        Some(verdict) => match tool.outcomes.get(verdict) {
            Some(transition) => transition.clone(),
            None => {
                log_transition_custom(
                    identity,
                    &tool.transition,
                    "failed",
                    false,
                    Some("external_executor_undeclared_verdict"),
                    "not_checked",
                );
                return Err(AgentError::message(format!(
                    "coding workspace returned undeclared verdict `{verdict}` for action '{}'",
                    tool.name
                )));
            }
        },
        None => tool.transition.clone(),
    };

    if routed == tool.transition {
        // No-verdict head route: the action's own transition opens a PR from the
        // workspace head. This is the engineer `open_pr` default. The
        // create-pull-request effect-shape and non-empty-diff guards apply only
        // here, so a verdict-routed action that declares no PR effect never trips
        // them.
        return run_pull_request_head(
            tools,
            item,
            tool,
            identity,
            base_branch,
            correlation_key,
            output,
        )
        .await;
    }

    // Verdict routed to a non-PR-create outcome transition (e.g. escalation, a
    // content-bearing rewrite, or an issue breakdown). The (possibly empty) head
    // is discarded; the routed transition applies its own effects. Any
    // agent-authored body / review body / children the workspace produced is
    // bound through the keyed runtime seam so the routed transition's `set_body`
    // / `attach_review` / `create_issues` effects can consume the work product.
    // An empty diff here is the escalation signal, not an error, so the head
    // guards do not apply.
    log_verdict_route(identity, &tool.transition, &routed, output.verdict.as_ref());

    // Breakdown route: the routed transition declares `create_issues` and the
    // workspace authored the dependent children. Bind them under the
    // deterministic content key so a retry resolves the same children rather
    // than duplicating them.
    if !output.children.is_empty()
        && let Some(effect_index) = tools.create_issues_effect_index(&routed)
    {
        let content_key = workspace_content_key(&item.kind, &routed, target_number(item.target));
        return run_or_ignore_stale_with(
            tools
                .run_with_create_issues_at(
                    item.target,
                    &routed,
                    effect_index,
                    content_key,
                    output.children,
                )
                .await,
            identity,
            &routed,
        );
    }

    if output.body.is_some() || output.review_body.is_some() {
        let content_key = workspace_content_key(&item.kind, &routed, target_number(item.target));
        return run_or_ignore_stale_with(
            tools
                .run_with_workspace_content(
                    item.target,
                    &routed,
                    content_key,
                    output.body,
                    output.review_body,
                )
                .await,
            identity,
            &routed,
        );
    }
    run_or_ignore_stale(tools, item.target, &routed, identity).await
}

/// Opens a pull request from the workspace head on the no-verdict default route.
///
/// This is the engineer `open_pr` path: the action declares exactly one
/// `create_pull_request` effect, targets an issue, and the workspace returns a
/// real, non-empty diff. The guards here are scoped to this route so that a
/// verdict-routed review/triage action — which declares no PR effect and always
/// routes through `outcomes` — never reaches them.
async fn run_pull_request_head<F: Forge + ?Sized>(
    tools: &RoleTools<'_, F>,
    item: &WorkItem,
    tool: &ToolManifest,
    identity: &WorkItemIdentity,
    base_branch: String,
    correlation_key: String,
    output: CodingWorkspaceOutput,
) -> Result<bool, AgentError> {
    if create_pull_request_count(tool) != 1 {
        log_transition_custom(
            identity,
            &tool.transition,
            "failed",
            false,
            Some("external_executor_unsupported_effect_shape"),
            "not_checked",
        );
        return Err(AgentError::message(format!(
            "coding workspace supports exactly one CreatePullRequest effect for action '{}'",
            tool.name
        )));
    }
    let ArtifactSource::Issue { number } = item.target else {
        log_transition_custom(
            identity,
            &tool.transition,
            "stale_no_op",
            true,
            Some("target_not_issue"),
            "not_checked",
        );
        return Ok(false);
    };
    let Some(issue) = tools.get_issue(number).await? else {
        log_transition_custom(
            identity,
            &tool.transition,
            "stale_no_op",
            true,
            Some("target_missing"),
            "not_checked",
        );
        return Ok(false);
    };
    // The workspace head must be a real, non-empty diff.
    if output.branch.trim().is_empty() {
        log_transition_custom(
            identity,
            &tool.transition,
            "failed",
            false,
            Some("external_executor_invalid_output"),
            "not_checked",
        );
        return Err(AgentError::message(
            "coding workspace returned an empty PR head branch",
        ));
    }
    if output.changed_files.is_empty() {
        log_transition_custom(
            identity,
            &tool.transition,
            "failed",
            false,
            Some("external_executor_invalid_output"),
            "not_checked",
        );
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
        Ok(report) => {
            log_transition_success(identity, &report);
            Ok(true)
        }
        Err(error) if stale_execution(&error) => {
            log_transition_error(identity, &tool.transition, &error, true);
            Ok(false)
        }
        Err(error) => {
            log_transition_error(identity, &tool.transition, &error, false);
            Err(error.into())
        }
    }
}

async fn run_or_ignore_stale<'a, F: Forge + ?Sized + 'a>(
    tools: &'a RoleTools<'_, F>,
    target: ArtifactSource,
    transition: &'a TransitionId,
    identity: &'a WorkItemIdentity,
) -> Result<bool, AgentError> {
    run_or_ignore_stale_with(tools.run(target, transition).await, identity, transition)
}

/// Maps an already-computed transition execution result to the runner's
/// stale/success/failure logging contract, mirroring [`run_or_ignore_stale`]
/// for callers that ran the transition through a content-binding seam.
fn run_or_ignore_stale_with(
    result: Result<ExecutionReport, ExecutionError>,
    identity: &WorkItemIdentity,
    transition: &TransitionId,
) -> Result<bool, AgentError> {
    match result {
        Ok(report) => {
            log_transition_success(identity, &report);
            Ok(true)
        }
        Err(error) if stale_execution(&error) => {
            log_transition_error(identity, transition, &error, true);
            Ok(false)
        }
        Err(error) => {
            log_transition_error(identity, transition, &error, false);
            Err(error.into())
        }
    }
}

fn stale_execution(error: &ExecutionError) -> bool {
    matches!(
        error,
        ExecutionError::Precondition { .. }
            | ExecutionError::TargetMissing { .. }
            | ExecutionError::TargetStale { .. }
            | ExecutionError::Classification(_)
    )
}
