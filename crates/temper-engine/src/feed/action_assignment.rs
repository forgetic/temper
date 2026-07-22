// SPDX-License-Identifier: MPL-2.0

mod guidance;

use self::guidance::pull_request_writable_guidance;
use temper_forge::{CiJobQuery, Forge, ForgeError, PullRequest, Repository, RepositoryId};
use temper_protocol_worker::{JobContext, PullRequestFreshness, RepoAccess};
use temper_runner::{ScanError, WorkItem};
use temper_workflow::{
    ArtifactSource, CompiledWorkflow, Effect, GateCondition, QueueManifest, ToolManifest,
};

pub(super) fn enrich_job_context_from_workflow(
    item: &WorkItem,
    workflow: &temper_workflow::ValidatedWorkflow,
    compiled: &CompiledWorkflow,
    repository: &Repository,
    context: &mut JobContext,
) -> Result<(), ScanError> {
    let selected = select_workflow_action(item, compiled)?;
    let checkout = checkout_capability_for_action(item, &selected)?;

    context.action = Some(selected.tool.name.clone());
    context.allowed_verdicts = allowed_verdicts(selected.tool);
    let source_number = match item.target {
        ArtifactSource::Issue { number } | ArtifactSource::PullRequest { number } => number.get(),
    };
    let correlation_key = context
        .workspace
        .as_ref()
        .map(|workspace| workspace.coordination_key.as_str());
    context.verdict_contracts = crate::verdict_contract::derive_resolved_verdict_contracts(
        workflow,
        selected.tool,
        &crate::verdict_contract::BranchResolutionContext {
            source_kind: item.kind.as_str(),
            source_number: Some(source_number),
            source_metadata: &context.source_metadata,
            repository_default: &repository.default_branch,
            correlation_key,
        },
    )
    .map_err(invalid_workflow_scan)?;
    context.checkout_capability = Some(checkout);
    context.guidance = selected.guidance;
    Ok(())
}

struct SelectedWorkflowAction<'a> {
    tool: &'a ToolManifest,
    checkout: Option<String>,
    guidance: Option<String>,
}

fn select_workflow_action<'a>(
    item: &WorkItem,
    compiled: &'a CompiledWorkflow,
) -> Result<SelectedWorkflowAction<'a>, ScanError> {
    let role = compiled.role(&item.role).ok_or_else(|| {
        invalid_workflow_scan(format!("role `{}` is not compiled", item.role.as_str()))
    })?;
    let queue = compiled
        .queues()
        .iter()
        .find(|queue| queue.id.as_str() == item.queue.as_str())
        .ok_or_else(|| {
            invalid_workflow_scan(format!("queue `{}` is not compiled", item.queue.as_str()))
        })?;

    let explicit = matching_queue_actions(queue, item);
    match explicit.as_slice() {
        [action] => {
            let tool = role
                .tools
                .iter()
                .find(|tool| tool.transition.as_str() == action.action.as_str())
                .ok_or_else(|| {
                    invalid_workflow_scan(format!(
                        "queue `{}` assigns action `{}` to role `{}`, but that role has no matching compiled tool",
                        item.queue.as_str(),
                        action.action.as_str(),
                        item.role.as_str()
                    ))
                })?;
            if tool.artifact.as_str() != item.kind.as_str() {
                return Err(invalid_workflow_scan(format!(
                    "queue `{}` assigns action `{}` for artifact `{}`, but the action operates on `{}`",
                    item.queue.as_str(),
                    action.action.as_str(),
                    item.kind.as_str(),
                    tool.artifact.as_str()
                )));
            }
            return Ok(SelectedWorkflowAction {
                tool,
                checkout: action.checkout.clone(),
                guidance: action.guidance.clone(),
            });
        }
        [] if !queue.actions.is_empty() => {
            return Err(invalid_workflow_scan(format!(
                "queue `{}` declares role-worker actions, but none match role `{}` and artifact `{}`",
                item.queue.as_str(),
                item.role.as_str(),
                item.kind.as_str()
            )));
        }
        [] => {}
        actions => {
            let names = actions
                .iter()
                .map(|action| action.action.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            return Err(invalid_workflow_scan(format!(
                "queue `{}` has ambiguous action assignments for role `{}` and artifact `{}`: {names}",
                item.queue.as_str(),
                item.role.as_str(),
                item.kind.as_str()
            )));
        }
    }

    let candidates = role
        .tools
        .iter()
        .filter(|tool| action_is_workspace_backed(tool) && tool.artifact == item.kind)
        .collect::<Vec<_>>();
    match candidates.as_slice() {
        [tool] => Ok(SelectedWorkflowAction {
            tool,
            checkout: None,
            guidance: None,
        }),
        [] => Err(invalid_workflow_scan(format!(
            "queue `{}` role `{}` artifact `{}` did not declare an action and has no unambiguous workspace-backed fallback",
            item.queue.as_str(),
            item.role.as_str(),
            item.kind.as_str()
        ))),
        tools => {
            let names = tools
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            Err(invalid_workflow_scan(format!(
                "queue `{}` role `{}` artifact `{}` has ambiguous workspace-backed fallback actions: {names}",
                item.queue.as_str(),
                item.role.as_str(),
                item.kind.as_str()
            )))
        }
    }
}

fn matching_queue_actions<'a>(
    queue: &'a QueueManifest,
    item: &WorkItem,
) -> Vec<&'a temper_workflow::QueueAction> {
    queue
        .actions
        .iter()
        .filter(|action| action.role.as_str() == item.role.as_str())
        .filter(|action| match action.artifact.as_ref() {
            Some(artifact) => artifact.as_str() == item.kind.as_str(),
            None => true,
        })
        .collect()
}

fn checkout_capability_for_action(
    item: &WorkItem,
    selected: &SelectedWorkflowAction<'_>,
) -> Result<String, ScanError> {
    if let Some(checkout) = &selected.checkout {
        if supported_checkout_capability(checkout) {
            return Ok(checkout.clone());
        }
        return Err(invalid_workflow_scan(format!(
            "assigned action `{}` declares unsupported checkout capability `{checkout}`",
            selected.tool.name
        )));
    }

    if create_pull_request_count(selected.tool) > 0 {
        return Ok("writable".to_string());
    }
    match item.target {
        ArtifactSource::Issue { .. } => Ok("read_only".to_string()),
        ArtifactSource::PullRequest { .. } if !selected.tool.outcomes.is_empty() => {
            Ok("pull_request_read_only".to_string())
        }
        ArtifactSource::PullRequest { .. } => Err(invalid_workflow_scan(format!(
            "assigned PR action `{}` has no declared verdict outcomes or create_pull_request effect; declare its queue action checkout explicitly",
            selected.tool.name
        ))),
    }
}

fn supported_checkout_capability(checkout: &str) -> bool {
    matches!(
        checkout,
        "writable" | "read_only" | "pull_request_read_only" | "pull_request_writable"
    )
}

fn invalid_workflow_scan(message: impl Into<String>) -> ScanError {
    ScanError::InvalidWorkflow(message.into())
}

/// Turns a generic PR job into a writable PR-head fix job by pointing the
/// manifest's primary repo at the PR's actual head branch and attaching guidance
/// for the selected action.
pub(super) async fn enrich_pull_request_writable_job<F: Forge + ?Sized>(
    forge: &F,
    repo: &RepositoryId,
    item: &WorkItem,
    compiled: &CompiledWorkflow,
    context: &mut JobContext,
    preloaded: Option<&PullRequest>,
) -> Result<(), ScanError> {
    let ArtifactSource::PullRequest { number } = item.target else {
        return Err(invalid_workflow_scan(format!(
            "assigned action `{}` requested pull_request_writable checkout for non-PR target {:?}",
            context.action.as_deref().unwrap_or("<unknown>"),
            item.target
        )));
    };
    let pull_request = match preloaded {
        Some(pull_request) => pull_request.clone(),
        None => forge
            .get_pull_request_by_number(repo, number)
            .await?
            .ok_or_else(|| {
                ScanError::Forge(ForgeError::NotFound(format!("pull request {number}")))
            })?,
    };
    let head_branch = pull_request.source.branch.clone();
    let base_branch = pull_request.target.branch.clone();
    if head_branch.trim().is_empty() {
        return Err(invalid_workflow_scan(format!(
            "assigned action `{}` requested pull_request_writable checkout, but PR {number} has no head branch",
            context.action.as_deref().unwrap_or("<unknown>")
        )));
    }

    if let Some(workspace) = context.workspace.as_mut() {
        if let Some(primary) = workspace.repos.first_mut() {
            primary.access = RepoAccess::Writable;
            primary.branch_hint = Some(head_branch.clone());
            if !base_branch.trim().is_empty() {
                primary.base_branch = base_branch.clone();
                primary.default_branch = base_branch.clone();
            }
        }
    }

    let query = CiJobQuery {
        pull_request_id: Some(pull_request.id.clone()),
        commit_sha: pull_request.head_sha.clone(),
        ..CiJobQuery::default()
    };
    let action = context
        .action
        .clone()
        .unwrap_or_else(|| "assigned action".to_string());
    context.pull_request_freshness = Some(pull_request_freshness(
        repo,
        item,
        compiled,
        context,
        &pull_request,
        &action,
    ));
    let guidance = pull_request_writable_guidance(
        forge,
        repo,
        compiled,
        item,
        &query,
        &action,
        &pull_request,
        &head_branch,
        &base_branch,
    )
    .await;
    append_guidance(context, guidance);
    Ok(())
}

fn pull_request_freshness(
    repo: &RepositoryId,
    item: &WorkItem,
    compiled: &CompiledWorkflow,
    context: &JobContext,
    pull_request: &PullRequest,
    action: &str,
) -> PullRequestFreshness {
    let queue = compiled
        .queues()
        .iter()
        .find(|queue| queue.id.as_str() == item.queue.as_str());
    PullRequestFreshness {
        repository_id: repo.as_str().to_string(),
        repo: context.repo.clone(),
        role: context.role.clone(),
        queue: item.queue.as_str().to_string(),
        action: action.to_string(),
        number: pull_request.number.get(),
        pull_request_id: pull_request.id.as_str().to_string(),
        head_sha: pull_request.head_sha.clone(),
        queue_condition: queue.and_then(|queue| queue.condition.as_ref().and_then(condition_token)),
        queue_labels: queue
            .map(|queue| {
                queue
                    .labels
                    .iter()
                    .map(|label| label.as_str().to_string())
                    .collect()
            })
            .unwrap_or_default(),
    }
}

fn condition_token(condition: &GateCondition) -> Option<String> {
    let token = match condition {
        GateCondition::CiPassed => "ci_passed",
        GateCondition::CiFailed => "ci_failed",
        GateCondition::ReviewApproved => "review_approved",
        GateCondition::ReviewChangesRequested => "review_changes_requested",
        GateCondition::DependenciesResolved
        | GateCondition::LabelPresent(_)
        | GateCondition::StateEquals { .. } => return None,
    };
    Some(token.to_string())
}

fn append_guidance(context: &mut JobContext, generated: String) {
    context.guidance = match context.guidance.take() {
        Some(existing) if !existing.trim().is_empty() => Some(format!("{existing}\n\n{generated}")),
        _ => Some(generated),
    };
}

fn action_is_workspace_backed(tool: &ToolManifest) -> bool {
    !tool.outcomes.is_empty()
        || tool
            .effects
            .iter()
            .any(|effect| matches!(effect, Effect::CreatePullRequest { .. }))
}

fn create_pull_request_count(tool: &ToolManifest) -> usize {
    tool.effects
        .iter()
        .filter(|effect| matches!(effect, Effect::CreatePullRequest { .. }))
        .count()
}

fn allowed_verdicts(tool: &ToolManifest) -> Vec<String> {
    tool.outcomes
        .keys()
        .map(|verdict| verdict.as_str().to_string())
        .collect()
}
