// SPDX-License-Identifier: MPL-2.0

//! Workflow-derived label and branch helpers shared by the Forge applier and
//! the work-item feed.
//!
//! The daemon never hardcodes label names: every label set a job or PR must
//! carry is derived from the validated workflow's artifact-kind declarations.

use temper_forge::Repository;
use temper_workflow::{ArtifactKindId, ValidatedWorkflow};

/// The repository's default branch, falling back to `main` when the Forge
/// reports a blank default.
pub(crate) fn default_base_branch(repository: &Repository) -> String {
    if repository.default_branch.trim().is_empty() {
        "main".to_string()
    } else {
        repository.default_branch.clone()
    }
}

/// The identifying labels of the `implementation_pr` artifact kind — the labels
/// an existing implementation PR is looked up by.
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

/// Labels a freshly-created implementation PR must carry at final handoff.
pub(crate) fn implementation_pr_create_labels(workflow: &ValidatedWorkflow) -> Vec<String> {
    artifact_kind_create_labels(workflow, "implementation_pr")
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
