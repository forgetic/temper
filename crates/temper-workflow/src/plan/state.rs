//! Resulting-state checks for transition planning.
//!
//! This module is split out of `plan.rs` to keep the planner's orchestration
//! code small while preserving a single place for label-projected state rules.

use super::PlanDiagnostic;
use crate::classify::ClassifiedArtifact;
use crate::ids::{ArtifactKindId, StateId};
use crate::validated::{Effect, ValidatedTransition, ValidatedWorkflow};
use std::collections::HashSet;

/// Diagnoses state dimensions that would be impossible after applying effects.
///
/// A transition may change the artifact kind by replacing identifying labels
/// (for example, `intake` -> `code`). State legality is therefore checked
/// against the most specific artifact kind that the resulting labels identify,
/// falling back to the current classified kind when the result cannot be inferred
/// unambiguously.
pub(super) fn check_resulting_states(
    workflow: &ValidatedWorkflow,
    transition: &ValidatedTransition,
    artifact: &ClassifiedArtifact,
    labels: &HashSet<&str>,
    diagnostics: &mut Vec<PlanDiagnostic>,
) {
    let result = apply_label_effects(transition, labels);
    let resulting_kind = resulting_artifact_kind(workflow, artifact, &result);

    for dimension in workflow.state_dimensions() {
        let active: Vec<_> = dimension
            .states
            .iter()
            .filter(|state| {
                state
                    .label
                    .as_ref()
                    .is_some_and(|label| result.contains(label.as_str()))
            })
            .collect();
        let active_ids: Vec<StateId> = active.iter().map(|state| state.id.clone()).collect();
        if dimension.exclusive && active_ids.len() > 1 {
            diagnostics.push(PlanDiagnostic::ImpossibleState {
                transition: transition.id.clone(),
                dimension: dimension.id.clone(),
                states: active_ids.clone(),
            });
        }
        for state in active
            .iter()
            .filter(|state| !state.allows_artifact(&resulting_kind))
        {
            diagnostics.push(PlanDiagnostic::ImpossibleState {
                transition: transition.id.clone(),
                dimension: dimension.id.clone(),
                states: vec![state.id.clone()],
            });
        }
    }
}

fn apply_label_effects(
    transition: &ValidatedTransition,
    labels: &HashSet<&str>,
) -> HashSet<String> {
    let mut result: HashSet<String> = labels.iter().map(|label| label.to_string()).collect();
    for effect in &transition.effects {
        match effect {
            Effect::AddLabel(label) => {
                result.insert(label.as_str().to_string());
            }
            Effect::RemoveLabel(label) | Effect::RemoveLabelIfPresent(label) => {
                result.remove(label.as_str());
            }
            Effect::SetAssignee(_)
            | Effect::RemoveAssignee(_)
            | Effect::CreateComment { .. }
            | Effect::CreatePullRequest { .. }
            | Effect::RequestReviewers { .. }
            | Effect::SubmitReview { .. }
            | Effect::SetBody { .. }
            | Effect::AttachReview { .. }
            | Effect::CreateIssues { .. }
            | Effect::MergePullRequest => {}
        }
    }
    result
}

fn resulting_artifact_kind(
    workflow: &ValidatedWorkflow,
    artifact: &ClassifiedArtifact,
    labels: &HashSet<String>,
) -> ArtifactKindId {
    let matches: Vec<_> = workflow
        .artifact_kinds()
        .iter()
        .filter(|kind| kind.target == artifact.target)
        .filter(|kind| !kind.identifying_labels.is_empty())
        .filter(|kind| {
            kind.identifying_labels
                .iter()
                .all(|label| labels.contains(label.as_str()))
        })
        .collect();

    let Some(max) = matches
        .iter()
        .map(|kind| kind.identifying_labels.len())
        .max()
    else {
        return artifact.kind.clone();
    };
    let top: Vec<_> = matches
        .into_iter()
        .filter(|kind| kind.identifying_labels.len() == max)
        .collect();
    if top.len() == 1 {
        top[0].id.clone()
    } else {
        artifact.kind.clone()
    }
}
