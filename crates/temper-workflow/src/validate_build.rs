//! Raw-to-validated conversion helpers for static workflow validation.

use crate::ids::{
    ArtifactKindId, ExternalToolId, GateId, LabelId, QueueId, RoleId, StateDimensionId, StateId,
    TransitionId, VerdictId,
};
use crate::spec::{
    RawEffect, RawGateCondition, RawIntakeAuthor, RawQueueAction, RawQueueAutomation,
    RawWorkflowSpec,
};
use crate::validated::{
    Effect, ExternalToolDeclaration, GateCondition, IntakeAuthor, QueueAction, QueueAutomation,
    QueueLabelSet, RolePromptExtension, ValidatedArtifactKind, ValidatedGate, ValidatedQueue,
    ValidatedRelation, ValidatedRole, ValidatedState, ValidatedStateDimension, ValidatedTransition,
    ValidatedWorkflow,
};
use chrono::Duration;
use std::collections::BTreeMap;

/// Converts a checked spec into the typed validated model.
pub(crate) fn build_validated(spec: &RawWorkflowSpec) -> ValidatedWorkflow {
    let roles = spec
        .roles
        .iter()
        .map(|role| ValidatedRole {
            id: RoleId::new(&role.id),
            charter: role.charter.clone(),
            prompt: RolePromptExtension {
                guidance: role.prompt.guidance.clone(),
                tool_guidance: role.prompt.tool_guidance.clone(),
            },
            external_tools: role
                .external_tools
                .iter()
                .map(|tool| ExternalToolDeclaration {
                    id: ExternalToolId::new(&tool.id),
                    description: tool.description.clone(),
                    required: tool.required,
                    constraints: tool.constraints.clone(),
                    guidance: tool.guidance.clone(),
                })
                .collect(),
            concurrency: role.concurrency,
            queues: role.queues.iter().map(QueueId::new).collect(),
        })
        .collect();

    let labels = spec.labels.iter().map(|l| LabelId::new(&l.id)).collect();
    let artifact_kinds = spec
        .artifact_kinds
        .iter()
        .map(|artifact| ValidatedArtifactKind {
            id: ArtifactKindId::new(&artifact.id),
            target: artifact.target,
            identifying_labels: artifact
                .identifying_labels
                .iter()
                .map(LabelId::new)
                .collect(),
            initial_labels: artifact.initial_labels.iter().map(LabelId::new).collect(),
        })
        .collect();
    let state_dimensions = spec
        .state_dimensions
        .iter()
        .map(build_state_dimension)
        .collect();
    let queues = spec.queues.iter().map(build_queue).collect();
    let transitions = spec.transitions.iter().map(build_transition).collect();
    let gates = spec.gates.iter().map(build_gate).collect();
    let relations = spec
        .relations
        .iter()
        .map(|relation| ValidatedRelation {
            kind: relation.kind,
            source: ArtifactKindId::new(&relation.source),
            target: ArtifactKindId::new(&relation.target),
        })
        .collect();

    let intake_author = spec.intake_author.as_ref().map(build_intake_author);

    ValidatedWorkflow::new(
        spec.name.clone(),
        roles,
        labels,
        artifact_kinds,
        state_dimensions,
        queues,
        transitions,
        gates,
        relations,
        intake_author,
    )
}

fn build_intake_author(author: &RawIntakeAuthor) -> IntakeAuthor {
    match author {
        RawIntakeAuthor::Role { role } => IntakeAuthor::Role(RoleId::new(role)),
        RawIntakeAuthor::SiteAdmin => IntakeAuthor::SiteAdmin,
    }
}

fn build_state_dimension(dimension: &crate::spec::RawStateDimension) -> ValidatedStateDimension {
    ValidatedStateDimension {
        id: StateDimensionId::new(&dimension.id),
        exclusive: dimension.exclusive,
        states: dimension
            .states
            .iter()
            .map(|state| ValidatedState {
                id: StateId::new(&state.id),
                label: state.label.as_ref().map(LabelId::new),
                artifacts: state.artifacts.iter().map(ArtifactKindId::new).collect(),
            })
            .collect(),
    }
}

fn build_queue(queue: &crate::spec::RawQueue) -> ValidatedQueue {
    ValidatedQueue {
        id: QueueId::new(&queue.id),
        artifacts: queue.artifacts.iter().map(ArtifactKindId::new).collect(),
        labels: queue.labels.iter().map(LabelId::new).collect(),
        any_of: queue
            .any_of
            .iter()
            .map(|label_set| QueueLabelSet {
                labels: label_set.labels.iter().map(LabelId::new).collect(),
            })
            .collect(),
        min_depth: queue.min_depth,
        max_age: queue
            .max_age
            .map(|seconds| Duration::seconds(i64::from(seconds))),
        condition: queue.condition.as_ref().map(build_gate_condition),
        automation: queue.automation.as_ref().map(build_queue_automation),
        actions: queue.actions.iter().map(build_queue_action).collect(),
    }
}

fn build_queue_action(action: &RawQueueAction) -> QueueAction {
    QueueAction {
        role: RoleId::new(&action.role),
        artifact: action.artifact.as_deref().map(ArtifactKindId::new),
        action: TransitionId::new(&action.action),
        checkout: action.checkout.clone(),
        guidance: action.guidance.clone(),
    }
}

fn build_queue_automation(automation: &RawQueueAutomation) -> QueueAutomation {
    QueueAutomation {
        actor: RoleId::new(&automation.actor),
        transition: TransitionId::new(&automation.transition),
        executor: automation.executor.as_deref().map(ExternalToolId::new),
        outcomes: build_outcomes(
            &automation.outcomes,
            automation.on_merge_conflict.as_deref(),
        ),
    }
}

/// Builds the verdict -> transition outcome map, desugaring `on_merge_conflict`
/// into the built-in merge-conflict verdict. An explicit `outcomes` entry for
/// that verdict takes precedence over the sugar.
fn build_outcomes(
    outcomes: &BTreeMap<String, String>,
    on_merge_conflict: Option<&str>,
) -> BTreeMap<VerdictId, TransitionId> {
    let mut map: BTreeMap<VerdictId, TransitionId> = outcomes
        .iter()
        .map(|(verdict, transition)| (VerdictId::new(verdict), TransitionId::new(transition)))
        .collect();
    if let Some(fallback) = on_merge_conflict {
        map.entry(VerdictId::merge_conflict())
            .or_insert_with(|| TransitionId::new(fallback));
    }
    map
}

fn build_transition(transition: &crate::spec::RawTransition) -> ValidatedTransition {
    ValidatedTransition {
        id: TransitionId::new(&transition.id),
        artifact: ArtifactKindId::new(&transition.artifact),
        roles: transition.roles.iter().map(RoleId::new).collect(),
        requires_gates: transition.requires_gates.iter().map(GateId::new).collect(),
        effects: transition.effects.iter().map(build_effect).collect(),
        outcomes: build_outcomes(&transition.outcomes, None),
    }
}

fn build_gate(gate: &crate::spec::RawGate) -> ValidatedGate {
    ValidatedGate {
        id: GateId::new(&gate.id),
        satisfied_by: gate.satisfied_by.iter().map(TransitionId::new).collect(),
        condition: gate.condition.as_ref().map(build_gate_condition),
    }
}

fn build_gate_condition(condition: &RawGateCondition) -> GateCondition {
    match condition {
        RawGateCondition::LabelPresent { label } => {
            GateCondition::LabelPresent(LabelId::new(label))
        }
        RawGateCondition::DependenciesResolved => GateCondition::DependenciesResolved,
        RawGateCondition::CiPassed => GateCondition::CiPassed,
        RawGateCondition::CiFailed => GateCondition::CiFailed,
        RawGateCondition::ReviewApproved => GateCondition::ReviewApproved,
        RawGateCondition::ReviewChangesRequested => GateCondition::ReviewChangesRequested,
        RawGateCondition::StateEquals { dimension, state } => GateCondition::StateEquals {
            dimension: StateDimensionId::new(dimension),
            state: StateId::new(state),
        },
    }
}

/// Converts a raw effect into a typed effect.
fn build_effect(effect: &RawEffect) -> Effect {
    match effect {
        RawEffect::AddLabel { label } => Effect::AddLabel(LabelId::new(label)),
        RawEffect::RemoveLabel { label, if_present } => {
            if *if_present {
                Effect::RemoveLabelIfPresent(LabelId::new(label))
            } else {
                Effect::RemoveLabel(LabelId::new(label))
            }
        }
        RawEffect::SetAssignee { role } => Effect::SetAssignee(RoleId::new(role)),
        RawEffect::RemoveAssignee { role } => Effect::RemoveAssignee(RoleId::new(role)),
        RawEffect::CreateComment { body } => Effect::CreateComment { body: body.clone() },
        RawEffect::CreatePullRequest { correlation_key } => Effect::CreatePullRequest {
            correlation_key: correlation_key.clone(),
        },
        RawEffect::RequestReviewers { roles } => Effect::RequestReviewers {
            roles: roles.iter().map(RoleId::new).collect(),
        },
        RawEffect::SubmitReview { decision } => Effect::SubmitReview {
            decision: *decision,
        },
        RawEffect::SetBody { correlation_key } => Effect::SetBody {
            correlation_key: correlation_key.clone(),
        },
        RawEffect::AttachReview {
            decision,
            correlation_key,
        } => Effect::AttachReview {
            decision: *decision,
            correlation_key: correlation_key.clone(),
        },
        RawEffect::CreateIssues { correlation_key } => Effect::CreateIssues {
            correlation_key: correlation_key.clone(),
        },
        RawEffect::MergePullRequest => Effect::MergePullRequest,
        RawEffect::CloseParentIssues => Effect::CloseParentIssues,
    }
}
