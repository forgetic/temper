//! Workspace-backed queue automation (ADR 0022 §D).
//!
//! The LLM role-decision path can bind a workspace executor to an action, run
//! it, and route on its verdict. This module gives the queue-automation path the
//! same ability without an upfront LLM classification: when a queue automation
//! declares an `executor`, the mechanical worker invokes the workspace bound for
//! the automation's actor, then routes the returned verdict through the
//! automation's `outcomes` map and applies the routed transition under the
//! actor's authority. The real work and any escalation stay inside the
//! workspace; the engine still owns transition legality and effect application.
//!
//! Leasing differs from the decision-driven path on purpose. A role worker
//! leases the artifact before invoking its workspace so two role workers do not
//! pick up the same item. The mechanical worker holds no per-item lease here:
//! idempotency comes from workflow state. The primary transition (or the routed
//! outcome) removes the queue's activating label, so a completed item no longer
//! matches on the next scan, and the deterministic correlation keys make a
//! repeated PR-create or content write a no-op. A workspace that is mid-flight
//! when the next tick fires is re-invoked; providers keep `produce_head`
//! idempotent for the same correlation key, exactly as on the decision path.

mod helpers;
#[cfg(test)]
mod tests;

use temper_forge::{Forge, RepositoryId};
use temper_workflow::{
    ArtifactSource, CompiledWorkflow, ExecutionContext, ExecutionError, TransitionId,
    ValidatedWorkflow,
};

use crate::scan::AutomatedWorkItem;
use crate::workspace_request::{
    pr_branch_hint, pr_correlation_key, target_number, workspace_content_key,
    workspace_pull_request_input,
};
use crate::{
    CodingWorkspaceRepository, CodingWorkspaceRequest, CodingWorkspaceWorkItem,
    ExternalToolExecutors,
};

use helpers::{
    automation_guidance, create_issues_effect_index, run_routed, undeclared_verdict_error,
};

/// Outcome of servicing one workspace-backed automated item.
pub(crate) enum WorkspaceAutomationOutcome {
    /// The workspace ran and a transition was applied; `routed` is the
    /// transition the verdict selected (the primary transition when the
    /// workspace returned no verdict).
    Applied { routed: TransitionId },
    /// Nothing was applied this tick for an expected, retry-on-next-tick reason
    /// (no executor bound, the target is gone, or the routed transition no
    /// longer applies). `reason` is a short stable token for observability.
    Skipped { reason: &'static str },
}

/// Invokes the workspace bound for `item.actor` under `item.executor`, then
/// routes on the returned verdict through `item.outcomes` and applies the routed
/// transition under the actor's authority.
pub(crate) async fn execute_workspace_automation<F: Forge + ?Sized>(
    workflow: &ValidatedWorkflow,
    compiled: &CompiledWorkflow,
    executors: &ExternalToolExecutors,
    forge: &F,
    repo: &RepositoryId,
    item: &AutomatedWorkItem,
) -> Result<WorkspaceAutomationOutcome, ExecutionError> {
    let Some(executor_id) = &item.executor else {
        return Ok(WorkspaceAutomationOutcome::Skipped {
            reason: "no_executor_declared",
        });
    };
    let Some(workspace) = executors.workspace_for(&item.actor, executor_id) else {
        // The automation declares a workspace executor but the runner bound
        // none (e.g. the coding-workspace env is unset). Stay quiet and let a
        // later tick retry once a binding exists, mirroring the role path's
        // no-op when a required executor is unavailable.
        return Ok(WorkspaceAutomationOutcome::Skipped {
            reason: "executor_unavailable",
        });
    };
    let checkout = executors
        .checkout_for(&item.actor, executor_id)
        .unwrap_or_default();
    let Some(actor) = compiled.role(&item.actor) else {
        return Ok(WorkspaceAutomationOutcome::Skipped {
            reason: "actor_role_missing",
        });
    };

    let ArtifactSource::Issue { number } = item.target else {
        // Today the single configured workspace produces a code head for an
        // issue. A pull-request-targeted workspace automation is not expressible
        // yet; skip rather than fail so an unexpected target does not wedge the
        // queue.
        return Ok(WorkspaceAutomationOutcome::Skipped {
            reason: "target_not_issue",
        });
    };
    let Some(issue) = forge.get_issue_by_number(repo, number).await? else {
        return Ok(WorkspaceAutomationOutcome::Skipped {
            reason: "target_missing",
        });
    };
    let Some(repository) = forge.get_repository(repo).await? else {
        return Err(ExecutionError::Backend {
            message: format!("repository {repo} not found"),
        });
    };

    let base_branch = if repository.default_branch.trim().is_empty() {
        "main".to_string()
    } else {
        repository.default_branch.clone()
    };
    let correlation_key = pr_correlation_key(&item.kind, number);
    let context_json = serde_json::to_string_pretty(&serde_json::json!({
        "repository": repo.as_str(),
        "role": item.actor.as_str(),
        "queue": item.queue.as_str(),
        "kind": item.kind.as_str(),
        "artifact": {
            "type": "issue",
            "number": number.get(),
            "title": issue.title,
            "body": issue.body,
            "labels": issue.labels,
            "state": format!("{:?}", issue.state),
        },
    }))
    .unwrap_or_default();
    let request = CodingWorkspaceRequest {
        repository: CodingWorkspaceRepository {
            id: repository.id.clone(),
            owner: repository.owner,
            name: repository.name,
            default_branch: repository.default_branch,
        },
        work_item: CodingWorkspaceWorkItem {
            role: item.actor.clone(),
            queue: item.queue.clone(),
            kind: item.kind.clone(),
            target: item.target,
            context_json,
        },
        base_branch: base_branch.clone(),
        branch_hint: pr_branch_hint(&item.kind, number),
        correlation_key: correlation_key.clone(),
        guidance: automation_guidance(actor, executor_id.as_str()),
        allowed_verdicts: item
            .outcomes
            .keys()
            .map(|verdict| verdict.as_str().to_string())
            .collect(),
        checkout,
    };
    let output =
        workspace
            .produce_head(request)
            .await
            .map_err(|error| ExecutionError::Backend {
                message: format!("coding workspace failed: {error}"),
            })?;

    // Resolve the transition the verdict routes to. No verdict keeps the
    // automation's own transition (the head produces a PR); a verdict selects
    // the declared outcome transition or fails if undeclared.
    let routed = match &output.verdict {
        Some(verdict) => match item.outcomes.get(verdict) {
            Some(transition) => transition.clone(),
            None => return Err(undeclared_verdict_error(item, verdict)),
        },
        None => item.transition.clone(),
    };
    let routes_to_pr_create = routed == item.transition;

    if routes_to_pr_create {
        if output.branch.trim().is_empty() {
            return Err(ExecutionError::Backend {
                message: "coding workspace returned an empty PR head branch".to_string(),
            });
        }
        if output.changed_files.is_empty() {
            return Err(ExecutionError::Backend {
                message: "coding workspace returned no changed files for PR head".to_string(),
            });
        }
        let input =
            workspace_pull_request_input(repo.clone(), number, &issue.title, output, base_branch);
        let mut context = ExecutionContext::new();
        context.set_pull_request_create_at(item.transition.clone(), 0, input);
        context.set_pull_request_correlation_key_at(item.transition.clone(), 0, correlation_key);
        return run_routed(workflow, forge, context, repo, item, &item.transition).await;
    }

    // A verdict routed to a non-PR-create outcome transition (escalation, a
    // content-bearing rewrite, or an issue breakdown). The head is discarded;
    // any authored body / review body / children is bound through the same keyed
    // runtime seam the role path uses, so the routed transition's `set_body` /
    // `attach_review` / `create_issues` effects can consume the work product. An
    // empty diff here is the escalation signal, not an error.
    let mut context = ExecutionContext::new();
    let routed_create_issues_index = create_issues_effect_index(workflow, &routed);
    if !output.children.is_empty() {
        if let Some(effect_index) = routed_create_issues_index {
            let content_key =
                workspace_content_key(&item.kind, &routed, target_number(item.target));
            context.set_create_issues_at(routed.clone(), effect_index, output.children);
            context.set_create_issues_correlation_key_at(routed.clone(), effect_index, content_key);
        }
    } else if output.body.is_some() || output.review_body.is_some() {
        let content_key = workspace_content_key(&item.kind, &routed, target_number(item.target));
        if let Some(body) = output.body {
            context.set_set_body_at(routed.clone(), 0, body);
            context.set_set_body_correlation_key_at(routed.clone(), 0, content_key.clone());
        }
        if let Some(review_body) = output.review_body {
            context.set_attach_review_at(routed.clone(), 0, review_body);
            context.set_attach_review_correlation_key_at(routed.clone(), 0, content_key);
        }
    }
    run_routed(workflow, forge, context, repo, item, &routed).await
}
