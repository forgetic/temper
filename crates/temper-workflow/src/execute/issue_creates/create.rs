//! Create pass for durable child fan-out.

use super::{IntentExecutionMode, annotate_target_repo_error, decode_intent_body, metadata_error};
use crate::artifact::ArtifactRef;
use crate::classify::ArtifactSource;
use crate::metadata::{
    CreateIssueIntentChild, CreateIssuesIntent, parse_metadata_block, replace_metadata_block,
};
use std::collections::{BTreeMap, BTreeSet};
use temper_forge::{
    CreateIssue, Forge, Issue, IssueQuery, IssueState, ItemListDetails, ItemNumber, RepositoryId,
};

use super::super::{ChildIssueCheckpoint, ExecutionError, Executor};

impl<F: Forge + ?Sized> Executor<'_, F> {
    /// Ensures every child exists, then checkpoints all returned numbers in one
    /// parent update. Any uncertain create aborts before that checkpoint; the
    /// next invocation is recovery and discovers the landed correlation.
    pub(super) async fn create_pass(
        &self,
        repo_id: &RepositoryId,
        parent_number: ItemNumber,
        key: &str,
        mut intent: CreateIssuesIntent,
        mode: IntentExecutionMode,
        mut parent: Issue,
    ) -> Result<(CreateIssuesIntent, Issue, Vec<Issue>), ExecutionError> {
        let recovered = if mode == IntentExecutionMode::Recovery {
            self.recover_unresolved_children(&intent).await?
        } else {
            BTreeMap::new()
        };
        let mut recovered = recovered;
        let mut children = Vec::with_capacity(intent.children.len());
        let mut numbers_changed = false;

        for index in 0..intent.children.len() {
            let child = intent.children[index].clone();
            let issue = if let Some(number) = child.number {
                self.forge
                    .get_issue_by_number(&child.repository_id, number)
                    .await?
                    .ok_or(ExecutionError::TargetMissing {
                        target: ArtifactSource::Issue { number },
                    })?
            } else if let Some(existing) = recovered.remove(&child.correlation_key) {
                existing
            } else {
                let input = self.staged_child_input(repo_id, parent_number, &child)?;
                let same_repo = child.repository_id == *repo_id;
                let created = self
                    .forge
                    .create_issue(&child.repository_id, input)
                    .await
                    .map_err(ExecutionError::from)
                    .map_err(|error| {
                        if same_repo {
                            error
                        } else {
                            annotate_target_repo_error(&child.repository_id, error)
                        }
                    })?;
                // The hook runs after the create is known committed but before
                // any child number can be durable on the parent.
                self.child_issue_checkpoint(ChildIssueCheckpoint::Created)
                    .await;
                created
            };

            if intent.children[index].number != Some(issue.number) {
                intent.children[index].number = Some(issue.number);
                numbers_changed = true;
            }
            children.push(issue);
        }

        if numbers_changed {
            parent = self.save_create_intent(parent, key, &intent).await?;
        }
        Ok((intent, parent, children))
    }

    /// Recovery scans unresolved correlations in a repository-grouped pass:
    /// exactly one open and one closed summary query for each affected repo.
    async fn recover_unresolved_children(
        &self,
        intent: &CreateIssuesIntent,
    ) -> Result<BTreeMap<String, Issue>, ExecutionError> {
        let mut by_repo = BTreeMap::<RepositoryId, BTreeSet<String>>::new();
        for child in intent
            .children
            .iter()
            .filter(|child| child.number.is_none())
        {
            by_repo
                .entry(child.repository_id.clone())
                .or_default()
                .insert(child.correlation_key.clone());
        }

        let mut found = BTreeMap::new();
        for (repository_id, wanted) in by_repo {
            for state in [IssueState::Open, IssueState::Closed] {
                let issues = self
                    .forge
                    .list_issues(
                        &repository_id,
                        IssueQuery {
                            limit: None,
                            state: Some(state),
                            labels: Vec::new(),
                            body_contains: Some("\"correlation_key\"".into()),
                            author_id: None,
                            assignee_id: None,
                            sort: None,
                            details: ItemListDetails::summary(),
                        },
                    )
                    .await
                    .map_err(ExecutionError::from)
                    .map_err(|error| annotate_target_repo_error(&repository_id, error))?;
                for issue in issues {
                    let correlation = parse_metadata_block(&issue.body)
                        .map_err(metadata_error)?
                        .and_then(|metadata| metadata.correlation_key);
                    if let Some(correlation) = correlation.filter(|key| wanted.contains(key)) {
                        found.entry(correlation).or_insert(issue);
                    }
                }
            }
        }
        Ok(found)
    }

    fn staged_child_input(
        &self,
        parent_repo: &RepositoryId,
        parent_number: ItemNumber,
        child: &CreateIssueIntentChild,
    ) -> Result<CreateIssue, ExecutionError> {
        let mut body = decode_intent_body(&child.body_hex)?;
        let mut metadata = parse_metadata_block(&body)
            .map_err(metadata_error)?
            .unwrap_or_default();
        metadata.correlation_key = Some(child.correlation_key.clone());
        let parent = if child.repository_id == *parent_repo {
            ArtifactRef::same_repo(parent_number)
        } else {
            ArtifactRef::in_repo(parent_repo.clone(), parent_number)
        };
        if !metadata.parents.contains(&parent) {
            metadata.parents.push(parent);
            metadata.parents.sort();
            metadata.parents.dedup();
        }
        metadata.staged = true;
        body = replace_metadata_block(&body, &metadata).map_err(metadata_error)?;
        Ok(CreateIssue {
            title: child.title.clone(),
            body,
            // Final queue/kind labels are present from the atomic create. The
            // staging bit, not missing labels, is the dispatch barrier.
            labels: self.staged_labels(child),
            assignees: Vec::new(),
        })
    }
}
