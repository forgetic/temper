//! Shared workflow-derived label interest for candidate discovery.
//!
//! Role scans and bounded reconciliation have different local queue matching
//! rules, but they must agree on which workflow labels can keep terminal work
//! recoverable. This module is the single derivation boundary for that policy.

use crate::{
    ArtifactTarget, Effect, GateCondition, LabelId, ValidatedTransition, ValidatedWorkflow,
};

/// Workflow-wide candidate-discovery interest.
///
/// `open_labels` contains every declared workflow label for broad labelled
/// reconciliation. `terminal_labels(target)` is narrower: it includes labels
/// used by states, queues (positive, excluded, any-of, and label conditions),
/// transition effects, and gate conditions that can apply to that Forge target.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WorkflowInterest {
    targets: Vec<ArtifactTarget>,
    open_labels: Vec<String>,
    issue_terminal_labels: Vec<String>,
    pull_request_terminal_labels: Vec<String>,
}

impl WorkflowInterest {
    /// Derives deterministic interest from a validated workflow.
    pub fn from_workflow(workflow: &ValidatedWorkflow) -> Self {
        let targets = workflow_targets(workflow);
        let open_labels = workflow
            .labels()
            .iter()
            .map(|label| label.as_str().to_string())
            .collect();
        let issue_terminal_labels = targets
            .contains(&ArtifactTarget::Issue)
            .then(|| terminal_labels(workflow, ArtifactTarget::Issue))
            .unwrap_or_default();
        let pull_request_terminal_labels = targets
            .contains(&ArtifactTarget::PullRequest)
            .then(|| terminal_labels(workflow, ArtifactTarget::PullRequest))
            .unwrap_or_default();
        Self {
            targets,
            open_labels,
            issue_terminal_labels,
            pull_request_terminal_labels,
        }
    }

    /// Forge artifact targets represented by at least one artifact kind.
    pub fn targets(&self) -> &[ArtifactTarget] {
        &self.targets
    }

    /// Whether the workflow represents `target`.
    pub fn has_target(&self, target: ArtifactTarget) -> bool {
        self.targets.contains(&target)
    }

    /// Every declared workflow label, in declaration order.
    pub fn open_labels(&self) -> &[String] {
        &self.open_labels
    }

    /// Bounded terminal/recovery labels for one Forge target.
    pub fn terminal_labels(&self, target: ArtifactTarget) -> &[String] {
        match target {
            ArtifactTarget::Issue => &self.issue_terminal_labels,
            ArtifactTarget::PullRequest => &self.pull_request_terminal_labels,
        }
    }
}

/// Derives the shared candidate-discovery interest for `workflow`.
pub fn workflow_interest(workflow: &ValidatedWorkflow) -> WorkflowInterest {
    WorkflowInterest::from_workflow(workflow)
}

fn workflow_targets(workflow: &ValidatedWorkflow) -> Vec<ArtifactTarget> {
    let mut targets = Vec::new();
    for kind in workflow.artifact_kinds() {
        if !targets.contains(&kind.target) {
            targets.push(kind.target);
        }
    }
    targets
}

fn terminal_labels(workflow: &ValidatedWorkflow, target: ArtifactTarget) -> Vec<String> {
    let mut interest = Vec::new();
    record_state_labels(workflow, target, &mut interest);
    record_queue_labels(workflow, target, &mut interest);
    record_transition_effect_labels(workflow, target, &mut interest);
    record_gate_condition_labels(workflow, target, &mut interest);

    workflow
        .labels()
        .iter()
        .filter(|label| interest.contains(label))
        .map(|label| label.as_str().to_string())
        .collect()
}

fn record_state_labels(
    workflow: &ValidatedWorkflow,
    target: ArtifactTarget,
    labels: &mut Vec<LabelId>,
) {
    for dimension in workflow.state_dimensions() {
        for state in &dimension.states {
            if !state_allows_target(workflow, state, target) {
                continue;
            }
            if let Some(label) = &state.label {
                push_label(labels, label);
            }
        }
    }
}

fn record_queue_labels(
    workflow: &ValidatedWorkflow,
    target: ArtifactTarget,
    labels: &mut Vec<LabelId>,
) {
    for queue in workflow.queues() {
        if !queue.artifacts.iter().any(|artifact| {
            workflow
                .artifact_kind(artifact)
                .is_some_and(|kind| kind.target == target)
        }) {
            continue;
        }
        for label in queue.labels.iter().chain(&queue.excluded_labels) {
            push_label(labels, label);
        }
        for label_set in &queue.any_of {
            for label in &label_set.labels {
                push_label(labels, label);
            }
        }
        if let Some(condition) = &queue.condition {
            if let Some(label) = condition_label(workflow, condition) {
                push_label(labels, label);
            }
        }
    }
}

fn record_transition_effect_labels(
    workflow: &ValidatedWorkflow,
    target: ArtifactTarget,
    labels: &mut Vec<LabelId>,
) {
    for transition in workflow.transitions() {
        if !transition_targets(workflow, transition, target) {
            continue;
        }
        for effect in &transition.effects {
            if let Some(label) = effect_label(effect) {
                push_label(labels, label);
            }
        }
    }
}

fn record_gate_condition_labels(
    workflow: &ValidatedWorkflow,
    target: ArtifactTarget,
    labels: &mut Vec<LabelId>,
) {
    for gate in workflow.gates() {
        if !workflow.transitions().iter().any(|transition| {
            transition.requires_gates.contains(&gate.id)
                && transition_targets(workflow, transition, target)
        }) {
            continue;
        }
        if let Some(condition) = &gate.condition {
            if let Some(label) = condition_label(workflow, condition) {
                push_label(labels, label);
            }
        }
    }
}

fn state_allows_target(
    workflow: &ValidatedWorkflow,
    state: &crate::ValidatedState,
    target: ArtifactTarget,
) -> bool {
    state.artifacts.is_empty()
        || state.artifacts.iter().any(|artifact| {
            workflow
                .artifact_kind(artifact)
                .is_some_and(|kind| kind.target == target)
        })
}

fn transition_targets(
    workflow: &ValidatedWorkflow,
    transition: &ValidatedTransition,
    target: ArtifactTarget,
) -> bool {
    workflow
        .artifact_kind(&transition.artifact)
        .is_some_and(|kind| kind.target == target)
}

fn condition_label<'a>(
    workflow: &'a ValidatedWorkflow,
    condition: &'a GateCondition,
) -> Option<&'a LabelId> {
    match condition {
        GateCondition::LabelPresent(label) => Some(label),
        GateCondition::StateEquals { dimension, state } => workflow
            .state_dimensions()
            .iter()
            .find(|candidate| &candidate.id == dimension)?
            .states
            .iter()
            .find(|candidate| &candidate.id == state)?
            .label
            .as_ref(),
        GateCondition::DependenciesResolved
        | GateCondition::CiPassed
        | GateCondition::CiFailed
        | GateCondition::CiRecoveryRequired
        | GateCondition::ReviewApproved
        | GateCondition::ReviewChangesRequested
        | GateCondition::ExactHeadValidation => None,
    }
}

fn effect_label(effect: &Effect) -> Option<&LabelId> {
    match effect {
        Effect::AddLabel(label)
        | Effect::RemoveLabel(label)
        | Effect::RemoveLabelIfPresent(label) => Some(label),
        Effect::SetAssignee(_)
        | Effect::RemoveAssignee(_)
        | Effect::CreateComment { .. }
        | Effect::CreatePullRequest { .. }
        | Effect::RequestReviewers { .. }
        | Effect::SubmitReview { .. }
        | Effect::SetBody { .. }
        | Effect::AttachReview { .. }
        | Effect::CreateIssues { .. }
        | Effect::MergePullRequest
        | Effect::CloseParentIssues => None,
    }
}

fn push_label(labels: &mut Vec<LabelId>, label: &LabelId) {
    if !labels.contains(label) {
        labels.push(label.clone());
    }
}
