//! Semantic-consistency contract checks for workflow validation.
//!
//! These checks run after simple undeclared-reference diagnostics are collected.
//! They cover duplicate role external tools, default artifact-kind uniqueness,
//! and the queue-automation and per-transition outcome-routing authority/artifact
//! contracts. Split from the validation root to keep each file within the
//! source-size budget.

use crate::diagnostics::Diagnostic;
use crate::spec::{
    RawEffect, RawQueueAutomation, RawTransition, RawWorkflowSpec, TargetBranchPolicy,
};
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

/// Rejects a terminal queue whose discovery superset would be unlabelled.
///
/// Positive common/alternative labels are sufficient evidence. A condition-only
/// queue may fall back to artifact-kind identifying labels, but a catch-all kind
/// has no bounded terminal representation and is therefore rejected.
pub(super) fn check_terminal_queue_contract(
    spec: &RawWorkflowSpec,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let artifacts = spec
        .artifact_kinds
        .iter()
        .map(|artifact| (artifact.id.as_str(), artifact))
        .collect::<HashMap<_, _>>();
    for queue in spec.queues.iter().filter(|queue| queue.terminal) {
        let has_positive_labels = !queue.labels.is_empty()
            || (!queue.any_of.is_empty()
                && queue.any_of.iter().all(|branch| !branch.labels.is_empty()));
        if has_positive_labels {
            continue;
        }
        let unlabelled = queue
            .artifacts
            .iter()
            .filter(|artifact| {
                artifacts
                    .get(artifact.as_str())
                    .is_some_and(|kind| kind.identifying_labels.is_empty())
            })
            .cloned()
            .collect::<Vec<_>>();
        if !unlabelled.is_empty() {
            diagnostics.push(Diagnostic::UnfilteredTerminalQueue {
                queue: queue.id.clone(),
                artifacts: unlabelled,
            });
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

/// Checks semantic consistency of validation bindings once simple
/// undeclared-reference diagnostics have been collected.
pub(super) fn check_validation_binding_contract(
    spec: &RawWorkflowSpec,
    roles: &HashSet<String>,
    artifacts: &HashSet<String>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let transitions: HashMap<&str, &RawTransition> = spec
        .transitions
        .iter()
        .map(|transition| (transition.id.as_str(), transition))
        .collect();

    for binding in &spec.validation_bindings {
        let Some(action) = transitions.get(binding.action.as_str()).copied() else {
            continue;
        };
        if roles.contains(&binding.role) && !action.roles.contains(&binding.role) {
            diagnostics.push(Diagnostic::ValidationBindingActionUnauthorized {
                binding: binding.id.clone(),
                role: binding.role.clone(),
                action: action.id.clone(),
            });
        }
        if artifacts.contains(&binding.target_artifact)
            && action.artifact != binding.target_artifact
        {
            diagnostics.push(Diagnostic::ValidationBindingActionArtifactMismatch {
                binding: binding.id.clone(),
                action: action.id.clone(),
                target_artifact: binding.target_artifact.clone(),
                action_artifact: action.artifact.clone(),
            });
        }
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

/// Checks that `create_pull_request.artifact_kind`, when supplied, names a
/// pull-request artifact kind. Undeclared kinds are reported by the reference
/// pass; this semantic pass only checks declared kinds.
pub(super) fn check_create_pull_request_artifact_kind_targets(
    spec: &RawWorkflowSpec,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let artifacts: HashMap<&str, crate::ArtifactTarget> = spec
        .artifact_kinds
        .iter()
        .map(|kind| (kind.id.as_str(), kind.target))
        .collect();

    for transition in &spec.transitions {
        for effect in &transition.effects {
            let RawEffect::CreatePullRequest {
                artifact_kind: Some(artifact_kind),
                ..
            } = effect
            else {
                continue;
            };
            let Some(target) = artifacts.get(artifact_kind.as_str()).copied() else {
                continue;
            };
            if target != crate::ArtifactTarget::PullRequest {
                diagnostics.push(Diagnostic::CreatePullRequestArtifactKindTargetMismatch {
                    transition: transition.id.clone(),
                    artifact_kind: artifact_kind.clone(),
                    target: target.to_string(),
                });
            }
        }
    }
}

/// Checks that target-branch policy semantics are supported by their effect.
///
/// Child creation can derive, inherit, or explicitly select the repository
/// default. Pull-request creation consumes a branch and can require it to be
/// non-default or explicitly permit repository-default/same-branch behavior.
pub(super) fn check_target_branch_policy_contract(
    spec: &RawWorkflowSpec,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for transition in &spec.transitions {
        for effect in &transition.effects {
            let unsupported = match effect {
                RawEffect::CreateIssues {
                    target_branch_policy: Some(TargetBranchPolicy::NonDefault),
                    ..
                } => Some(("create_issues", TargetBranchPolicy::NonDefault)),
                RawEffect::CreatePullRequest {
                    target_branch_policy:
                        Some(
                            policy @ (TargetBranchPolicy::DerivedFeatureBranch
                            | TargetBranchPolicy::Inherit),
                        ),
                    ..
                } => Some(("create_pull_request", *policy)),
                _ => None,
            };
            if let Some((effect, policy)) = unsupported {
                diagnostics.push(Diagnostic::UnsupportedTargetBranchPolicy {
                    transition: transition.id.clone(),
                    effect: effect.to_string(),
                    policy,
                });
            }
        }
    }
}

/// Checks that every child-producing effect has a useful, ordered cardinality.
pub(super) fn check_create_issues_cardinality(
    spec: &RawWorkflowSpec,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for transition in &spec.transitions {
        for effect in &transition.effects {
            let RawEffect::CreateIssues {
                min_children,
                max_children,
                ..
            } = effect
            else {
                continue;
            };
            if *min_children == 0 || max_children.is_some_and(|max| max < *min_children) {
                diagnostics.push(Diagnostic::InvalidCreateIssuesCardinality {
                    transition: transition.id.clone(),
                    min_children: *min_children,
                    max_children: *max_children,
                });
            }
        }
    }
}

pub(super) fn check_create_issues_child_kind_requirements(
    spec: &RawWorkflowSpec,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let artifacts = spec
        .artifact_kinds
        .iter()
        .map(|artifact| (artifact.id.as_str(), artifact.target))
        .collect::<HashMap<_, _>>();
    for transition in &spec.transitions {
        for effect in &transition.effects {
            let RawEffect::CreateIssues {
                child_kind_requirements,
                ..
            } = effect
            else {
                continue;
            };
            let mut seen = HashSet::new();
            let required_kinds = child_kind_requirements
                .iter()
                .map(|requirement| requirement.kind.as_str())
                .collect::<HashSet<_>>();
            for requirement in child_kind_requirements {
                let kind = requirement.kind.trim();
                let reason = if kind.is_empty() {
                    Some("kind must not be blank".to_string())
                } else if !seen.insert(kind) {
                    Some("kind is required more than once".to_string())
                } else if requirement.min_children == 0
                    || requirement
                        .max_children
                        .is_some_and(|max| max < requirement.min_children)
                {
                    Some("cardinality must require at least one child and max_children must be >= min_children".to_string())
                } else if artifacts.get(kind) != Some(&crate::ArtifactTarget::Issue) {
                    Some("kind must name a declared issue artifact".to_string())
                } else if !spec.relations.iter().any(|relation| {
                    relation.kind == crate::RelationKind::Parent
                        && relation.source == kind
                        && relation.target == transition.artifact
                }) {
                    Some(format!(
                        "workflow must declare a parent relation from `{kind}` to `{}`",
                        transition.artifact
                    ))
                } else if requirement
                    .depends_on_all_kinds
                    .iter()
                    .any(|dependency| dependency == kind)
                {
                    Some(
                        "depends_on_all_kinds cannot contain the requirement's own kind"
                            .to_string(),
                    )
                } else if let Some(unknown) = requirement
                    .depends_on_all_kinds
                    .iter()
                    .find(|dependency| !required_kinds.contains(dependency.as_str()))
                {
                    Some(format!(
                        "depends_on_all_kinds references non-required kind `{unknown}`"
                    ))
                } else {
                    requirement
                        .depends_on_all_kinds
                        .iter()
                        .find(|dependency| {
                            !spec.relations.iter().any(|relation| {
                                relation.kind == crate::RelationKind::Dependency
                                    && relation.source == kind
                                    && relation.target == dependency.as_str()
                            })
                        })
                        .map(|dependency| {
                            format!(
                                "workflow must declare a dependency relation from `{kind}` to `{dependency}`"
                            )
                        })
                };
                if let Some(reason) = reason {
                    diagnostics.push(Diagnostic::InvalidChildKindRequirement {
                        transition: transition.id.clone(),
                        kind: requirement.kind.clone(),
                        reason,
                    });
                }
            }
        }
    }
}

/// Checks semantic consistency of per-transition outcome routing (the
/// workspace-verdict path for assigned actions).
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
/// actor role's external tools, matching the assigned-job contract that an
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
