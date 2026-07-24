//! Cross-symbol reference checks for workflow validation.
//!
//! Walks every place one spec symbol references another (roles, labels, artifact
//! kinds, queues, transitions, gates, states) and records an undeclared-reference
//! diagnostic when the target is not declared. Split from the validation root to
//! keep each file within the source-size budget.

use super::contracts::automation_outcome_references;
use super::{Declared, check_reference};
use crate::diagnostics::{Diagnostic, ReferenceSite, SymbolKind};
use crate::spec::{RawEffect, RawGateCondition, RawIntakeAuthor, RawWorkflowSpec};
use std::collections::HashSet;

/// Walks every cross-symbol reference in the spec.
pub(super) fn check_references(
    spec: &RawWorkflowSpec,
    declared: &Declared<'_>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for role in &spec.roles {
        for queue in &role.queues {
            check_reference(
                declared.queues,
                queue,
                SymbolKind::Queue,
                ReferenceSite::RoleQueue {
                    role: role.id.clone(),
                },
                diagnostics,
            );
        }
    }

    for artifact in &spec.artifact_kinds {
        for label in &artifact.identifying_labels {
            check_reference(
                declared.labels,
                label,
                SymbolKind::Label,
                ReferenceSite::ArtifactLabel {
                    artifact: artifact.id.clone(),
                },
                diagnostics,
            );
        }
        for label in &artifact.initial_labels {
            check_reference(
                declared.labels,
                label,
                SymbolKind::Label,
                ReferenceSite::ArtifactLabel {
                    artifact: artifact.id.clone(),
                },
                diagnostics,
            );
        }
    }

    for relation in &spec.relations {
        let site = relation_site(relation.kind, &relation.source, &relation.target);
        check_reference(
            declared.artifacts,
            &relation.source,
            SymbolKind::ArtifactKind,
            ReferenceSite::RelationSource {
                relation: site.clone(),
            },
            diagnostics,
        );
        check_reference(
            declared.artifacts,
            &relation.target,
            SymbolKind::ArtifactKind,
            ReferenceSite::RelationTarget { relation: site },
            diagnostics,
        );
    }

    check_queue_references(spec, declared, diagnostics);
    check_transition_references(spec, declared, diagnostics);
    check_validation_binding_references(spec, declared, diagnostics);

    for gate in &spec.gates {
        for transition in &gate.satisfied_by {
            check_reference(
                declared.transitions,
                transition,
                SymbolKind::Transition,
                ReferenceSite::GateTransition {
                    gate: gate.id.clone(),
                },
                diagnostics,
            );
        }
        if let Some(condition) = &gate.condition {
            check_condition(
                spec,
                declared,
                condition,
                ReferenceSite::GateCondition {
                    gate: gate.id.clone(),
                },
                diagnostics,
            );
        }
    }

    if let Some(RawIntakeAuthor::Role { role }) = &spec.intake_author {
        check_reference(
            declared.roles,
            role,
            SymbolKind::Role,
            ReferenceSite::IntakeAuthor,
            diagnostics,
        );
    }
}

fn check_queue_references(
    spec: &RawWorkflowSpec,
    declared: &Declared<'_>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for queue in &spec.queues {
        if queue.artifacts.is_empty() {
            diagnostics.push(Diagnostic::EmptyQueueArtifacts {
                queue: queue.id.clone(),
            });
        }
        for artifact in &queue.artifacts {
            check_reference(
                declared.artifacts,
                artifact,
                SymbolKind::ArtifactKind,
                ReferenceSite::QueueArtifact {
                    queue: queue.id.clone(),
                },
                diagnostics,
            );
        }
        check_queue_labels(queue.labels.iter(), &queue.id, declared, diagnostics);
        check_queue_labels(
            queue.excluded_labels.iter(),
            &queue.id,
            declared,
            diagnostics,
        );
        for label_set in &queue.any_of {
            check_queue_labels(label_set.labels.iter(), &queue.id, declared, diagnostics);
        }
        if let Some(condition) = &queue.condition {
            check_condition(
                spec,
                declared,
                condition,
                ReferenceSite::QueueCondition {
                    queue: queue.id.clone(),
                },
                diagnostics,
            );
        }
        if let Some(automation) = &queue.automation {
            check_reference(
                declared.roles,
                &automation.actor,
                SymbolKind::Role,
                ReferenceSite::QueueAutomationActor {
                    queue: queue.id.clone(),
                },
                diagnostics,
            );
            check_reference(
                declared.transitions,
                &automation.transition,
                SymbolKind::Transition,
                ReferenceSite::QueueAutomationTransition {
                    queue: queue.id.clone(),
                },
                diagnostics,
            );
            for (verdict, transition) in automation_outcome_references(automation) {
                check_reference(
                    declared.transitions,
                    &transition,
                    SymbolKind::Transition,
                    ReferenceSite::QueueAutomationOutcome {
                        queue: queue.id.clone(),
                        verdict,
                    },
                    diagnostics,
                );
            }
        }
        for action in &queue.actions {
            check_reference(
                declared.roles,
                &action.role,
                SymbolKind::Role,
                ReferenceSite::QueueActionRole {
                    queue: queue.id.clone(),
                },
                diagnostics,
            );
            if let Some(artifact) = &action.artifact {
                check_reference(
                    declared.artifacts,
                    artifact,
                    SymbolKind::ArtifactKind,
                    ReferenceSite::QueueActionArtifact {
                        queue: queue.id.clone(),
                        role: action.role.clone(),
                    },
                    diagnostics,
                );
            }
            check_reference(
                declared.transitions,
                &action.action,
                SymbolKind::Transition,
                ReferenceSite::QueueActionTransition {
                    queue: queue.id.clone(),
                    role: action.role.clone(),
                },
                diagnostics,
            );
        }
    }
}

fn check_validation_binding_references(
    spec: &RawWorkflowSpec,
    declared: &Declared<'_>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for binding in &spec.validation_bindings {
        check_reference(
            declared.roles,
            &binding.role,
            SymbolKind::Role,
            ReferenceSite::ValidationBindingRole {
                binding: binding.id.clone(),
            },
            diagnostics,
        );
        check_reference(
            declared.transitions,
            &binding.action,
            SymbolKind::Transition,
            ReferenceSite::ValidationBindingAction {
                binding: binding.id.clone(),
            },
            diagnostics,
        );
        check_reference(
            declared.artifacts,
            &binding.target_artifact,
            SymbolKind::ArtifactKind,
            ReferenceSite::ValidationBindingTargetArtifact {
                binding: binding.id.clone(),
            },
            diagnostics,
        );
    }
}

fn check_transition_references(
    spec: &RawWorkflowSpec,
    declared: &Declared<'_>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for transition in &spec.transitions {
        check_reference(
            declared.artifacts,
            &transition.artifact,
            SymbolKind::ArtifactKind,
            ReferenceSite::TransitionArtifact {
                transition: transition.id.clone(),
            },
            diagnostics,
        );
        for role in &transition.roles {
            check_reference(
                declared.roles,
                role,
                SymbolKind::Role,
                ReferenceSite::TransitionRole {
                    transition: transition.id.clone(),
                },
                diagnostics,
            );
        }
        for gate in &transition.requires_gates {
            check_reference(
                declared.gates,
                gate,
                SymbolKind::Gate,
                ReferenceSite::TransitionGate {
                    transition: transition.id.clone(),
                },
                diagnostics,
            );
        }
        for effect in &transition.effects {
            if let Some(label) = effect_label(effect) {
                check_reference(
                    declared.labels,
                    label,
                    SymbolKind::Label,
                    ReferenceSite::TransitionEffectLabel {
                        transition: transition.id.clone(),
                    },
                    diagnostics,
                );
            }
            if let Some(artifact_kind) = effect_artifact_kind(effect) {
                check_reference(
                    declared.artifacts,
                    artifact_kind,
                    SymbolKind::ArtifactKind,
                    ReferenceSite::TransitionEffectArtifactKind {
                        transition: transition.id.clone(),
                    },
                    diagnostics,
                );
            }
            for role in effect_roles(effect) {
                check_reference(
                    declared.roles,
                    role,
                    SymbolKind::Role,
                    ReferenceSite::TransitionEffectRole {
                        transition: transition.id.clone(),
                    },
                    diagnostics,
                );
            }
        }
        for (verdict, target) in &transition.outcomes {
            check_reference(
                declared.transitions,
                target,
                SymbolKind::Transition,
                ReferenceSite::TransitionOutcome {
                    transition: transition.id.clone(),
                    verdict: verdict.clone(),
                },
                diagnostics,
            );
        }
    }
}

fn check_queue_labels<'a>(
    labels: impl Iterator<Item = &'a String>,
    queue: &str,
    declared: &Declared<'_>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for label in labels {
        check_reference(
            declared.labels,
            label,
            SymbolKind::Label,
            ReferenceSite::QueueLabel {
                queue: queue.to_string(),
            },
            diagnostics,
        );
    }
}

fn relation_site(kind: crate::relation::RelationKind, source: &str, target: &str) -> String {
    format!("{kind} {source}->{target}")
}

fn check_condition(
    spec: &RawWorkflowSpec,
    declared: &Declared<'_>,
    condition: &RawGateCondition,
    site: ReferenceSite,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match condition {
        RawGateCondition::LabelPresent { label } => {
            check_reference(declared.labels, label, SymbolKind::Label, site, diagnostics)
        }
        RawGateCondition::StateEquals { dimension, state } => {
            check_reference(
                declared.state_dimensions,
                dimension,
                SymbolKind::StateDimension,
                site.clone(),
                diagnostics,
            );
            if spec
                .state_dimensions
                .iter()
                .find(|candidate| &candidate.id == dimension)
                .is_some_and(|declared| declared.states.iter().all(|s| &s.id != state))
            {
                diagnostics.push(Diagnostic::UndeclaredReference {
                    expected: SymbolKind::State,
                    id: state.clone(),
                    site,
                });
            }
        }
        RawGateCondition::DependenciesResolved
        | RawGateCondition::CiPassed
        | RawGateCondition::CiFailed
        | RawGateCondition::CiRecoveryRequired
        | RawGateCondition::ReviewApproved
        | RawGateCondition::ReviewChangesRequested => {}
    }
}

/// Checks within-dimension duplicate states and label references on states.
pub(super) fn check_state_dimensions(
    spec: &RawWorkflowSpec,
    labels: &HashSet<String>,
    artifacts: &HashSet<String>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for dimension in &spec.state_dimensions {
        let mut seen = HashSet::new();
        for state in &dimension.states {
            if !seen.insert(state.id.clone()) {
                diagnostics.push(Diagnostic::DuplicateState {
                    dimension: dimension.id.clone(),
                    id: state.id.clone(),
                });
            }
            if let Some(label) = &state.label {
                check_reference(
                    labels,
                    label,
                    SymbolKind::Label,
                    ReferenceSite::StateLabel {
                        dimension: dimension.id.clone(),
                        state: state.id.clone(),
                    },
                    diagnostics,
                );
            }
            for artifact in &state.artifacts {
                check_reference(
                    artifacts,
                    artifact,
                    SymbolKind::ArtifactKind,
                    ReferenceSite::StateArtifact {
                        dimension: dimension.id.clone(),
                        state: state.id.clone(),
                    },
                    diagnostics,
                );
            }
        }
    }
}

/// Returns the label id referenced by a raw effect, if any.
fn effect_label(effect: &RawEffect) -> Option<&str> {
    match effect {
        RawEffect::AddLabel { label } | RawEffect::RemoveLabel { label, .. } => Some(label),
        RawEffect::SetAssignee { .. }
        | RawEffect::RemoveAssignee { .. }
        | RawEffect::CreateComment { .. }
        | RawEffect::CreatePullRequest { .. }
        | RawEffect::RequestReviewers { .. }
        | RawEffect::SubmitReview { .. }
        | RawEffect::SetBody { .. }
        | RawEffect::AttachReview { .. }
        | RawEffect::CreateIssues { .. }
        | RawEffect::MergePullRequest
        | RawEffect::CloseParentIssues => None,
    }
}

/// Returns the artifact kind id referenced by a raw effect, if any.
fn effect_artifact_kind(effect: &RawEffect) -> Option<&str> {
    match effect {
        RawEffect::CreatePullRequest {
            artifact_kind: Some(artifact_kind),
            ..
        } => Some(artifact_kind),
        RawEffect::AddLabel { .. }
        | RawEffect::RemoveLabel { .. }
        | RawEffect::SetAssignee { .. }
        | RawEffect::RemoveAssignee { .. }
        | RawEffect::CreateComment { .. }
        | RawEffect::CreatePullRequest { .. }
        | RawEffect::RequestReviewers { .. }
        | RawEffect::SubmitReview { .. }
        | RawEffect::SetBody { .. }
        | RawEffect::AttachReview { .. }
        | RawEffect::CreateIssues { .. }
        | RawEffect::MergePullRequest
        | RawEffect::CloseParentIssues => None,
    }
}

/// Returns role ids referenced by a raw effect.
fn effect_roles(effect: &RawEffect) -> Vec<&str> {
    match effect {
        RawEffect::SetAssignee { role } | RawEffect::RemoveAssignee { role } => vec![role],
        RawEffect::RequestReviewers { roles } => roles.iter().map(String::as_str).collect(),
        RawEffect::AddLabel { .. }
        | RawEffect::RemoveLabel { .. }
        | RawEffect::CreateComment { .. }
        | RawEffect::CreatePullRequest { .. }
        | RawEffect::SubmitReview { .. }
        | RawEffect::SetBody { .. }
        | RawEffect::AttachReview { .. }
        | RawEffect::CreateIssues { .. }
        | RawEffect::MergePullRequest
        | RawEffect::CloseParentIssues => Vec::new(),
    }
}
