//! Parent-intent discovery and compare-and-swap persistence.

use super::super::{ExecutionError, Executor};
use super::metadata_error;
use crate::classify::ArtifactSource;
use crate::metadata::{CreateIssuesIntent, parse_metadata_block, replace_metadata_block};
use std::collections::BTreeMap;
use temper_forge::{
    Forge, ForgeError, IssueQuery, IssueState, ItemListDetails, ItemNumber, RepositoryId,
    UpdateIssue,
};

impl<F: Forge + ?Sized> Executor<'_, F> {
    /// Finds incomplete parent intents with bounded, state-explicit body-marker
    /// queries and resumes them without any worker-owned runtime context.
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
            // Summary list responses may truncate a large persisted intent.
            // Reload the selected parent by id before parsing authoritative data.
            let Some(issue) = self.forge.get_issue(&summary.id).await? else {
                continue;
            };
            let Some(metadata) = parse_metadata_block(&issue.body).map_err(metadata_error)? else {
                continue;
            };
            for (key, intent) in metadata.create_issue_intents {
                if intent.completed {
                    continue;
                }
                self.resume_create_intent(repo_id, issue.number, &key, intent)
                    .await?;
                recovered += 1;
            }
        }
        Ok(recovered)
    }

    pub(super) async fn persist_create_intent(
        &self,
        repo_id: &RepositoryId,
        parent_number: ItemNumber,
        key: &str,
        proposed: CreateIssuesIntent,
    ) -> Result<CreateIssuesIntent, ExecutionError> {
        for _ in 0..3 {
            let issue = self
                .forge
                .get_issue_by_number(repo_id, parent_number)
                .await?
                .ok_or(ExecutionError::TargetMissing {
                    target: ArtifactSource::Issue {
                        number: parent_number,
                    },
                })?;
            let mut metadata = parse_metadata_block(&issue.body)
                .map_err(metadata_error)?
                .unwrap_or_default();
            if let Some(existing) = metadata.create_issue_intents.get(key) {
                return Ok(existing.clone());
            }
            metadata
                .create_issue_intents
                .insert(key.to_string(), proposed.clone());
            let body = replace_metadata_block(&issue.body, &metadata).map_err(metadata_error)?;
            match self
                .forge
                .update_issue(
                    &issue.id,
                    UpdateIssue {
                        body: Some(body),
                        expected_version: Some(issue.version),
                        ..UpdateIssue::default()
                    },
                )
                .await
            {
                Ok(_) => return Ok(proposed),
                Err(ForgeError::Conflict(_)) => continue,
                Err(error) => return Err(error.into()),
            }
        }
        Err(ExecutionError::Backend {
            message: "could not persist create-issues intent after concurrent updates".into(),
        })
    }

    pub(super) async fn save_create_intent(
        &self,
        repo_id: &RepositoryId,
        parent_number: ItemNumber,
        key: &str,
        intent: &CreateIssuesIntent,
    ) -> Result<(), ExecutionError> {
        for _ in 0..3 {
            let issue = self
                .forge
                .get_issue_by_number(repo_id, parent_number)
                .await?
                .ok_or(ExecutionError::TargetMissing {
                    target: ArtifactSource::Issue {
                        number: parent_number,
                    },
                })?;
            let mut metadata = parse_metadata_block(&issue.body)
                .map_err(metadata_error)?
                .unwrap_or_default();
            if metadata
                .create_issue_intents
                .get(key)
                .is_some_and(|current| current.completed)
            {
                return Ok(());
            }
            metadata
                .create_issue_intents
                .insert(key.to_string(), intent.clone());
            let body = replace_metadata_block(&issue.body, &metadata).map_err(metadata_error)?;
            match self
                .forge
                .update_issue(
                    &issue.id,
                    UpdateIssue {
                        body: Some(body),
                        expected_version: Some(issue.version),
                        ..UpdateIssue::default()
                    },
                )
                .await
            {
                Ok(_) => return Ok(()),
                Err(ForgeError::Conflict(_)) => continue,
                Err(error) => return Err(error.into()),
            }
        }
        Err(ExecutionError::Backend {
            message: "could not update create-issues intent after concurrent updates".into(),
        })
    }
}
