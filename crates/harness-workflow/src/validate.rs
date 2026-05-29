//! Static validation from a [`RawWorkflowSpec`] to a [`ValidatedWorkflow`].
//!
//! Validation is diagnostic-collecting: it walks the whole spec and records
//! every problem before deciding whether the workflow is valid. Phase 2 covers
//! duplicate ids and undeclared references; later phases will add semantic
//! checks (contradictory effects, unsatisfiable gates, tool-authority limits).

use crate::diagnostics::{Diagnostic, ReferenceSite, SymbolKind};
use crate::ids::{
    ArtifactKindId, GateId, LabelId, QueueId, RoleId, StateDimensionId, StateId, TransitionId,
};
use crate::spec::{RawEffect, RawGateCondition, RawWorkflowSpec};
use crate::validated::{
    Effect, GateCondition, QueueLabelSet, ValidatedArtifactKind, ValidatedGate, ValidatedQueue,
    ValidatedRelation, ValidatedRole, ValidatedState, ValidatedStateDimension, ValidatedTransition,
    ValidatedWorkflow,
};
use crate::ValidationErrors;
use chrono::Duration;
use std::collections::HashSet;

/// Validates a raw workflow spec, collecting all diagnostics.
///
/// On success, returns a [`ValidatedWorkflow`]. On failure, returns every
/// detected problem so the author can fix them in one pass.
pub fn validate(spec: &RawWorkflowSpec) -> Result<ValidatedWorkflow, ValidationErrors> {
    let mut diagnostics = Vec::new();

    // Declared-symbol sets double as duplicate detectors and reference targets.
    let roles = collect_declared(
        spec.roles.iter().map(|r| &r.id),
        SymbolKind::Role,
        &mut diagnostics,
    );
    let labels = collect_declared(
        spec.labels.iter().map(|l| &l.id),
        SymbolKind::Label,
        &mut diagnostics,
    );
    let artifacts = collect_declared(
        spec.artifact_kinds.iter().map(|a| &a.id),
        SymbolKind::ArtifactKind,
        &mut diagnostics,
    );
    // State dimension ids are duplicate-checked and referenced by external
    // gate conditions.
    let state_dimensions = collect_declared(
        spec.state_dimensions.iter().map(|d| &d.id),
        SymbolKind::StateDimension,
        &mut diagnostics,
    );
    let queues = collect_declared(
        spec.queues.iter().map(|q| &q.id),
        SymbolKind::Queue,
        &mut diagnostics,
    );
    let transitions = collect_declared(
        spec.transitions.iter().map(|t| &t.id),
        SymbolKind::Transition,
        &mut diagnostics,
    );
    let gates = collect_declared(
        spec.gates.iter().map(|g| &g.id),
        SymbolKind::Gate,
        &mut diagnostics,
    );

    check_references(
        spec,
        &Declared {
            roles: &roles,
            labels: &labels,
            artifacts: &artifacts,
            state_dimensions: &state_dimensions,
            queues: &queues,
            transitions: &transitions,
            gates: &gates,
        },
        &mut diagnostics,
    );

    check_state_dimensions(spec, &labels, &mut diagnostics);

    if diagnostics.is_empty() {
        Ok(build_validated(spec))
    } else {
        Err(ValidationErrors::new(diagnostics))
    }
}

/// Declared symbol sets used to resolve references.
struct Declared<'a> {
    roles: &'a HashSet<String>,
    labels: &'a HashSet<String>,
    artifacts: &'a HashSet<String>,
    state_dimensions: &'a HashSet<String>,
    queues: &'a HashSet<String>,
    transitions: &'a HashSet<String>,
    gates: &'a HashSet<String>,
}

/// Collects declared ids, recording a [`Diagnostic::DuplicateId`] per repeat.
fn collect_declared<'a>(
    ids: impl Iterator<Item = &'a String>,
    kind: SymbolKind,
    diagnostics: &mut Vec<Diagnostic>,
) -> HashSet<String> {
    let mut seen = HashSet::new();
    for id in ids {
        if !seen.insert(id.clone()) {
            diagnostics.push(Diagnostic::DuplicateId {
                kind,
                id: id.clone(),
            });
        }
    }
    seen
}

/// Records an undeclared-reference diagnostic when `id` is not declared.
fn check_reference(
    declared: &HashSet<String>,
    id: &str,
    expected: SymbolKind,
    site: ReferenceSite,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if !declared.contains(id) {
        diagnostics.push(Diagnostic::UndeclaredReference {
            expected,
            id: id.to_string(),
            site,
        });
    }
}

/// Walks every cross-symbol reference in the spec.
fn check_references(
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
        for label_set in &queue.any_of {
            check_queue_labels(label_set.labels.iter(), &queue.id, declared, diagnostics);
        }
    }

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
            if let Some(role) = effect_role(effect) {
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
    }

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
            check_gate_condition(spec, declared, &gate.id, condition, diagnostics);
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

fn check_gate_condition(
    spec: &RawWorkflowSpec,
    declared: &Declared<'_>,
    gate: &str,
    condition: &RawGateCondition,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match condition {
        RawGateCondition::LabelPresent { label } => check_reference(
            declared.labels,
            label,
            SymbolKind::Label,
            ReferenceSite::GateCondition {
                gate: gate.to_string(),
            },
            diagnostics,
        ),
        RawGateCondition::StateEquals { dimension, state } => {
            check_reference(
                declared.state_dimensions,
                dimension,
                SymbolKind::StateDimension,
                ReferenceSite::GateCondition {
                    gate: gate.to_string(),
                },
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
                    site: ReferenceSite::GateCondition {
                        gate: gate.to_string(),
                    },
                });
            }
        }
        RawGateCondition::DependenciesResolved => {
            // No id references to check: the condition reads `dependency`
            // relations by kind, not by id, and the resolved targets are
            // supplied by the runtime rather than declared in the spec.
        }
    }
}

/// Checks within-dimension duplicate states and label references on states.
fn check_state_dimensions(
    spec: &RawWorkflowSpec,
    labels: &HashSet<String>,
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
        }
    }
}

/// Returns the label id referenced by a raw effect, if any.
fn effect_label(effect: &RawEffect) -> Option<&str> {
    match effect {
        RawEffect::AddLabel { label } | RawEffect::RemoveLabel { label } => Some(label),
        RawEffect::SetAssignee { .. }
        | RawEffect::RemoveAssignee { .. }
        | RawEffect::CreateComment { .. }
        | RawEffect::CreatePullRequest { .. }
        | RawEffect::MergePullRequest => None,
    }
}

/// Returns the role id referenced by a raw effect, if any.
fn effect_role(effect: &RawEffect) -> Option<&str> {
    match effect {
        RawEffect::SetAssignee { role } | RawEffect::RemoveAssignee { role } => Some(role),
        RawEffect::AddLabel { .. }
        | RawEffect::RemoveLabel { .. }
        | RawEffect::CreateComment { .. }
        | RawEffect::CreatePullRequest { .. }
        | RawEffect::MergePullRequest => None,
    }
}

/// Converts a checked spec into the typed validated model.
fn build_validated(spec: &RawWorkflowSpec) -> ValidatedWorkflow {
    let roles = spec
        .roles
        .iter()
        .map(|role| ValidatedRole {
            id: RoleId::new(&role.id),
            charter: role.charter.clone(),
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
        })
        .collect();

    let state_dimensions = spec
        .state_dimensions
        .iter()
        .map(|dimension| ValidatedStateDimension {
            id: StateDimensionId::new(&dimension.id),
            exclusive: dimension.exclusive,
            states: dimension
                .states
                .iter()
                .map(|state| ValidatedState {
                    id: StateId::new(&state.id),
                    label: state.label.as_ref().map(LabelId::new),
                })
                .collect(),
        })
        .collect();

    let queues = spec
        .queues
        .iter()
        .map(|queue| ValidatedQueue {
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
        })
        .collect();

    let transitions = spec
        .transitions
        .iter()
        .map(|transition| ValidatedTransition {
            id: TransitionId::new(&transition.id),
            artifact: ArtifactKindId::new(&transition.artifact),
            roles: transition.roles.iter().map(RoleId::new).collect(),
            requires_gates: transition.requires_gates.iter().map(GateId::new).collect(),
            effects: transition.effects.iter().map(build_effect).collect(),
        })
        .collect();

    let gates = spec
        .gates
        .iter()
        .map(|gate| ValidatedGate {
            id: GateId::new(&gate.id),
            satisfied_by: gate.satisfied_by.iter().map(TransitionId::new).collect(),
            condition: gate.condition.as_ref().map(build_gate_condition),
        })
        .collect();

    let relations = spec
        .relations
        .iter()
        .map(|relation| ValidatedRelation {
            kind: relation.kind,
            source: ArtifactKindId::new(&relation.source),
            target: ArtifactKindId::new(&relation.target),
        })
        .collect();

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
    )
}

fn build_gate_condition(condition: &RawGateCondition) -> GateCondition {
    match condition {
        RawGateCondition::LabelPresent { label } => {
            GateCondition::LabelPresent(LabelId::new(label))
        }
        RawGateCondition::DependenciesResolved => GateCondition::DependenciesResolved,
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
        RawEffect::RemoveLabel { label } => Effect::RemoveLabel(LabelId::new(label)),
        RawEffect::SetAssignee { role } => Effect::SetAssignee(RoleId::new(role)),
        RawEffect::RemoveAssignee { role } => Effect::RemoveAssignee(RoleId::new(role)),
        RawEffect::CreateComment { body } => Effect::CreateComment { body: body.clone() },
        RawEffect::CreatePullRequest { correlation_key } => Effect::CreatePullRequest {
            correlation_key: correlation_key.clone(),
        },
        RawEffect::MergePullRequest => Effect::MergePullRequest,
    }
}
