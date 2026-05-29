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
use crate::spec::{RawEffect, RawWorkflowSpec};
use crate::validated::{
    Effect, ValidatedArtifactKind, ValidatedGate, ValidatedQueue, ValidatedRole, ValidatedState,
    ValidatedStateDimension, ValidatedTransition, ValidatedWorkflow,
};
use crate::ValidationErrors;
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
    // State dimension ids are only used for duplicate detection; nothing
    // references a dimension by id, so the returned set is discarded.
    collect_declared(
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
        for label in &artifact.labels {
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

    for queue in &spec.queues {
        check_reference(
            declared.artifacts,
            &queue.artifact,
            SymbolKind::ArtifactKind,
            ReferenceSite::QueueArtifact {
                queue: queue.id.clone(),
            },
            diagnostics,
        );
        for label in &queue.labels {
            check_reference(
                declared.labels,
                label,
                SymbolKind::Label,
                ReferenceSite::QueueLabel {
                    queue: queue.id.clone(),
                },
                diagnostics,
            );
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
            let label = effect_label(effect);
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

/// Returns the label id referenced by a raw effect.
fn effect_label(effect: &RawEffect) -> &str {
    match effect {
        RawEffect::AddLabel { label } | RawEffect::RemoveLabel { label } => label,
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
            queues: role.queues.iter().map(QueueId::new).collect(),
        })
        .collect();

    let labels = spec.labels.iter().map(|l| LabelId::new(&l.id)).collect();

    let artifact_kinds = spec
        .artifact_kinds
        .iter()
        .map(|artifact| ValidatedArtifactKind {
            id: ArtifactKindId::new(&artifact.id),
            labels: artifact.labels.iter().map(LabelId::new).collect(),
        })
        .collect();

    let state_dimensions = spec
        .state_dimensions
        .iter()
        .map(|dimension| ValidatedStateDimension {
            id: StateDimensionId::new(&dimension.id),
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
            artifact: ArtifactKindId::new(&queue.artifact),
            labels: queue.labels.iter().map(LabelId::new).collect(),
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
    )
}

/// Converts a raw effect into a typed effect.
fn build_effect(effect: &RawEffect) -> Effect {
    match effect {
        RawEffect::AddLabel { label } => Effect::AddLabel(LabelId::new(label)),
        RawEffect::RemoveLabel { label } => Effect::RemoveLabel(LabelId::new(label)),
    }
}
