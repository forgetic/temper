// SPDX-License-Identifier: MPL-2.0

//! Workflow-derived label and branch helpers shared by the Forge applier and
//! the work-item feed.
//!
//! The daemon never hardcodes label names: every label set a job or PR must
//! carry is derived from the validated workflow's artifact-kind declarations.

use temper_forge::Repository;
use temper_workflow::{
    ArtifactKindId, Effect, TargetBranchPolicy, TransitionId, ValidatedWorkflow,
};

/// The repository's default branch, falling back to `main` when the Forge
/// reports a blank default.
pub(crate) fn default_base_branch(repository: &Repository) -> String {
    if repository.default_branch.trim().is_empty() {
        "main".to_string()
    } else {
        repository.default_branch.clone()
    }
}

/// Returns the explicit target-branch policy carried by a transition's PR
/// create. A transition that omits the policy retains legacy behavior. Mixing
/// legacy and explicit creates, or declaring conflicting explicit policies, is
/// rejected at runtime rather than allowing effect order to choose authority.
pub(crate) fn create_pull_request_target_branch_policy(
    workflow: &ValidatedWorkflow,
    transition: &TransitionId,
) -> Result<Option<TargetBranchPolicy>, String> {
    let Some(transition) = workflow
        .transitions()
        .iter()
        .find(|candidate| candidate.id == *transition)
    else {
        return Err(format!("workflow transition `{transition}` does not exist"));
    };
    let policies = transition
        .effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::CreatePullRequest {
                target_branch_policy,
                ..
            } => Some(*target_branch_policy),
            _ => None,
        })
        .collect::<Vec<_>>();
    let Some(first) = policies.first().copied() else {
        return Ok(None);
    };
    if policies.iter().any(|policy| *policy != first) {
        return Err(format!(
            "workflow transition `{}` mixes conflicting or omitted create_pull_request target-branch policies",
            transition.id
        ));
    }
    Ok(first)
}

/// Resolves the pull-request artifact kind produced by a writable issue action.
/// Legacy untyped `create_pull_request` effects continue to produce
/// `implementation_pr`; an explicitly typed effect enables distinct products
/// such as a scenario-authoring PR.
pub(crate) fn success_pull_request_artifact_kind(
    workflow: &ValidatedWorkflow,
    transition: &TransitionId,
) -> Result<ArtifactKindId, String> {
    let Some(transition) = workflow
        .transitions()
        .iter()
        .find(|candidate| candidate.id == *transition)
    else {
        return Err(format!("workflow transition `{transition}` does not exist"));
    };
    let kinds = transition
        .effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::CreatePullRequest { artifact_kind, .. } => Some(
                artifact_kind
                    .clone()
                    .unwrap_or_else(|| ArtifactKindId::new("implementation_pr")),
            ),
            _ => None,
        })
        .collect::<Vec<_>>();
    let Some(first) = kinds.first() else {
        return Ok(ArtifactKindId::new("implementation_pr"));
    };
    if kinds.iter().any(|kind| kind != first) {
        return Err(format!(
            "workflow transition `{}` creates conflicting pull-request artifact kinds",
            transition.id
        ));
    }
    Ok(first.clone())
}

/// The identifying labels of the legacy implementation PR kind used by feed
/// suppression when a code issue already produced an open PR.
pub(crate) fn implementation_pr_labels(workflow: &ValidatedWorkflow) -> Vec<String> {
    workflow
        .artifact_kind(&ArtifactKindId::new("implementation_pr"))
        .map(|kind| {
            kind.identifying_labels
                .iter()
                .map(|label| label.as_str().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Labels a freshly-created child issue of `kind_id` must carry. The artifact
/// kind's identifying labels are always included. Initial labels are included
/// only when they do not conflict with state labels the child explicitly
/// authored for the same artifact kind (for example, a `blocked` code child must
/// not also receive the code kind's default `ready` lifecycle label).
pub(crate) fn artifact_kind_child_create_labels(
    workflow: &ValidatedWorkflow,
    kind_id: &ArtifactKindId,
    child_labels: &[String],
) -> Option<Vec<String>> {
    let kind = workflow.artifact_kind(kind_id)?;

    let mut labels = Vec::new();
    for label in &kind.identifying_labels {
        push_label(&mut labels, label.as_str());
    }
    for label in &kind.initial_labels {
        if !initial_label_conflicts_with_child_state(
            workflow,
            kind_id,
            label.as_str(),
            child_labels,
        ) {
            push_label(&mut labels, label.as_str());
        }
    }
    for label in child_labels {
        push_label(&mut labels, label);
    }
    Some(labels)
}

fn initial_label_conflicts_with_child_state(
    workflow: &ValidatedWorkflow,
    kind_id: &ArtifactKindId,
    initial_label: &str,
    child_labels: &[String],
) -> bool {
    workflow
        .state_dimensions()
        .iter()
        .filter(|dimension| dimension.exclusive)
        .any(|dimension| {
            let initial_is_state_for_kind = dimension.states.iter().any(|state| {
                state.allows_artifact(kind_id)
                    && state
                        .label
                        .as_ref()
                        .is_some_and(|label| label.as_str() == initial_label)
            });
            initial_is_state_for_kind
                && dimension.states.iter().any(|state| {
                    state.allows_artifact(kind_id)
                        && state.label.as_ref().is_some_and(|label| {
                            label.as_str() != initial_label
                                && child_labels
                                    .iter()
                                    .any(|child| child.as_str() == label.as_str())
                        })
                })
        })
}

fn push_label(labels: &mut Vec<String>, label: &str) {
    if !labels.iter().any(|existing| existing == label) {
        labels.push(label.to_string());
    }
}

/// Union of an artifact-kind's identifying and initial labels, in declaration
/// order with duplicates removed. These are the labels an artifact of that kind
/// is created with.
pub(crate) fn artifact_kind_create_labels(
    workflow: &ValidatedWorkflow,
    kind_id: &str,
) -> Vec<String> {
    let Some(kind) = workflow.artifact_kind(&ArtifactKindId::new(kind_id)) else {
        return Vec::new();
    };

    let mut labels = Vec::new();
    for label in kind
        .identifying_labels
        .iter()
        .chain(kind.initial_labels.iter())
    {
        let label = label.as_str().to_string();
        if !labels.contains(&label) {
            labels.push(label);
        }
    }
    labels
}
