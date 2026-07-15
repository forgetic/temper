//! Child wiring and parent aggregation pass.

use super::{FanOutMetrics, ParentDependencyStyle, metadata_error};
use crate::artifact::ArtifactRef;
use crate::metadata::{
    CreateIssueIntentChild, CreateIssuesIntent, parse_metadata_block, replace_metadata_block,
};
use std::collections::{BTreeMap, BTreeSet};
use temper_forge::{Forge, ForgeError, Issue, ItemNumber, RepositoryId, UpdateIssue};

use super::super::{ChildIssueCheckpoint, ExecutionError, Executor};

impl<F: Forge + ?Sized> Executor<'_, F> {
    /// Writes each dependent child's complete sorted dependency list once, then
    /// aggregates all child refs and wiring progress in one parent update.
    pub(super) async fn wiring_and_aggregation_pass(
        &self,
        repo_id: &RepositoryId,
        key: &str,
        mut intent: CreateIssuesIntent,
        mut parent: Issue,
        mut issues: Vec<Issue>,
        metrics: &mut FanOutMetrics,
    ) -> Result<(CreateIssuesIntent, Issue, Vec<Issue>), ExecutionError> {
        let child_numbers = child_number_map(&intent)?;
        for (child, issue_slot) in intent.children.iter_mut().zip(&mut issues) {
            if child.wired {
                continue;
            }
            let dependencies = dependency_refs(child, &child_numbers);
            if !dependencies.is_empty() {
                let (issue, changed) = self
                    .write_complete_child_dependencies(issue_slot.clone(), &dependencies, metrics)
                    .await?;
                *issue_slot = issue;
                if changed {
                    self.child_issue_checkpoint(ChildIssueCheckpoint::Wired)
                        .await;
                }
            }
            child.wired = true;
        }

        if !intent.parent_wired {
            let parent_dependencies = parent_dependency_refs(repo_id, &intent)?;
            intent.parent_wired = true;
            let (committed, changed) = self
                .aggregate_create_intent(parent, key, &intent, &parent_dependencies, metrics)
                .await?;
            parent = committed;
            if changed {
                self.child_issue_checkpoint(ChildIssueCheckpoint::ParentAggregated)
                    .await;
            }
        }
        Ok((intent, parent, issues))
    }

    async fn write_complete_child_dependencies(
        &self,
        mut issue: Issue,
        dependencies: &[ArtifactRef],
        metrics: &mut FanOutMetrics,
    ) -> Result<(Issue, bool), ExecutionError> {
        for _ in 0..3 {
            let mut metadata = parse_metadata_block(&issue.body)
                .map_err(metadata_error)?
                .unwrap_or_default();
            if metadata.dependencies == dependencies {
                return Ok((issue, false));
            }
            metadata.dependencies = dependencies.to_vec();
            let body = replace_metadata_block(&issue.body, &metadata).map_err(metadata_error)?;
            metrics.write();
            match self
                .forge
                .update_issue_from_snapshot(
                    &issue,
                    UpdateIssue {
                        body: Some(body),
                        // Staged children are intent-owned and cannot be
                        // dispatched. Their body-only wiring write is safe to
                        // apply from the supplied snapshot without a provider
                        // CAS preflight.
                        expected_version: None,
                        ..UpdateIssue::default()
                    },
                )
                .await
            {
                Ok(committed) => return Ok((committed, true)),
                Err(ForgeError::Conflict(_)) => {
                    metrics.read();
                    issue = self
                        .forge
                        .get_issue_with_details(&issue.id, temper_forge::ItemListDetails::summary())
                        .await?
                        .ok_or_else(|| ExecutionError::Backend {
                            message: format!(
                                "issue {:?} vanished while wiring dependencies",
                                issue.id
                            ),
                        })?;
                }
                Err(error) => return Err(error.into()),
            }
        }
        Err(ExecutionError::Backend {
            message: format!(
                "could not wire dependencies for issue #{} after concurrent updates",
                issue.number
            ),
        })
    }
}

fn child_number_map(
    intent: &CreateIssuesIntent,
) -> Result<BTreeMap<String, (RepositoryId, ItemNumber)>, ExecutionError> {
    intent
        .children
        .iter()
        .map(|child| {
            Ok((
                child.slug.clone(),
                (
                    child.repository_id.clone(),
                    child.number.ok_or_else(|| ExecutionError::Backend {
                        message: format!("intent child `{}` has no persisted number", child.slug),
                    })?,
                ),
            ))
        })
        .collect()
}

fn dependency_refs(
    child: &CreateIssueIntentChild,
    child_numbers: &BTreeMap<String, (RepositoryId, ItemNumber)>,
) -> Vec<ArtifactRef> {
    child
        .dependencies
        .iter()
        .map(|dependency_slug| {
            let (dependency_repo, dependency_number) = &child_numbers[dependency_slug];
            if dependency_repo == &child.repository_id {
                ArtifactRef::same_repo(*dependency_number)
            } else {
                ArtifactRef::in_repo(dependency_repo.clone(), *dependency_number)
            }
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn parent_dependency_refs(
    repo_id: &RepositoryId,
    intent: &CreateIssuesIntent,
) -> Result<Vec<ArtifactRef>, ExecutionError> {
    let any_cross_repo = intent
        .children
        .iter()
        .any(|child| child.repository_id != *repo_id);
    if !any_cross_repo && !intent.record_parent_dependencies {
        return Ok(Vec::new());
    }
    let style = if intent.record_parent_dependencies {
        ParentDependencyStyle::Natural
    } else {
        ParentDependencyStyle::LegacyRepoQualified
    };
    intent
        .children
        .iter()
        .map(|child| {
            let number = child.number.ok_or_else(|| ExecutionError::Backend {
                message: format!("intent child `{}` has no number", child.slug),
            })?;
            Ok(match style {
                ParentDependencyStyle::Natural if child.repository_id == *repo_id => {
                    ArtifactRef::same_repo(number)
                }
                ParentDependencyStyle::Natural | ParentDependencyStyle::LegacyRepoQualified => {
                    ArtifactRef::in_repo(child.repository_id.clone(), number)
                }
            })
        })
        .collect::<Result<BTreeSet<_>, _>>()
        .map(|dependencies| dependencies.into_iter().collect())
}
