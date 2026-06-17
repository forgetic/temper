//! Semantic-consistency contract checks for workflow validation.
//!
//! These checks run after simple undeclared-reference diagnostics are collected.
//! They cover duplicate role external tools, default artifact-kind uniqueness,
//! and the queue-automation and per-transition outcome-routing authority/artifact
//! contracts. Split from the validation root to keep each file within the
//! source-size budget.

use crate::diagnostics::Diagnostic;
use crate::spec::{RawQueueAction, RawQueueAutomation, RawTransition, RawWorkflowSpec};
use std::collections::{HashMap, HashSet};

/// Checks duplicate external tool ids within each role declaration.
pub(super) fn check_role_external_tools(spec: &RawWorkflowSpec, diagnostics: &mut Vec<Diagnostic>) {
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
pub(super) fn check_queue_automation_contract(
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
            if let Some(primary) = primary
                && outcome.artifact != primary.artifact
            {
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

/// The verdict id -> transition id outcome references declared by a queue
/// automation, with `on_merge_conflict` desugared into the built-in
/// merge-conflict verdict. An explicit `outcomes` entry wins over the sugar.
pub(super) fn automation_outcome_references(
    automation: &RawQueueAutomation,
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

/// Checks semantic consistency of queue role-worker action assignments once
/// simple undeclared-reference diagnostics have been collected.
pub(super) fn check_queue_action_contract(
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
        for action in &queue.actions {
            let Some(transition) = transitions.get(action.action.as_str()).copied() else {
                continue;
            };
            check_queue_action_authority(&queue.id, action, transition, roles, diagnostics);
            check_queue_action_artifact(
                &queue.id,
                &queue.artifacts,
                action,
                transition,
                diagnostics,
            );
            check_queue_action_checkout(&queue.id, action, diagnostics);
        }
    }
}

fn check_queue_action_authority(
    queue: &str,
    action: &RawQueueAction,
    transition: &RawTransition,
    roles: &HashSet<String>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if roles.contains(&action.role) && !transition.roles.iter().any(|role| role == &action.role) {
        diagnostics.push(Diagnostic::QueueActionUnauthorized {
            queue: queue.to_string(),
            role: action.role.clone(),
            action: transition.id.clone(),
        });
    }
}

fn check_queue_action_artifact(
    queue: &str,
    queue_artifacts: &[String],
    action: &RawQueueAction,
    transition: &RawTransition,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if !queue_artifacts.contains(&transition.artifact) {
        diagnostics.push(Diagnostic::QueueActionArtifactMismatch {
            queue: queue.to_string(),
            action: transition.id.clone(),
            artifact: transition.artifact.clone(),
            queue_artifacts: queue_artifacts.to_vec(),
        });
    }
    if let Some(artifact) = &action.artifact
        && artifact != &transition.artifact
    {
        diagnostics.push(Diagnostic::QueueActionFilterArtifactMismatch {
            queue: queue.to_string(),
            role: action.role.clone(),
            action: transition.id.clone(),
            declared_artifact: artifact.clone(),
            action_artifact: transition.artifact.clone(),
        });
    }
}

fn check_queue_action_checkout(
    queue: &str,
    action: &RawQueueAction,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(checkout) = &action.checkout else {
        return;
    };
    if !matches!(
        checkout.as_str(),
        "writable" | "read_only" | "pull_request_read_only" | "pull_request_writable"
    ) {
        diagnostics.push(Diagnostic::QueueActionInvalidCheckout {
            queue: queue.to_string(),
            role: action.role.clone(),
            action: action.action.clone(),
            checkout: checkout.clone(),
        });
    }
}

/// Checks that each Forge target declares at most one default (catch-all)
/// artifact kind — one with no identifying labels. The default kind admits any
/// artifact of its target that no labeled kind claims (raw human intake), so two
/// defaults for the same target would make classification ambiguous.
pub(super) fn check_default_artifact_kinds(
    spec: &RawWorkflowSpec,
    diagnostics: &mut Vec<Diagnostic>,
) {
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
pub(super) fn check_transition_outcome_contract(
    spec: &RawWorkflowSpec,
    diagnostics: &mut Vec<Diagnostic>,
) {
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
