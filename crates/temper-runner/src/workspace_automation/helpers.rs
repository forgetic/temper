//! Routing, guidance, and stale-state helpers for workspace-backed automation.

use temper_forge_model::{Forge, RepositoryId};
use temper_workflow::{
    Effect, ExecutionContext, ExecutionError, Executor, RoleManifest, TransitionId,
    ValidatedWorkflow, VerdictId,
};

use super::WorkspaceAutomationOutcome;
use crate::CodingWorkspaceGuidance;
use crate::scan::AutomatedWorkItem;

/// Per-kind index of the first `CreateIssues` effect declared by `transition` in
/// `workflow`, if any.
///
/// The executor counts effect indices per kind, so the first (and, in practice,
/// only) `create_issues` effect on a transition is index `0`.
pub(super) fn create_issues_effect_index(
    workflow: &ValidatedWorkflow,
    transition: &TransitionId,
) -> Option<usize> {
    let declares_create_issues = workflow
        .transitions()
        .iter()
        .find(|candidate| &candidate.id == transition)?
        .effects
        .iter()
        .any(|effect| matches!(effect, Effect::CreateIssues { .. }));
    declares_create_issues.then_some(0)
}

/// Applies `routed` under the actor's authority with a content/PR-bound
/// execution context, mapping a stale-state failure to a quiet skip so the next
/// tick retries against fresh state instead of failing the whole tick.
pub(super) async fn run_routed<F: Forge + ?Sized>(
    workflow: &ValidatedWorkflow,
    forge: &F,
    context: ExecutionContext,
    repo: &RepositoryId,
    item: &AutomatedWorkItem,
    routed: &TransitionId,
) -> Result<WorkspaceAutomationOutcome, ExecutionError> {
    match Executor::with_context(workflow, forge, context)
        .execute(repo, item.target, routed, &item.actor)
        .await
    {
        Ok(_) => Ok(WorkspaceAutomationOutcome::Applied {
            routed: routed.clone(),
        }),
        Err(error) if is_stale(&error) => Ok(WorkspaceAutomationOutcome::Skipped {
            reason: "stale_no_op",
        }),
        Err(error) => Err(error),
    }
}

/// Builds the guidance a workspace receives for an automation run from the
/// actor role's charter/prompt and the declared external tool's guidance and
/// constraints, mirroring the role-decision path's guidance assembly.
pub(super) fn automation_guidance(
    actor: &RoleManifest,
    executor_id: &str,
) -> CodingWorkspaceGuidance {
    let role_guidance = actor
        .charter
        .iter()
        .chain(actor.prompt_extension.guidance.iter())
        .cloned()
        .collect::<Vec<_>>()
        .join("\n\n");
    let declared = actor
        .external_tools
        .iter()
        .find(|tool| tool.id.as_str() == executor_id);
    CodingWorkspaceGuidance {
        role_guidance: (!role_guidance.trim().is_empty()).then_some(role_guidance),
        tool_guidance: actor
            .prompt_extension
            .tool_guidance
            .clone()
            .or_else(|| declared.and_then(|tool| tool.guidance.clone())),
        tool_constraints: declared
            .map(|tool| tool.constraints.clone())
            .unwrap_or_default(),
    }
}

pub(super) fn undeclared_verdict_error(
    item: &AutomatedWorkItem,
    verdict: &VerdictId,
) -> ExecutionError {
    ExecutionError::Backend {
        message: format!(
            "coding workspace returned undeclared verdict `{verdict}` for automation on queue `{}`",
            item.queue
        ),
    }
}

fn is_stale(error: &ExecutionError) -> bool {
    matches!(
        error,
        ExecutionError::Precondition { .. }
            | ExecutionError::TargetMissing { .. }
            | ExecutionError::TargetStale { .. }
            | ExecutionError::Classification(_)
    )
}
