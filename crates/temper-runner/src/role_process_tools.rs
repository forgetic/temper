//! Shared action execution helpers for process-backed role decisions.

use std::sync::Arc;

use temper_forge::Forge;
use temper_workflow::{
    ArtifactSource, Effect, ExecutionError, ExecutionReport, RoleManifest, ToolManifest,
    TransitionId, VerdictId,
};

use crate::workspace_request::{
    pr_branch_hint, pr_correlation_key, target_number, workspace_content_key,
    workspace_pull_request_input,
};
use crate::{
    execution_error_diagnostic_classes, execution_error_failure_class,
    postcondition_outcome_for_error, render_action_dispatch_event,
    render_transition_execution_event, ActionDispatchEvent, AgentError, BoundExternalTool,
    CodingWorkspace, CodingWorkspaceGuidance, CodingWorkspaceOutput, CodingWorkspaceRepository,
    CodingWorkspaceRequest, CodingWorkspaceWorkItem, ExternalToolExecutors, RoleTools,
    TransitionExecutionEvent, WorkItem, WorkItemIdentity, WorkspaceCheckout,
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
    if !output.children.is_empty() {
        if let Some(effect_index) = tools.create_issues_effect_index(&routed) {
            let content_key =
                workspace_content_key(&item.kind, &routed, target_number(item.target));
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

fn log_verdict_route(
    identity: &WorkItemIdentity,
    action_transition: &TransitionId,
    routed: &TransitionId,
    verdict: Option<&VerdictId>,
) {
    let verdict = verdict.map(VerdictId::as_str).unwrap_or("");
    eprintln!(
        "{}",
        render_action_dispatch_event(&ActionDispatchEvent {
            identity,
            selected_action: action_transition.as_str(),
            transition: routed,
            external_executor_required: true,
            external_executor_id: None,
            external_executor_available: Some(true),
            outcome: "verdict_routed",
            no_op_reason: (!verdict.is_empty()).then_some(verdict),
        })
    );
}

fn run_or_ignore_stale<'a, F: Forge + ?Sized + 'a>(
    tools: &'a RoleTools<'_, F>,
    target: ArtifactSource,
    transition: &'a TransitionId,
    identity: &'a WorkItemIdentity,
) -> impl std::future::Future<Output = Result<bool, AgentError>> + 'a {
    async move { run_or_ignore_stale_with(tools.run(target, transition).await, identity, transition) }
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

fn log_action_dispatch(
    identity: &WorkItemIdentity,
    tool: &ToolManifest,
    external_executor_required: bool,
    external_executor_id: Option<&str>,
    external_executor_available: Option<bool>,
    outcome: &str,
    no_op_reason: Option<&str>,
) {
    eprintln!(
        "{}",
        render_action_dispatch_event(&ActionDispatchEvent {
            identity,
            selected_action: &tool.name,
            transition: &tool.transition,
            external_executor_required,
            external_executor_id,
            external_executor_available,
            outcome,
            no_op_reason,
        })
    );
}

fn log_transition_success(identity: &WorkItemIdentity, report: &ExecutionReport) {
    eprintln!(
        "{}",
        render_transition_execution_event(&TransitionExecutionEvent {
            identity,
            transition: &report.transition,
            outcome: "mutated",
            stale_work: false,
            effects: &report.applied,
            failure_class: None,
            diagnostic_classes: Vec::new(),
            postcondition_outcome: "passed",
        })
    );
}

fn log_transition_error(
    identity: &WorkItemIdentity,
    transition: &TransitionId,
    error: &ExecutionError,
    stale_work: bool,
) {
    let failure_class = execution_error_failure_class(error);
    eprintln!(
        "{}",
        render_transition_execution_event(&TransitionExecutionEvent {
            identity,
            transition,
            outcome: if stale_work { "stale_no_op" } else { "failed" },
            stale_work,
            effects: &[],
            failure_class: Some(failure_class.as_str()),
            diagnostic_classes: execution_error_diagnostic_classes(error),
            postcondition_outcome: postcondition_outcome_for_error(error),
        })
    );
}

fn log_transition_custom(
    identity: &WorkItemIdentity,
    transition: &TransitionId,
    outcome: &str,
    stale_work: bool,
    failure_class: Option<&str>,
    postcondition_outcome: &str,
) {
    eprintln!(
        "{}",
        render_transition_execution_event(&TransitionExecutionEvent {
            identity,
            transition,
            outcome,
            stale_work,
            effects: &[],
            failure_class,
            diagnostic_classes: Vec::new(),
            postcondition_outcome,
        })
    );
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

/// A workspace executor resolved for an action, plus the executor id (declared
/// external-tool id) it was bound under, for observability and guidance lookup,
/// and the checkout capability the runner bound it with.
struct ResolvedWorkspace<'a> {
    tool_id: &'a str,
    workspace: Arc<dyn CodingWorkspace>,
    checkout: WorkspaceCheckout,
}

/// Whether `tool`'s action is backed by a workspace executor.
///
/// An action is workspace-backed when it **declares** workspace behavior, not
/// when its effects happen to create a pull request. The declaration is the
/// action's `outcomes` map: a workspace-backed action runs its executor and
/// routes the returned verdict through `outcomes`. The create-pull-request head
/// remains workspace-backed as the no-verdict default (the engineer `open_pr`
/// path), so an action that creates a PR is still treated as workspace-backed
/// even with no declared `outcomes`.
///
/// This replaces the earlier effect-shape inference (`creates a PR`), which made
/// a verdict-routed review action (only `remove_label`, no `create_pull_request`)
/// silently skip its workspace, and forced a bare `create_pull_request` marker
/// onto triage actions that never open a PR.
fn action_is_workspace_backed(tool: &ToolManifest) -> bool {
    !tool.outcomes.is_empty() || create_pull_request_count(tool) > 0
}

/// Resolves a workspace executor for `manifest`'s role from the runner-bound
/// external tools, keyed by executor id.
///
/// This is role-agnostic: it returns the first declared external tool that is
/// both bound and backed by a registered workspace executor, rather than
/// matching a hardcoded `coding_workspace` id.
fn workspace_executor<'a>(
    manifest: &'a RoleManifest,
    bound_external_tools: &[BoundExternalTool],
    external_tool_executors: &ExternalToolExecutors,
) -> Option<ResolvedWorkspace<'a>> {
    manifest.external_tools.iter().find_map(|declared| {
        if !bound_external_tools
            .iter()
            .any(|bound| bound.id.as_str() == declared.id.as_str())
        {
            return None;
        }
        let workspace = external_tool_executors.workspace_for(&manifest.id, &declared.id)?;
        let checkout = external_tool_executors
            .checkout_for(&manifest.id, &declared.id)
            .unwrap_or_default();
        Some(ResolvedWorkspace {
            tool_id: declared.id.as_str(),
            workspace,
            checkout,
        })
    })
}

/// Best-effort executor id for observability when no workspace executor is
/// bound: the role's first declared external tool, if any.
fn workspace_executor_hint(manifest: &RoleManifest) -> Option<&str> {
    manifest
        .external_tools
        .first()
        .map(|declared| declared.id.as_str())
}

fn workspace_guidance(
    manifest: &RoleManifest,
    bound_external_tools: &[BoundExternalTool],
    executor_tool_id: &str,
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
            .find(|tool| tool.id.as_str() == executor_tool_id)
            .and_then(|tool| tool.guidance.clone())
    });
    let tool_constraints = bound_external_tools
        .iter()
        .find(|tool| tool.id.as_str() == executor_tool_id)
        .map(|tool| tool.constraints.clone())
        .unwrap_or_default();
    CodingWorkspaceGuidance {
        role_guidance: (!role_guidance.trim().is_empty()).then_some(role_guidance),
        tool_guidance,
        tool_constraints,
    }
}

fn create_pull_request_count(tool: &ToolManifest) -> usize {
    tool.effects
        .iter()
        .filter(|effect| matches!(effect, Effect::CreatePullRequest { .. }))
        .count()
}

/// The verdict vocabulary an action declares: the keys of its `outcomes` map,
/// as plain strings, for the workspace request and context. Empty for a pure
/// head action with no declared `outcomes`.
fn allowed_verdicts(tool: &ToolManifest) -> Vec<String> {
    tool.outcomes
        .keys()
        .map(|verdict| verdict.as_str().to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use temper_workflow::{ArtifactKindId, LabelId, TransitionId, VerdictId};

    fn tool(name: &str, effects: Vec<Effect>, outcomes: Vec<(&str, &str)>) -> ToolManifest {
        ToolManifest {
            name: name.to_string(),
            transition: TransitionId::new(name),
            artifact: ArtifactKindId::new("artifact"),
            requires_gates: Vec::new(),
            effects,
            outcomes: outcomes
                .into_iter()
                .map(|(verdict, transition)| {
                    (VerdictId::new(verdict), TransitionId::new(transition))
                })
                .collect::<BTreeMap<_, _>>(),
        }
    }

    #[test]
    fn verdict_routed_action_without_pr_effect_is_workspace_backed() {
        // `review_pr`: only `remove_label`, no `create_pull_request`, but it
        // declares `outcomes`. It must be workspace-backed by declaration.
        let review_pr = tool(
            "review_pr",
            vec![Effect::RemoveLabel(LabelId::new("needs-reviewer"))],
            vec![
                ("approve", "approve_review"),
                ("changes", "request_changes_with_review"),
                ("escalate", "request_architect_input"),
            ],
        );
        assert!(action_is_workspace_backed(&review_pr));
    }

    #[test]
    fn pr_head_action_is_workspace_backed() {
        // `open_pr`: the engineer head path declares a real `create_pull_request`
        // (here also an `outcomes` escalation, but the PR effect alone suffices).
        let open_pr = tool(
            "open_pr",
            vec![
                Effect::AddLabel(LabelId::new("in-progress")),
                Effect::CreatePullRequest {
                    correlation_key: None,
                },
            ],
            vec![("needs_architect", "request_code_architect_input")],
        );
        assert!(action_is_workspace_backed(&open_pr));

        // The same head path with no declared outcomes is still workspace-backed
        // by its create-pull-request effect.
        let plain_head = tool(
            "open_pr",
            vec![Effect::CreatePullRequest {
                correlation_key: None,
            }],
            Vec::new(),
        );
        assert!(action_is_workspace_backed(&plain_head));
    }

    #[test]
    fn allowed_verdicts_are_the_declared_outcome_keys() {
        // A triage action declaring two outcomes surfaces exactly those verdict
        // keys, sorted by the BTreeMap's ordering, for the workspace request.
        let triage = tool(
            "triage_intake",
            vec![Effect::RemoveLabel(LabelId::new("untriaged"))],
            vec![
                ("ready_code", "mark_ready_code"),
                ("needs_breakdown", "break_down_intake"),
            ],
        );
        assert_eq!(
            allowed_verdicts(&triage),
            vec!["needs_breakdown".to_string(), "ready_code".to_string()],
        );

        // A pure head action with no declared outcomes surfaces an empty
        // vocabulary, so the provider defers verdict validation to the runner.
        let open_pr = tool(
            "open_pr",
            vec![Effect::CreatePullRequest {
                correlation_key: None,
            }],
            Vec::new(),
        );
        assert!(allowed_verdicts(&open_pr).is_empty());
    }

    #[test]
    fn plain_mechanical_action_is_not_workspace_backed() {
        // A mechanical action: no `outcomes`, no `create_pull_request`.
        let mechanical = tool(
            "approve_review",
            vec![
                Effect::RemoveLabel(LabelId::new("needs-reviewer")),
                Effect::AddLabel(LabelId::new("landing")),
            ],
            Vec::new(),
        );
        assert!(!action_is_workspace_backed(&mechanical));
    }
}
