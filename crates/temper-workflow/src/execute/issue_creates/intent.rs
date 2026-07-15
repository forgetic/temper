//! Parent-intent discovery and compare-and-swap pass checkpoints.

use super::super::{ExecutionError, Executor};
use super::round::{IntentRound, select_intent_round};
use super::{
    FanOutMetrics, IntentExecutionMode, PendingIntent, PersistedCreateIntent, decode_intent_body,
    metadata_error,
};
use crate::classify::ArtifactSource;
use crate::metadata::{
    CreateIssuesCompletion, CreateIssuesIntent, parse_metadata_block, replace_metadata_block,
};
use std::collections::{BTreeMap, BTreeSet};
use temper_forge::{
    Forge, ForgeError, Issue, IssueQuery, IssueState, ItemListDetails, RepositoryId, UpdateIssue,
};

impl<F: Forge + ?Sized> Executor<'_, F> {
    /// Finds incomplete parent intents with bounded, state-explicit body-marker
    /// queries and resumes them without worker-owned runtime context.
    /// Daemon startup code runs this before opening its dispatch barrier.
    pub async fn recover_create_issue_intents(
        &self,
        repo_id: &RepositoryId,
    ) -> Result<usize, ExecutionError> {
        let mut parents = BTreeMap::new();
        for state in [IssueState::Open, IssueState::Closed] {
            for issue in self
                .forge
                .list_issues(
                    repo_id,
                    IssueQuery {
                        limit: None,
                        state: Some(state),
                        labels: Vec::new(),
                        body_contains: Some("\"create_issue_intents\"".into()),
                        author_id: None,
                        assignee_id: None,
                        sort: None,
                        details: ItemListDetails::summary(),
                    },
                )
                .await?
            {
                parents.insert(issue.id.clone(), issue);
            }
        }

        let mut recovered = 0;
        for summary in parents.into_values() {
            let started = std::time::Instant::now();
            let provider_requests_before = self.forge.provider_request_count();
            let mut metrics = FanOutMetrics::default();
            // Summary list responses may truncate a large persisted intent.
            // Reload the selected parent by id before parsing authoritative data.
            metrics.read();
            let Some(mut parent) = self
                .forge
                .get_issue_with_details(&summary.id, ItemListDetails::summary())
                .await?
            else {
                continue;
            };
            let Some(metadata) = parse_metadata_block(&parent.body).map_err(metadata_error)? else {
                continue;
            };
            let incomplete = metadata
                .create_issue_intents
                .into_iter()
                .filter(|(_, intent)| !intent.completed)
                .collect::<Vec<_>>();
            if incomplete.is_empty() {
                continue;
            }
            let child_count = incomplete
                .iter()
                .map(|(_, intent)| intent.children.len())
                .sum();
            let dependency_edge_count = incomplete
                .iter()
                .flat_map(|(_, intent)| &intent.children)
                .map(|child| child.dependencies.len())
                .sum();
            let parent_number = parent.number;

            let mut completed = Vec::with_capacity(incomplete.len());
            for (key, intent) in incomplete {
                let resumed = self
                    .resume_create_intent(
                        repo_id,
                        parent.number,
                        &key,
                        intent,
                        IntentExecutionMode::Recovery,
                        parent,
                        &mut metrics,
                    )
                    .await?;
                parent = resumed.parent;
                completed.push(PendingIntent {
                    key: resumed.key,
                    intent: resumed.intent,
                });
                recovered += 1;
            }
            let (committed, changed) = self
                .complete_create_intents(parent, &completed, &mut metrics)
                .await?;
            if changed {
                self.child_issue_checkpoint(super::super::ChildIssueCheckpoint::Completed)
                    .await;
            }
            let provider_requests = provider_requests_before.and_then(|before| {
                self.forge
                    .provider_request_count()
                    .map(|after| after.saturating_sub(before))
            });
            super::emit_fan_out_completion(
                repo_id,
                parent_number,
                child_count,
                dependency_edge_count,
                &metrics,
                provider_requests,
                true,
                started.elapsed(),
            );
            drop(committed);
        }
        Ok(recovered)
    }

    /// Inserts or resumes the durable round for this fan-out request.
    ///
    /// An incomplete latest round is the caller's recovery signal and must
    /// match the normalized request. A completed latest round is reused only
    /// when both its request and its atomic source completion still match the
    /// current source. Otherwise this is a later logical execution and receives
    /// a fresh round key plus round-qualified child correlation keys.
    pub(super) async fn persist_create_intent(
        &self,
        mut parent: Issue,
        base_key: &str,
        proposed: CreateIssuesIntent,
        metrics: &mut FanOutMetrics,
    ) -> Result<PersistedCreateIntent, ExecutionError> {
        for _ in 0..3 {
            let mut metadata = parse_metadata_block(&parent.body)
                .map_err(metadata_error)?
                .unwrap_or_default();
            match select_intent_round(&metadata, &parent, base_key, &proposed)? {
                IntentRound::Existing { key, intent } => {
                    return Ok(PersistedCreateIntent {
                        key,
                        newly_inserted: false,
                        intent,
                        parent,
                    });
                }
                IntentRound::Insert { key, intent } => {
                    metadata
                        .create_issue_intents
                        .insert(key.clone(), intent.clone());
                    let body =
                        replace_metadata_block(&parent.body, &metadata).map_err(metadata_error)?;
                    metrics.write();
                    match self
                        .forge
                        .update_issue_from_snapshot(
                            &parent,
                            UpdateIssue {
                                body: Some(body),
                                expected_version: Some(parent.version),
                                ..UpdateIssue::default()
                            },
                        )
                        .await
                    {
                        Ok(committed) => {
                            return Ok(PersistedCreateIntent {
                                key,
                                newly_inserted: true,
                                intent,
                                parent: committed,
                            });
                        }
                        Err(ForgeError::Conflict(_)) => {
                            parent = self.reload_parent(&parent, metrics).await?;
                        }
                        Err(error) => return Err(error.into()),
                    }
                }
            }
        }
        Err(ExecutionError::Backend {
            message: "could not persist create-issues intent after concurrent updates".into(),
        })
    }

    /// Checkpoints every returned child number in one source update after the
    /// complete create pass.
    pub(super) async fn save_create_intent(
        &self,
        mut parent: Issue,
        key: &str,
        intent: &CreateIssuesIntent,
        metrics: &mut FanOutMetrics,
    ) -> Result<Issue, ExecutionError> {
        for _ in 0..3 {
            let mut metadata = parse_metadata_block(&parent.body)
                .map_err(metadata_error)?
                .unwrap_or_default();
            if metadata
                .create_issue_intents
                .get(key)
                .is_some_and(|current| current.completed)
            {
                return Ok(parent);
            }
            metadata
                .create_issue_intents
                .insert(key.to_string(), intent.clone());
            let body = replace_metadata_block(&parent.body, &metadata).map_err(metadata_error)?;
            metrics.write();
            match self
                .forge
                .update_issue_from_snapshot(
                    &parent,
                    UpdateIssue {
                        body: Some(body),
                        expected_version: Some(parent.version),
                        ..UpdateIssue::default()
                    },
                )
                .await
            {
                Ok(committed) => return Ok(committed),
                Err(ForgeError::Conflict(_)) => {
                    parent = self.reload_parent(&parent, metrics).await?;
                }
                Err(error) => return Err(error.into()),
            }
        }
        Err(ExecutionError::Backend {
            message: "could not checkpoint create-issues create pass after concurrent updates"
                .into(),
        })
    }

    /// Atomically aggregates child references, all wired flags, and
    /// `parent_wired` into one source metadata update.
    pub(super) async fn aggregate_create_intent(
        &self,
        mut parent: Issue,
        key: &str,
        intent: &CreateIssuesIntent,
        child_dependencies: &[crate::artifact::ArtifactRef],
        metrics: &mut FanOutMetrics,
    ) -> Result<(Issue, bool), ExecutionError> {
        for _ in 0..3 {
            let mut metadata = parse_metadata_block(&parent.body)
                .map_err(metadata_error)?
                .unwrap_or_default();
            if metadata
                .create_issue_intents
                .get(key)
                .is_some_and(|current| current.parent_wired)
            {
                return Ok((parent, false));
            }
            metadata.dependencies.extend_from_slice(child_dependencies);
            metadata.dependencies = metadata
                .dependencies
                .into_iter()
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            metadata
                .create_issue_intents
                .insert(key.to_string(), intent.clone());
            let body = replace_metadata_block(&parent.body, &metadata).map_err(metadata_error)?;
            metrics.write();
            match self
                .forge
                .update_issue_from_snapshot(
                    &parent,
                    UpdateIssue {
                        body: Some(body),
                        expected_version: Some(parent.version),
                        ..UpdateIssue::default()
                    },
                )
                .await
            {
                Ok(committed) => return Ok((committed, true)),
                Err(ForgeError::Conflict(_)) => {
                    parent = self.reload_parent(&parent, metrics).await?;
                }
                Err(error) => return Err(error.into()),
            }
        }
        Err(ExecutionError::Backend {
            message: "could not aggregate create-issues wiring after concurrent updates".into(),
        })
    }

    /// Commits child activation progress, `completed=true`, and the routed
    /// source transition's body/labels/assignees in one final source update.
    pub(super) async fn complete_create_intents(
        &self,
        mut parent: Issue,
        intents: &[PendingIntent],
        metrics: &mut FanOutMetrics,
    ) -> Result<(Issue, bool), ExecutionError> {
        if intents.is_empty() {
            return Ok((parent, false));
        }
        for pending in intents {
            if !pending.intent.parent_wired
                || pending
                    .intent
                    .children
                    .iter()
                    .any(|child| child.number.is_none() || !child.wired || !child.activated)
            {
                return Err(ExecutionError::Backend {
                    message: format!(
                        "create-issues intent `{}` reached completion before every pass finished",
                        pending.key
                    ),
                });
            }
        }
        let completion = common_completion(intents)?;

        for _ in 0..3 {
            let mut metadata = parse_metadata_block(&parent.body)
                .map_err(metadata_error)?
                .unwrap_or_default();
            let already_completed = intents.iter().all(|pending| {
                metadata
                    .create_issue_intents
                    .get(&pending.key)
                    .is_some_and(|intent| intent.completed)
            });
            if already_completed {
                return Ok((parent, false));
            }

            for pending in intents {
                let mut intent = pending.intent.clone();
                intent.completed = true;
                metadata
                    .create_issue_intents
                    .insert(pending.key.clone(), intent);
            }
            let base_body = match completion.and_then(|completion| completion.body_hex.as_deref()) {
                Some(encoded) => decode_intent_body(encoded)?,
                None => parent.body.clone(),
            };
            let body = replace_metadata_block(&base_body, &metadata).map_err(metadata_error)?;
            let completion = completion.cloned().unwrap_or_default();
            metrics.write();
            match self
                .forge
                .update_issue_from_snapshot(
                    &parent,
                    UpdateIssue {
                        body: Some(body),
                        add_labels: completion.add_labels,
                        remove_labels: completion.remove_labels,
                        add_assignees: completion.add_assignees,
                        remove_assignees: completion.remove_assignees,
                        expected_version: Some(parent.version),
                        ..UpdateIssue::default()
                    },
                )
                .await
            {
                Ok(committed) => return Ok((committed, true)),
                Err(ForgeError::Conflict(_)) => {
                    parent = self.reload_parent(&parent, metrics).await?;
                }
                Err(error) => return Err(error.into()),
            }
        }
        Err(ExecutionError::Backend {
            message: "could not complete create-issues passes after concurrent updates".into(),
        })
    }

    async fn reload_parent(
        &self,
        parent: &Issue,
        metrics: &mut FanOutMetrics,
    ) -> Result<Issue, ExecutionError> {
        metrics.read();
        self.forge
            .get_issue_with_details(&parent.id, ItemListDetails::summary())
            .await?
            .ok_or(ExecutionError::TargetMissing {
                target: ArtifactSource::Issue {
                    number: parent.number,
                },
            })
    }
}

fn common_completion(
    intents: &[PendingIntent],
) -> Result<Option<&CreateIssuesCompletion>, ExecutionError> {
    let mut completion = None;
    for candidate in intents
        .iter()
        .filter_map(|pending| pending.intent.completion.as_ref())
    {
        if completion.is_some_and(|current| current != candidate) {
            return Err(ExecutionError::Backend {
                message: "create-issues effects disagree on their source completion update".into(),
            });
        }
        completion = Some(candidate);
    }
    Ok(completion)
}
