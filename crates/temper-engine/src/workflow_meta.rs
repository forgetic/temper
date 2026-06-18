// SPDX-License-Identifier: MPL-2.0

//! Workflow-derived label and branch helpers shared by the Forge applier and
//! the work-item feed.
//!
//! The daemon never hardcodes label names: every label set a job or PR must
//! carry is derived from the validated workflow's artifact-kind declarations.

use temper_forge::Repository;
use temper_workflow::{ArtifactKindId, StateId, ValidatedWorkflow};

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

/// Labels a plan-first implementation PR carries while product work is still in
/// progress: stable identifying labels plus the workflow's implementation-PR
/// `in_progress` state label, deliberately excluding review-queue initial
/// labels such as `needs-reviewer`.
pub(crate) fn implementation_pr_plan_create_labels(workflow: &ValidatedWorkflow) -> Vec<String> {
    let artifact = ArtifactKindId::new("implementation_pr");
    let mut labels = implementation_pr_labels(workflow);
    for dimension in workflow.state_dimensions() {
        for state in &dimension.states {
            if state.id == StateId::new("in_progress")
                && state.allows_artifact(&artifact)
                && let Some(label) = &state.label
            {
                push_unique(&mut labels, label.as_str());
            }
        }
    }
    labels
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

fn push_unique(values: &mut Vec<String>, value: &str) {
    if !values.iter().any(|candidate| candidate == value) {
        values.push(value.to_string());
    }
}
