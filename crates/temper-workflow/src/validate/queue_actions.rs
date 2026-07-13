//! Semantic checks for role-worker queue action assignments.

use std::collections::{HashMap, HashSet};

use crate::diagnostics::Diagnostic;
use crate::spec::{RawQueueAction, RawTransition, RawWorkflowSpec};
use crate::validate_build::build_effect;

/// Checks action authority, artifact scope, checkout vocabulary, and effects
/// that must be safe for writable pull-request repair publication.
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
            check_authority(&queue.id, action, transition, roles, diagnostics);
            check_artifact(&queue.id, &queue.artifacts, action, transition, diagnostics);
            check_checkout(&queue.id, action, transition, diagnostics);
        }
    }
}

fn check_authority(
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

fn check_artifact(
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
    if let Some(artifact) = &action.artifact {
        if artifact != &transition.artifact {
            diagnostics.push(Diagnostic::QueueActionFilterArtifactMismatch {
                queue: queue.to_string(),
                role: action.role.clone(),
                action: transition.id.clone(),
                declared_artifact: artifact.clone(),
                action_artifact: transition.artifact.clone(),
            });
        }
    }
}

fn check_checkout(
    queue: &str,
    action: &RawQueueAction,
    transition: &RawTransition,
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
        return;
    }
    if checkout != "pull_request_writable" {
        return;
    }

    for effect in &transition.effects {
        let effect = build_effect(effect);
        if !effect.supports_pull_request_repair_publication() {
            diagnostics.push(Diagnostic::QueueActionUnsupportedPullRequestRepairEffect {
                queue: queue.to_string(),
                role: action.role.clone(),
                action: transition.id.clone(),
                effect: effect.kind_name().to_string(),
            });
        }
    }
}
