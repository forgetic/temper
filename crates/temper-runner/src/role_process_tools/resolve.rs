//! Workspace executor resolution and action-shape classification helpers.

use std::sync::Arc;

use temper_workflow::{Effect, RoleManifest, ToolManifest};

use crate::{
    BoundExternalTool, CodingWorkspace, CodingWorkspaceGuidance, ExternalToolExecutors,
    WorkspaceCheckout,
};

/// A workspace executor resolved for an action, plus the executor id (declared
/// external-tool id) it was bound under, for observability and guidance lookup,
/// and the checkout capability the runner bound it with.
pub(super) struct ResolvedWorkspace<'a> {
    pub(super) tool_id: &'a str,
    pub(super) workspace: Arc<dyn CodingWorkspace>,
    pub(super) checkout: WorkspaceCheckout,
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
pub(super) fn action_is_workspace_backed(tool: &ToolManifest) -> bool {
    !tool.outcomes.is_empty() || create_pull_request_count(tool) > 0
}

/// Resolves a workspace executor for `manifest`'s role from the runner-bound
/// external tools, keyed by executor id.
///
/// This is role-agnostic: it returns the first declared external tool that is
/// both bound and backed by a registered workspace executor, rather than
/// matching a hardcoded `coding_workspace` id.
pub(super) fn workspace_executor<'a>(
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
pub(super) fn workspace_executor_hint(manifest: &RoleManifest) -> Option<&str> {
    manifest
        .external_tools
        .first()
        .map(|declared| declared.id.as_str())
}

pub(super) fn workspace_guidance(
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

pub(super) fn create_pull_request_count(tool: &ToolManifest) -> usize {
    tool.effects
        .iter()
        .filter(|effect| matches!(effect, Effect::CreatePullRequest { .. }))
        .count()
}

/// The verdict vocabulary an action declares: the keys of its `outcomes` map,
/// as plain strings, for the workspace request and context. Empty for a pure
/// head action with no declared `outcomes`.
pub(super) fn allowed_verdicts(tool: &ToolManifest) -> Vec<String> {
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
