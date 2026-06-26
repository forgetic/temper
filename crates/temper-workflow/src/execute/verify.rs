//! Postcondition verification and the reconciler's label-only apply path.
//!
//! Split from the sibling `apply` module to keep both files within the
//! source-size budget. It holds the committed-state representation
//! ([`AppliedState`]), postcondition checks, and
//! [`apply_label_effects`](Executor::apply_label_effects) — the reconciler
//! applier's idempotent reuse of the executor's label-apply path.

use super::{ExecutionError, Executor};
use crate::classify::ArtifactSource;
use crate::plan::{Postcondition, WorkflowEffect};
use std::collections::HashSet;
use temper_forge::{Forge, RepositoryId, UserId};

/// Labels and assignees returned by the backend immediately after the commit update.
pub(super) struct AppliedState {
    pub(super) labels: Vec<String>,
    pub(super) assignees: Vec<UserId>,
}

impl<F: Forge + ?Sized> Executor<'_, F> {
    pub(super) async fn verify_current(
        &self,
        repo_id: &RepositoryId,
        target: ArtifactSource,
        postconditions: &[Postcondition],
    ) -> Result<(), ExecutionError> {
        if postconditions.is_empty() {
            return Ok(());
        }
        let state = self.current_state(repo_id, target).await?;
        self.verify_state(&state, postconditions)
    }

    pub(super) fn verify_state(
        &self,
        state: &AppliedState,
        postconditions: &[Postcondition],
    ) -> Result<(), ExecutionError> {
        for postcondition in postconditions {
            let satisfied = match postcondition {
                Postcondition::LabelPresent(label) => state
                    .labels
                    .iter()
                    .any(|label_name| label_name == label.as_str()),
                Postcondition::LabelAbsent(label) => state
                    .labels
                    .iter()
                    .all(|label_name| label_name != label.as_str()),
                Postcondition::AssigneePresent { role } => {
                    let user = self.resolve_assignee(role)?;
                    state.assignees.contains(&user)
                }
                Postcondition::AssigneeAbsent { role } => {
                    let user = self.resolve_assignee(role)?;
                    !state.assignees.contains(&user)
                }
            };
            if !satisfied {
                return Err(ExecutionError::PostconditionFailed {
                    postcondition: postcondition.clone(),
                });
            }
        }
        Ok(())
    }

    /// Re-applies label effects against fresh state, idempotently.
    ///
    /// This is the reconciler applier's reuse of the executor's label-apply
    /// path for partial transitions and mechanical unblocks. It loads fresh
    /// state, applies only missing label effects through the normal update path,
    /// verifies labels, and returns the effects it actually applied. Non-label
    /// effects are rejected.
    pub(crate) async fn apply_label_effects(
        &self,
        repo_id: &RepositoryId,
        target: ArtifactSource,
        effects: &[WorkflowEffect],
    ) -> Result<Vec<WorkflowEffect>, ExecutionError> {
        let loaded = self.load(repo_id, target).await?;
        let labels: HashSet<&str> = loaded
            .classified()
            .labels
            .iter()
            .map(String::as_str)
            .collect();
        let mut prepared = super::apply::PreparedEffects::default();
        let mut postconditions = Vec::new();
        let mut applied = Vec::new();
        for effect in effects {
            match effect {
                WorkflowEffect::AddLabel(label) => {
                    postconditions.push(Postcondition::LabelPresent(label.clone()));
                    if !labels.contains(label.as_str()) {
                        prepared.push_add_label(label.as_str().to_string());
                        applied.push(effect.clone());
                    }
                }
                WorkflowEffect::RemoveLabel(label) => {
                    postconditions.push(Postcondition::LabelAbsent(label.clone()));
                    if labels.contains(label.as_str()) {
                        prepared.push_remove_label(label.as_str().to_string());
                        applied.push(effect.clone());
                    }
                }
                other => {
                    return Err(ExecutionError::UnsupportedEffect {
                        effect: other.clone(),
                    });
                }
            }
        }
        let committed = self.apply_update(repo_id, &loaded, None, prepared).await?;
        if let Some(state) = committed {
            self.verify_state(&state, &postconditions)?;
        } else {
            self.verify_current(repo_id, target, &postconditions)
                .await?;
        }
        Ok(applied)
    }

    /// Reads the artifact's current labels and assignees from fresh Forge state.
    async fn current_state(
        &self,
        repo_id: &RepositoryId,
        target: ArtifactSource,
    ) -> Result<AppliedState, ExecutionError> {
        match target {
            ArtifactSource::Issue { number } => {
                let issue = self
                    .forge
                    .get_issue_by_number(repo_id, number)
                    .await?
                    .ok_or(ExecutionError::TargetMissing { target })?;
                Ok(AppliedState {
                    labels: issue.labels,
                    assignees: issue.assignees,
                })
            }
            ArtifactSource::PullRequest { number } => {
                let pull_request = self
                    .forge
                    .get_pull_request_by_number(repo_id, number)
                    .await?
                    .ok_or(ExecutionError::TargetMissing { target })?;
                Ok(AppliedState {
                    labels: pull_request.labels,
                    assignees: pull_request.assignees,
                })
            }
        }
    }
}
