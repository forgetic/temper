//! The label manifest and its compilation from a validated workflow.
//!
//! The label manifest records every label a workflow needs together with the
//! sites that require it (artifact identity, state projection, queue filters,
//! transition effects, and gate outcomes/conditions). Split from the compilation
//! root to keep each file within the source-size budget.

use crate::ids::{
    ArtifactKindId, GateId, LabelId, QueueId, StateDimensionId, StateId, TransitionId,
};
use crate::validated::{Effect, GateCondition, ValidatedWorkflow};

/// The labels a workflow needs, each annotated with why it is needed.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LabelManifest {
    labels: Vec<LabelSpec>,
}

impl LabelManifest {
    /// Returns the label specs in workflow declaration order.
    pub fn labels(&self) -> &[LabelSpec] {
        &self.labels
    }

    /// Returns the spec for a label id, if the workflow declares it.
    pub fn get(&self, id: &LabelId) -> Option<&LabelSpec> {
        self.labels.iter().find(|spec| &spec.id == id)
    }

    /// Returns `true` when the manifest contains the given label id.
    pub fn contains(&self, id: &LabelId) -> bool {
        self.get(id).is_some()
    }
}

/// A single label and the workflow sites that reference it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LabelSpec {
    pub id: LabelId,
    /// Why the label is needed. Empty for a declared-but-unreferenced label.
    pub usages: Vec<LabelUsage>,
}

/// A site that requires a label to exist in the Forge.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LabelUsage {
    /// The label identifies an artifact kind.
    ArtifactIdentity { artifact: ArtifactKindId },
    /// The label projects a state of a dimension.
    StateProjection {
        dimension: StateDimensionId,
        state: StateId,
    },
    /// The label is part of a queue's filter.
    QueueFilter { queue: QueueId },
    /// A transition effect adds or removes the label.
    TransitionEffect { transition: TransitionId },
    /// The label is produced by a transition that satisfies a gate.
    GateOutcome { gate: GateId },
    /// The label is a Forge-projected condition for an external gate.
    GateCondition { gate: GateId },
}

pub(super) fn compile_labels(workflow: &ValidatedWorkflow) -> LabelManifest {
    let mut specs: Vec<LabelSpec> = workflow
        .labels()
        .iter()
        .map(|id| LabelSpec {
            id: id.clone(),
            usages: Vec::new(),
        })
        .collect();

    let mut record = |label: &LabelId, usage: LabelUsage| {
        if let Some(spec) = specs.iter_mut().find(|spec| &spec.id == label)
            && !spec.usages.contains(&usage)
        {
            spec.usages.push(usage);
        }
    };

    for kind in workflow.artifact_kinds() {
        for label in &kind.identifying_labels {
            record(
                label,
                LabelUsage::ArtifactIdentity {
                    artifact: kind.id.clone(),
                },
            );
        }
    }

    for dimension in workflow.state_dimensions() {
        for state in &dimension.states {
            if let Some(label) = &state.label {
                record(
                    label,
                    LabelUsage::StateProjection {
                        dimension: dimension.id.clone(),
                        state: state.id.clone(),
                    },
                );
            }
        }
    }

    for queue in workflow.queues() {
        for label in &queue.labels {
            record(
                label,
                LabelUsage::QueueFilter {
                    queue: queue.id.clone(),
                },
            );
        }
        for label_set in &queue.any_of {
            for label in &label_set.labels {
                record(
                    label,
                    LabelUsage::QueueFilter {
                        queue: queue.id.clone(),
                    },
                );
            }
        }
        if let Some(condition) = &queue.condition
            && let Some(label) = gate_condition_label(condition, workflow)
        {
            record(
                label,
                LabelUsage::QueueFilter {
                    queue: queue.id.clone(),
                },
            );
        }
    }

    for transition in workflow.transitions() {
        for effect in &transition.effects {
            if let Some(label) = effect_label(effect) {
                record(
                    label,
                    LabelUsage::TransitionEffect {
                        transition: transition.id.clone(),
                    },
                );
            }
        }
    }

    for gate in workflow.gates() {
        if let Some(condition) = &gate.condition
            && let Some(label) = gate_condition_label(condition, workflow)
        {
            record(
                label,
                LabelUsage::GateCondition {
                    gate: gate.id.clone(),
                },
            );
        }
        for transition_id in &gate.satisfied_by {
            let Some(transition) = workflow
                .transitions()
                .iter()
                .find(|t| &t.id == transition_id)
            else {
                continue;
            };
            for effect in &transition.effects {
                if let Some(label) = effect_label(effect) {
                    record(
                        label,
                        LabelUsage::GateOutcome {
                            gate: gate.id.clone(),
                        },
                    );
                }
            }
        }
    }

    LabelManifest { labels: specs }
}

fn gate_condition_label<'a>(
    condition: &'a GateCondition,
    workflow: &'a ValidatedWorkflow,
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
        // Reads `dependency` relations by kind, not a label, so it tracks no
        // label usage.
        GateCondition::DependenciesResolved => None,
        // Reads native CI conclusions, not a label, so it tracks no label
        // usage (see ADR 0014).
        GateCondition::CiPassed | GateCondition::CiFailed => None,
        GateCondition::ReviewApproved | GateCondition::ReviewChangesRequested => None,
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
