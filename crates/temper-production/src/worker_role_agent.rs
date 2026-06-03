//! Role-agent construction for production workers.

use std::sync::Arc;

use temper_forge::{Forge, ReviewDecision};
use temper_runner::{Agent, ExternalToolExecutors, RunnerConfig, WorkflowRoleDecisionProcessAgent};
use temper_workflow::{CompiledWorkflow, Effect, QueueId, RoleId, RoleManifest, ToolManifest};

use crate::pr_diff_guard::{GuardRole, PullRequestDiffGuard};
use crate::worker_args::WorkerArgs;

pub(crate) fn build_role_agent(
    args: &WorkerArgs,
    compiled: &CompiledWorkflow,
    config: &RunnerConfig,
    external_tool_executors: ExternalToolExecutors,
    _role_id: &RoleId,
    role_manifest: &RoleManifest,
) -> Result<Arc<dyn Agent<dyn Forge>>, String> {
    let Some(process_config) = args.role_decision_process.clone() else {
        return Err(
            "role decision process is required; configure --role-decision-command or TEMPER_WORKER_ROLE_DECISION_COMMAND"
                .to_string(),
        );
    };
    config
        .validate_external_tool_bindings(compiled)
        .map_err(|error| error.to_string())?;
    external_tool_executors
        .validate(compiled, config)
        .map_err(|error| error.to_string())?;
    let bound_tools = config
        .bound_external_tools_for(role_manifest)
        .map_err(|error| error.to_string())?;
    let agent = Arc::new(
        WorkflowRoleDecisionProcessAgent::with_bound_external_tools_and_executors(
            compiled.name(),
            role_manifest.clone(),
            process_config,
            bound_tools,
            external_tool_executors,
        )
        .map_err(|error| error.to_string())?,
    ) as Arc<dyn Agent<dyn Forge>>;
    Ok(guard_agent_if_needed(args, compiled, role_manifest, agent))
}

fn guard_agent_if_needed(
    args: &WorkerArgs,
    compiled: &CompiledWorkflow,
    role: &RoleManifest,
    agent: Arc<dyn Agent<dyn Forge>>,
) -> Arc<dyn Agent<dyn Forge>> {
    if args.allow_bookkeeping_only_pr {
        return agent;
    }
    let Some(guard_role) = guard_role_for_manifest(compiled, role) else {
        return agent;
    };
    Arc::new(PullRequestDiffGuard::new(
        agent,
        guard_role,
        args.forgejo.base_url.clone(),
        args.forgejo.token.clone(),
    )) as Arc<dyn Agent<dyn Forge>>
}

pub(crate) fn guard_role_for_manifest(
    compiled: &CompiledWorkflow,
    role: &RoleManifest,
) -> Option<GuardRole> {
    let approval_tool = role.tools.iter().find(|tool| {
        tool.effects.iter().any(|effect| {
            matches!(
                effect,
                Effect::SubmitReview {
                    decision: ReviewDecision::Approved
                }
            )
        })
    });
    let request_changes_tool = role.tools.iter().find(|tool| {
        tool.effects.iter().any(|effect| {
            matches!(
                effect,
                Effect::SubmitReview {
                    decision: ReviewDecision::ChangesRequested
                }
            )
        })
    });
    if let (Some(approval), Some(request_changes)) = (approval_tool, request_changes_tool) {
        let queues = guard_queues_for_tool(compiled, role, approval);
        if !queues.is_empty() {
            return Some(GuardRole::Reviewer {
                request_changes: request_changes.transition.clone(),
                queues,
            });
        }
    }

    let merge_tool = role.tools.iter().find(|tool| {
        tool.effects
            .iter()
            .any(|effect| matches!(effect, Effect::MergePullRequest))
    });
    merge_tool
        .map(|tool| guard_queues_for_tool(compiled, role, tool))
        .filter(|queues| !queues.is_empty())
        .map(|queues| GuardRole::Owner { queues })
}

fn guard_queues_for_tool(
    compiled: &CompiledWorkflow,
    role: &RoleManifest,
    tool: &ToolManifest,
) -> Vec<QueueId> {
    let removed_labels = tool
        .effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::RemoveLabel(label) => Some(label),
            _ => None,
        })
        .collect::<Vec<_>>();

    compiled
        .queues()
        .iter()
        .filter(|queue| role.queues.contains(&queue.id))
        .filter(|queue| queue.artifacts.contains(&tool.artifact))
        .filter(|queue| {
            removed_labels.iter().any(|label| {
                queue.labels.contains(label)
                    || queue
                        .any_of
                        .iter()
                        .any(|label_set| label_set.labels.contains(label))
            })
        })
        .map(|queue| queue.id.clone())
        .collect()
}
