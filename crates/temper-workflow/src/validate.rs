//! Static validation from a [`RawWorkflowSpec`] to a [`ValidatedWorkflow`].
//!
//! Validation is diagnostic-collecting: it walks the whole spec and records
//! every problem before deciding whether the workflow is valid. Phase 2 covers
//! duplicate ids and undeclared references; later phases will add semantic
//! checks (contradictory effects, unsatisfiable gates, tool-authority limits).

use crate::diagnostics::{Diagnostic, ReferenceSite, SymbolKind};
use crate::spec::{RawEffect, RawGateCondition, RawIntakeAuthor, RawTransition, RawWorkflowSpec};
use crate::validate_build::build_validated;
use crate::validated::ValidatedWorkflow;
use crate::ValidationErrors;
use std::collections::{HashMap, HashSet};

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

    check_state_dimensions(spec, &labels, &artifacts, &mut diagnostics);
    check_role_external_tools(spec, &mut diagnostics);
    check_default_artifact_kinds(spec, &mut diagnostics);
    check_queue_automation_contract(spec, &roles, &mut diagnostics);
    check_transition_outcome_contract(spec, &mut diagnostics);

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
        | RawGateCondition::ReviewApproved
        | RawGateCondition::ReviewChangesRequested => {}
    }
}

/// Checks within-dimension duplicate states and label references on states.
fn check_state_dimensions(
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

/// Checks duplicate external tool ids within each role declaration.
fn check_role_external_tools(spec: &RawWorkflowSpec, diagnostics: &mut Vec<Diagnostic>) {
    for role in &spec.roles {
        let mut seen = HashSet::new();
        for tool in &role.external_tools {
            if !seen.insert(tool.id.clone()) {
                diagnostics.push(Diagnostic::DuplicateRoleExternalTool {
                    role: role.id.clone(),
                    id: tool.id.clone(),
                });
            }
        }
    }
}

/// Checks semantic consistency of queue automation declarations once simple
/// undeclared-reference diagnostics have been collected.
fn check_queue_automation_contract(
    spec: &RawWorkflowSpec,
    roles: &HashSet<String>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let transitions: HashMap<&str, &RawTransition> = spec
        .transitions
        .iter()
        .map(|transition| (transition.id.as_str(), transition))
        .collect();

    for queue in &spec.queues {
        let Some(automation) = &queue.automation else {
            continue;
        };
        let actor_declared = roles.contains(&automation.actor);
        let primary = transitions.get(automation.transition.as_str()).copied();

        if let Some(transition) = primary {
            check_queue_automation_authority(
                &queue.id,
                &automation.actor,
                actor_declared,
                transition,
                diagnostics,
            );
            check_queue_automation_artifact(&queue.id, &queue.artifacts, transition, diagnostics);
        }

        if let Some(executor) = &automation.executor {
            check_queue_automation_executor(
                spec,
                &queue.id,
                &automation.actor,
                actor_declared,
                executor,
                diagnostics,
            );
        }

        for (verdict, outcome_id) in automation_outcome_references(automation) {
            let Some(outcome) = transitions.get(outcome_id.as_str()).copied() else {
                continue;
            };
            if actor_declared && !outcome.roles.contains(&automation.actor) {
                diagnostics.push(Diagnostic::QueueAutomationOutcomeUnauthorized {
                    queue: queue.id.clone(),
                    verdict: verdict.clone(),
                    actor: automation.actor.clone(),
                    transition: outcome.id.clone(),
                });
            }
            if let Some(primary) = primary {
                if outcome.artifact != primary.artifact {
                    diagnostics.push(Diagnostic::QueueAutomationOutcomeArtifactMismatch {
                        queue: queue.id.clone(),
                        verdict: verdict.clone(),
                        transition: outcome.id.clone(),
                        expected: primary.artifact.clone(),
                        actual: outcome.artifact.clone(),
                    });
                }
            }
        }
    }
}

/// The verdict id -> transition id outcome references declared by a queue
/// automation, with `on_merge_conflict` desugared into the built-in
/// merge-conflict verdict. An explicit `outcomes` entry wins over the sugar.
fn automation_outcome_references(
    automation: &crate::spec::RawQueueAutomation,
) -> Vec<(String, String)> {
    let mut references: Vec<(String, String)> = automation
        .outcomes
        .iter()
        .map(|(verdict, transition)| (verdict.clone(), transition.clone()))
        .collect();
    if let Some(fallback) = &automation.on_merge_conflict {
        let verdict = crate::ids::VerdictId::merge_conflict().as_str().to_string();
        if !references.iter().any(|(existing, _)| *existing == verdict) {
            references.push((verdict, fallback.clone()));
        }
    }
    references
}

/// Checks that each Forge target declares at most one default (catch-all)
/// artifact kind — one with no identifying labels. The default kind admits any
/// artifact of its target that no labeled kind claims (raw human intake), so two
/// defaults for the same target would make classification ambiguous.
fn check_default_artifact_kinds(spec: &RawWorkflowSpec, diagnostics: &mut Vec<Diagnostic>) {
    let mut defaults_by_target: HashMap<crate::ArtifactTarget, Vec<String>> = HashMap::new();
    for kind in &spec.artifact_kinds {
        if kind.identifying_labels.is_empty() {
            defaults_by_target
                .entry(kind.target)
                .or_default()
                .push(kind.id.clone());
        }
    }
    for (target, kinds) in defaults_by_target {
        if kinds.len() > 1 {
            diagnostics.push(Diagnostic::MultipleDefaultArtifactKinds {
                target: target.to_string(),
                kinds,
            });
        }
    }
}

/// Checks semantic consistency of per-transition outcome routing (the
/// workspace-verdict path for role-decision actions).
fn check_transition_outcome_contract(spec: &RawWorkflowSpec, diagnostics: &mut Vec<Diagnostic>) {
    let transitions: HashMap<&str, &RawTransition> = spec
        .transitions
        .iter()
        .map(|transition| (transition.id.as_str(), transition))
        .collect();

    for transition in &spec.transitions {
        for (verdict, outcome_id) in &transition.outcomes {
            let Some(outcome) = transitions.get(outcome_id.as_str()).copied() else {
                continue;
            };
            let shares_role = transition.roles.iter().any(|role| {
                outcome
                    .roles
                    .iter()
                    .any(|outcome_role| outcome_role == role)
            });
            if !transition.roles.is_empty() && !outcome.roles.is_empty() && !shares_role {
                diagnostics.push(Diagnostic::TransitionOutcomeUnauthorized {
                    transition: transition.id.clone(),
                    verdict: verdict.clone(),
                    outcome_transition: outcome.id.clone(),
                });
            }
            if outcome.artifact != transition.artifact {
                diagnostics.push(Diagnostic::TransitionOutcomeArtifactMismatch {
                    transition: transition.id.clone(),
                    verdict: verdict.clone(),
                    outcome_transition: outcome.id.clone(),
                    expected: transition.artifact.clone(),
                    actual: outcome.artifact.clone(),
                });
            }
        }
    }
}

fn check_queue_automation_authority(
    queue: &str,
    actor: &str,
    actor_declared: bool,
    transition: &RawTransition,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if actor_declared && !transition.roles.iter().any(|role| role == actor) {
        diagnostics.push(Diagnostic::QueueAutomationUnauthorized {
            queue: queue.to_string(),
            actor: actor.to_string(),
            transition: transition.id.clone(),
        });
    }
}

fn check_queue_automation_artifact(
    queue: &str,
    queue_artifacts: &[String],
    transition: &RawTransition,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if !queue_artifacts.contains(&transition.artifact) {
        diagnostics.push(Diagnostic::QueueAutomationArtifactMismatch {
            queue: queue.to_string(),
            transition: transition.id.clone(),
            artifact: transition.artifact.clone(),
            queue_artifacts: queue_artifacts.to_vec(),
        });
    }
}

/// Checks that a workspace-backed automation's `executor` id is declared on the
/// actor role's external tools, mirroring the role-decision contract that an
/// executor must be a declared external tool of the role that invokes it. The
/// check is skipped when the actor role itself is undeclared (already
/// diagnosed) so a single missing role does not cascade.
fn check_queue_automation_executor(
    spec: &RawWorkflowSpec,
    queue: &str,
    actor: &str,
    actor_declared: bool,
    executor: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if !actor_declared {
        return;
    }
    let declares_executor = spec
        .roles
        .iter()
        .find(|role| role.id == actor)
        .is_some_and(|role| role.external_tools.iter().any(|tool| tool.id == executor));
    if !declares_executor {
        diagnostics.push(Diagnostic::QueueAutomationExecutorUndeclared {
            queue: queue.to_string(),
            actor: actor.to_string(),
            executor: executor.to_string(),
        });
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
        | RawEffect::RequestReviewers { .. }
        | RawEffect::SubmitReview { .. }
        | RawEffect::SetBody { .. }
        | RawEffect::AttachReview { .. }
        | RawEffect::CreateIssues { .. }
        | RawEffect::MergePullRequest => None,
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
        | RawEffect::MergePullRequest => Vec::new(),
    }
}
