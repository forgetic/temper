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

use temper_forge::{Forge, RepositoryId};
use temper_workflow::{
    ArtifactSource, CompiledWorkflow, Effect, ExecutionContext, ExecutionError, Executor,
    RoleManifest, TransitionId, ValidatedWorkflow, VerdictId,
};

use crate::scan::AutomatedWorkItem;
use crate::workspace_request::{
    pr_branch_hint, pr_correlation_key, target_number, workspace_content_key,
    workspace_pull_request_input,
};
use crate::{
    CodingWorkspaceGuidance, CodingWorkspaceRepository, CodingWorkspaceRequest,
    CodingWorkspaceWorkItem, ExternalToolExecutors,
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

/// Per-kind index of the first `CreateIssues` effect declared by `transition` in
/// `workflow`, if any.
///
/// The executor counts effect indices per kind, so the first (and, in practice,
/// only) `create_issues` effect on a transition is index `0`.
fn create_issues_effect_index(
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
async fn run_routed<F: Forge + ?Sized>(
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
fn automation_guidance(actor: &RoleManifest, executor_id: &str) -> CodingWorkspaceGuidance {
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

fn undeclared_verdict_error(item: &AutomatedWorkItem, verdict: &VerdictId) -> ExecutionError {
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

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::BTreeMap;
    use std::sync::Arc;

    use async_trait::async_trait;
    use temper_forge::{CreateIssue, CreateRepository, Forge, IssueQuery};
    use temper_forge_memory::MemoryForge;
    use temper_workflow::{
        parse_metadata_block, ArtifactKindId, ArtifactRef, CreateIssuesChild, ExternalToolId,
        QueueId, RawWorkflowSpec, RoleId,
    };

    use crate::{
        CodingWorkspace, CodingWorkspaceError, CodingWorkspaceOutput, CodingWorkspaceRequest,
    };

    /// A queue automation whose `needs_breakdown` verdict routes to a transition
    /// that fans the intake out into dependent children via `create_issues`.
    const BREAKDOWN_WORKFLOW: &str = r#"{
        "name": "architect-automation",
        "roles": [{
            "id": "architect",
            "external_tools": [{
                "id": "coding_workspace",
                "description": "Analyze and author a breakdown.",
                "required": true,
                "constraints": ["Only touch the checked-out repository."],
                "guidance": "Break the intake into children."
            }]
        }],
        "labels": [{"id": "intake"}, {"id": "triaging"}, {"id": "planned"}, {"id": "code"}, {"id": "ready"}],
        "artifact_kinds": [{"id": "epic", "target": "issue", "identifying_labels": ["intake"]}],
        "queues": [{"id": "intake_triage", "artifact": "epic", "labels": ["triaging"]}],
        "transitions": [
            {
                "id": "triage_intake",
                "artifact": "epic",
                "roles": ["architect"],
                "outcomes": {"needs_breakdown": "triage_intake_breakdown"},
                "effects": [{"kind": "remove_label", "label": "triaging"}]
            },
            {
                "id": "triage_intake_breakdown",
                "artifact": "epic",
                "roles": ["architect"],
                "effects": [
                    {"kind": "create_issues"},
                    {"kind": "remove_label", "label": "triaging"},
                    {"kind": "add_label", "label": "planned"}
                ]
            }
        ]
    }"#;

    fn workflow() -> ValidatedWorkflow {
        let spec: RawWorkflowSpec = serde_json::from_str(BREAKDOWN_WORKFLOW).expect("json parses");
        spec.validate().expect("workflow validates")
    }

    /// A workspace that returns a verdict plus authored dependent children, with
    /// no diff — the architect breakdown shape on the automation path.
    struct ChildrenWorkspace {
        verdict: VerdictId,
        children: Vec<CreateIssuesChild>,
    }

    #[async_trait]
    impl CodingWorkspace for ChildrenWorkspace {
        async fn produce_head(
            &self,
            request: CodingWorkspaceRequest,
        ) -> Result<CodingWorkspaceOutput, CodingWorkspaceError> {
            Ok(CodingWorkspaceOutput::new(
                request.branch_hint,
                request.base_branch,
                "broke the intake into dependent children",
                Vec::new(),
                Vec::new(),
            )
            .with_verdict(self.verdict.clone())
            .with_children(self.children.clone()))
        }
    }

    #[tokio::test]
    async fn automation_verdict_routes_to_create_issues_and_fans_out_children() {
        let forge = MemoryForge::new();
        let repo = forge
            .create_repository(CreateRepository {
                owner: "acme".to_string(),
                name: "service".to_string(),
                default_branch: "main".to_string(),
                description: None,
            })
            .await
            .expect("repo created")
            .id;
        let epic = forge
            .create_issue(
                &repo,
                CreateIssue {
                    title: "Epic".to_string(),
                    body: "raw human epic".to_string(),
                    labels: vec!["intake".to_string(), "triaging".to_string()],
                    assignees: Vec::new(),
                },
            )
            .await
            .expect("epic created")
            .number;

        let workflow = workflow();
        let compiled = workflow.compile();
        let workspace: Arc<dyn CodingWorkspace> = Arc::new(ChildrenWorkspace {
            verdict: VerdictId::new("needs_breakdown"),
            children: vec![
                CreateIssuesChild::new("api", "Add the API", "Author the API.")
                    .with_labels(["code", "ready"]),
                CreateIssuesChild::new("ui", "Add the UI", "Consume the API.")
                    .with_labels(["code", "ready"])
                    .with_dependencies(["api"]),
            ],
        });
        let executors = ExternalToolExecutors::new().with_workspace(
            RoleId::new("architect"),
            ExternalToolId::new("coding_workspace"),
            workspace,
        );

        let item = AutomatedWorkItem {
            queue: QueueId::new("intake_triage"),
            actor: RoleId::new("architect"),
            transition: TransitionId::new("triage_intake"),
            executor: Some(ExternalToolId::new("coding_workspace")),
            outcomes: BTreeMap::from([(
                VerdictId::new("needs_breakdown"),
                TransitionId::new("triage_intake_breakdown"),
            )]),
            target: ArtifactSource::Issue { number: epic },
            kind: ArtifactKindId::new("epic"),
        };

        let outcome =
            execute_workspace_automation(&workflow, &compiled, &executors, &forge, &repo, &item)
                .await
                .expect("workspace automation routes to the breakdown");

        assert!(matches!(
            outcome,
            WorkspaceAutomationOutcome::Applied { routed }
                if routed == TransitionId::new("triage_intake_breakdown")
        ));

        // The children fanned out under the intake as parent, the sibling
        // dependency was recorded, and the parent's `planned` label flip applied.
        let parent_ref = ArtifactRef::same_repo(epic);
        let mut created: Vec<_> = forge
            .list_issues(&repo, IssueQuery::default())
            .await
            .expect("issues list")
            .into_iter()
            .filter(|issue| {
                parse_metadata_block(&issue.body)
                    .ok()
                    .flatten()
                    .is_some_and(|metadata| metadata.parents.contains(&parent_ref))
            })
            .collect();
        created.sort_by(|a, b| a.title.cmp(&b.title));
        assert_eq!(created.len(), 2, "two children fanned out under the intake");
        assert_eq!(created[0].title, "Add the API");
        assert_eq!(created[1].title, "Add the UI");

        let api_number = created[0].number;
        let ui_meta = parse_metadata_block(&created[1].body)
            .expect("metadata parses")
            .expect("metadata exists");
        assert!(ui_meta
            .dependencies
            .contains(&ArtifactRef::same_repo(api_number)));

        let parent = forge
            .get_issue_by_number(&repo, epic)
            .await
            .expect("lookup")
            .expect("epic exists");
        assert!(parent.labels.iter().any(|label| label == "planned"));
    }
}
