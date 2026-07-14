//! Child activation pass.

use super::{decode_intent_body, metadata_error, normalized_labels};
use crate::metadata::{
    CreateIssueIntentChild, CreateIssuesIntent, parse_metadata_block, replace_metadata_block,
};
use temper_forge::{Forge, ForgeError, Issue, UpdateIssue};

use super::super::{ChildIssueCheckpoint, ExecutionError, Executor};

impl<F: Forge + ?Sized> Executor<'_, F> {
    /// Clears staging only after child and parent wiring are complete. New-path
    /// children already carry their final labels; label reconciliation here is
    /// retained only so boolean-era intents whose staged children had no labels
    /// can recover safely in the same body update.
    pub(super) async fn activation_pass(
        &self,
        mut intent: CreateIssuesIntent,
        mut issues: Vec<Issue>,
    ) -> Result<(CreateIssuesIntent, Vec<Issue>), ExecutionError> {
        if !intent.parent_wired || intent.children.iter().any(|child| !child.wired) {
            return Err(ExecutionError::Backend {
                message: "cannot activate create-issues children before complete wiring".into(),
            });
        }
        for (child, issue_slot) in intent.children.iter_mut().zip(&mut issues) {
            if child.activated {
                continue;
            }
            let labels = self.staged_labels(child);
            let (issue, changed) = self
                .activate_staged_issue(issue_slot.clone(), &child.correlation_key, &labels)
                .await?;
            *issue_slot = issue;
            if changed {
                self.child_issue_checkpoint(ChildIssueCheckpoint::Activated)
                    .await;
            }
            child.activated = true;
        }
        Ok((intent, issues))
    }

    async fn activate_staged_issue(
        &self,
        mut issue: Issue,
        correlation_key: &str,
        labels: &[String],
    ) -> Result<(Issue, bool), ExecutionError> {
        for _ in 0..3 {
            let mut metadata = parse_metadata_block(&issue.body)
                .map_err(metadata_error)?
                .unwrap_or_default();
            if metadata.correlation_key.as_deref() != Some(correlation_key) {
                return Err(ExecutionError::Backend {
                    message: format!(
                        "intent child #{} has an unexpected correlation key",
                        issue.number
                    ),
                });
            }
            if !metadata.staged {
                // Do not regress legitimate lifecycle changes made after an
                // activation landed but before completion was checkpointed.
                return Ok((issue, false));
            }
            let add_labels = labels
                .iter()
                .filter(|label| !issue.labels.contains(label))
                .cloned()
                .collect::<Vec<_>>();
            let remove_labels = issue
                .labels
                .iter()
                .filter(|label| !labels.contains(label))
                .cloned()
                .collect::<Vec<_>>();
            let body_only = add_labels.is_empty() && remove_labels.is_empty();
            metadata.staged = false;
            let body = replace_metadata_block(&issue.body, &metadata).map_err(metadata_error)?;
            match self
                .forge
                .update_issue_from_snapshot(
                    &issue,
                    UpdateIssue {
                        body: Some(body),
                        add_labels,
                        remove_labels,
                        // New-protocol children already carry final labels and
                        // are still intent-owned while staged, so activation is
                        // a body-only unconditional write. Legacy recovery that
                        // must repair labels retains CAS protection.
                        expected_version: (!body_only).then_some(issue.version),
                        ..UpdateIssue::default()
                    },
                )
                .await
            {
                Ok(committed) => return Ok((committed, true)),
                Err(ForgeError::Conflict(_)) => {
                    issue = self
                        .forge
                        .get_issue_with_details(&issue.id, temper_forge::ItemListDetails::summary())
                        .await?
                        .ok_or_else(|| ExecutionError::Backend {
                            message: format!("intent child #{} vanished", issue.number),
                        })?;
                }
                Err(error) => return Err(error.into()),
            }
        }
        Err(ExecutionError::Backend {
            message: format!(
                "could not activate staged issue #{} after concurrent updates",
                issue.number
            ),
        })
    }

    /// Computes labels used by the atomic staged create. Dependent children get
    /// the workflow's blocked state from durable intent/body metadata without
    /// reading dependency state.
    pub(super) fn staged_labels(&self, child: &CreateIssueIntentChild) -> Vec<String> {
        let mut labels = child.final_labels.clone();
        if child.dependencies.is_empty() {
            return normalized_labels(&labels);
        }
        let kind = decode_intent_body(&child.body_hex)
            .ok()
            .and_then(|body| parse_metadata_block(&body).ok().flatten())
            .and_then(|metadata| metadata.kind);
        for dimension in self
            .workflow
            .state_dimensions()
            .iter()
            .filter(|dimension| dimension.exclusive)
        {
            let ready = dimension.states.iter().find(|state| {
                state.id.as_str() == "ready"
                    && kind.as_ref().is_none_or(|kind| state.allows_artifact(kind))
            });
            let blocked = dimension.states.iter().find(|state| {
                state.id.as_str() == "blocked"
                    && kind.as_ref().is_none_or(|kind| state.allows_artifact(kind))
            });
            if let (Some(ready), Some(blocked)) = (ready, blocked) {
                if let Some(label) = ready.label.as_ref() {
                    labels.retain(|candidate| candidate != label.as_str());
                }
                if let Some(label) = blocked.label.as_ref() {
                    labels.retain(|candidate| candidate != label.as_str());
                    labels.push(label.as_str().to_string());
                }
            }
        }
        normalized_labels(&labels)
    }
}
