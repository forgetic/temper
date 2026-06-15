// SPDX-License-Identifier: MPL-2.0

//! Workflow-derived label and branch helpers shared by the Forge applier and
//! the work-item feed.
//!
//! The daemon never hardcodes label names: every label set a job or PR must
//! carry is derived from the validated workflow's artifact-kind declarations.

use temper_forge_model::Repository;
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

/// Labels a freshly-created implementation PR must carry.
pub(crate) fn implementation_pr_create_labels(workflow: &ValidatedWorkflow) -> Vec<String> {
    artifact_kind_create_labels(workflow, "implementation_pr")
}

/// Labels a freshly-materialised `code` child issue must carry to be a valid,
/// engineer-ready code artifact: the `code` artifact-kind's identifying labels
/// (so it classifies as `code`, not the catch-all `intake`) plus its initial
/// labels (the activation label, e.g. `ready`, that routes it to the engineer's
/// queue). Derived from the workflow so the daemon never hardcodes label names.
pub(crate) fn code_child_create_labels(workflow: &ValidatedWorkflow) -> Vec<String> {
    artifact_kind_create_labels(workflow, "code")
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
