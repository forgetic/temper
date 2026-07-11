//! Durable, idempotent multi-artifact issue fan-out for the [`Executor`].

mod intent;

use super::{ChildIssueCheckpoint, ExecutionError, Executor};
use crate::artifact::ArtifactRef;
use crate::classify::ArtifactSource;
use crate::context::CreateIssuesChild;
use crate::ids::TransitionId;
use crate::metadata::{
    CreateIssueIntentChild, CreateIssuesIntent, global_child_correlation_key, parse_metadata_block,
    replace_metadata_block,
};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use temper_forge::{
    CreateIssue, Forge, ForgeError, IssueState, ItemNumber, RepositoryId, UpdateIssue,
};

/// A concrete multi-artifact request prepared from a `CreateIssues` effect.
pub(super) struct PreparedCreateIssues {
    pub(super) transition: TransitionId,
    pub(super) effect_index: usize,
    pub(super) base_correlation_key: String,
    pub(super) children: Vec<CreateIssuesChild>,
    pub(super) record_parent_dependencies: bool,
}

#[derive(Clone, Copy)]
enum ParentDependencyStyle {
    LegacyRepoQualified,
    Natural,
}

impl<F: Forge + ?Sized> Executor<'_, F> {
    /// Persists each complete fan-out before creating its first child, then
    /// drives the durable intent to completion. Child issues are created with a
    /// metadata staging bit and no labels, so neither label races nor process
    /// death can make a partially-wired child dispatchable.
    pub(super) async fn apply_issue_creates(
        &self,
        repo_id: &RepositoryId,
        target: ArtifactSource,
        creates: &[PreparedCreateIssues],
    ) -> Result<(), ExecutionError> {
        if creates.is_empty() {
            return Ok(());
        }
        let ArtifactSource::Issue {
            number: parent_number,
        } = target
        else {
            return Err(ExecutionError::Backend {
                message: "create_issues durability requires an issue source artifact".into(),
            });
        };

        for create in creates {
            let key = create_intent_key(
                target,
                &create.transition,
                create.effect_index,
                &create.base_correlation_key,
            );
            let proposed = self.intent_from_create(repo_id, parent_number, create);
            let intent = self
                .persist_create_intent(repo_id, parent_number, &key, proposed)
                .await?;
            if !intent.completed {
                self.resume_create_intent(repo_id, parent_number, &key, intent)
                    .await?;
            }
        }
        Ok(())
    }

    fn intent_from_create(
        &self,
        repo_id: &RepositoryId,
        parent_number: ItemNumber,
        create: &PreparedCreateIssues,
    ) -> CreateIssuesIntent {
        let children = create
            .children
            .iter()
            .map(|child| {
                let repository_id = child.target_repo.clone().unwrap_or_else(|| repo_id.clone());
                let correlation_key = if &repository_id == repo_id {
                    child_correlation_key(&create.base_correlation_key, &child.slug)
                } else {
                    global_child_correlation_key(repo_id, parent_number, &child.slug)
                };
                CreateIssueIntentChild {
                    slug: child.slug.clone(),
                    title: child.title.clone(),
                    body_hex: hex_encode(child.body.as_bytes()),
                    final_labels: normalized_labels(&child.labels),
                    dependencies: child.dependencies.clone(),
                    repository_id,
                    correlation_key,
                    number: None,
                    wired: false,
                    activated: false,
                }
            })
            .collect();
        CreateIssuesIntent {
            transition: create.transition.as_str().to_string(),
            effect_index: create.effect_index,
            correlation_key: create.base_correlation_key.clone(),
            record_parent_dependencies: create.record_parent_dependencies,
            children,
            parent_wired: false,
            completed: false,
        }
    }

    async fn resume_create_intent(
        &self,
        repo_id: &RepositoryId,
        parent_number: ItemNumber,
        key: &str,
        mut intent: CreateIssuesIntent,
    ) -> Result<(), ExecutionError> {
        if intent.completed {
            return Ok(());
        }

        // Pass 1: every staged child exists and carries its parent reference.
        for index in 0..intent.children.len() {
            let child = intent.children[index].clone();
            let same_repo = child.repository_id == *repo_id;
            let parent = if same_repo {
                ArtifactRef::same_repo(parent_number)
            } else {
                ArtifactRef::in_repo(repo_id.clone(), parent_number)
            };
            let body = decode_intent_body(&child.body_hex)?;
            let outcome = self
                .ensure_staged_issue_with_parent(
                    &child.repository_id,
                    &child.correlation_key,
                    parent,
                    CreateIssue {
                        title: child.title,
                        body,
                        labels: Vec::new(),
                        assignees: Vec::new(),
                    },
                )
                .await
                .map_err(|error| {
                    if same_repo {
                        error
                    } else {
                        annotate_target_repo_error(&child.repository_id, error)
                    }
                })?;
            let was_created = matches!(&outcome, super::EnsureOutcome::Created(_));
            intent.children[index].number = Some(outcome.into_artifact().number);
            if was_created {
                self.child_issue_checkpoint(ChildIssueCheckpoint::Created)
                    .await;
            }
            self.save_create_intent(repo_id, parent_number, key, &intent)
                .await?;
        }

        let child_numbers = intent
            .children
            .iter()
            .map(|child| {
                Ok((
                    child.slug.clone(),
                    (
                        child.repository_id.clone(),
                        child.number.ok_or_else(|| ExecutionError::Backend {
                            message: format!(
                                "intent child `{}` has no persisted number",
                                child.slug
                            ),
                        })?,
                    ),
                ))
            })
            .collect::<Result<BTreeMap<_, _>, ExecutionError>>()?;

        // Pass 2: write every sibling dependency. Progress may lag a landed
        // write; the metadata operation itself is idempotent on replay.
        for index in 0..intent.children.len() {
            let child = intent.children[index].clone();
            let child_number = child.number.expect("numbers checked above");
            let child_issue = self
                .forge
                .get_issue_by_number(&child.repository_id, child_number)
                .await?
                .ok_or(ExecutionError::TargetMissing {
                    target: ArtifactSource::Issue {
                        number: child_number,
                    },
                })?;
            for dependency_slug in &child.dependencies {
                let (dependency_repo, dependency_number) = &child_numbers[dependency_slug];
                let dependency = if dependency_repo == &child.repository_id {
                    ArtifactRef::same_repo(*dependency_number)
                } else {
                    ArtifactRef::in_repo(dependency_repo.clone(), *dependency_number)
                };
                self.ensure_issue_dependency_metadata(&child_issue.id, &dependency)
                    .await?;
            }
            self.child_issue_checkpoint(ChildIssueCheckpoint::Wired)
                .await;
            intent.children[index].wired = true;
            self.save_create_intent(repo_id, parent_number, key, &intent)
                .await?;
        }

        let any_cross_repo = intent
            .children
            .iter()
            .any(|child| child.repository_id != *repo_id);
        if any_cross_repo || intent.record_parent_dependencies {
            let style = if intent.record_parent_dependencies {
                ParentDependencyStyle::Natural
            } else {
                ParentDependencyStyle::LegacyRepoQualified
            };
            self.link_intent_parent_dependencies(repo_id, parent_number, &intent, style)
                .await?;
        }
        intent.parent_wired = true;
        self.save_create_intent(repo_id, parent_number, key, &intent)
            .await?;

        // Pass 3: only after all children and relations are safe, atomically
        // project each child's lifecycle labels and clear its staging marker.
        for index in 0..intent.children.len() {
            let child = intent.children[index].clone();
            let unresolved = self
                .intent_dependencies_unresolved(&child, &child_numbers)
                .await?;
            let labels = self.activation_labels(&child, unresolved);
            self.activate_staged_issue(
                &child.repository_id,
                child.number.expect("numbers checked above"),
                &child.correlation_key,
                &labels,
            )
            .await?;
            self.child_issue_checkpoint(ChildIssueCheckpoint::Activated)
                .await;
            intent.children[index].activated = true;
            self.save_create_intent(repo_id, parent_number, key, &intent)
                .await?;
        }

        intent.completed = intent.parent_wired
            && intent
                .children
                .iter()
                .all(|child| child.number.is_some() && child.wired && child.activated);
        self.save_create_intent(repo_id, parent_number, key, &intent)
            .await?;
        Ok(())
    }

    async fn ensure_staged_issue_with_parent(
        &self,
        repo_id: &RepositoryId,
        correlation_key: &str,
        parent: ArtifactRef,
        input: CreateIssue,
    ) -> Result<super::EnsureOutcome<temper_forge::Issue>, ExecutionError> {
        if let Some(existing) = self
            .find_issue_by_correlation(repo_id, correlation_key, &[])
            .await?
        {
            let existing = self.ensure_issue_parent(existing, Some(parent)).await?;
            return Ok(super::EnsureOutcome::Existing(existing));
        }
        let mut metadata = parse_metadata_block(&input.body)
            .map_err(metadata_error)?
            .unwrap_or_default();
        metadata.correlation_key = Some(correlation_key.to_string());
        if !metadata.parents.contains(&parent) {
            metadata.parents.push(parent);
        }
        metadata.staged = true;
        let body = replace_metadata_block(&input.body, &metadata).map_err(metadata_error)?;
        let created = self
            .forge
            .create_issue(repo_id, CreateIssue { body, ..input })
            .await?;
        Ok(super::EnsureOutcome::Created(created))
    }

    async fn activate_staged_issue(
        &self,
        repo_id: &RepositoryId,
        number: ItemNumber,
        correlation_key: &str,
        labels: &[String],
    ) -> Result<(), ExecutionError> {
        for _ in 0..3 {
            let issue = self
                .forge
                .get_issue_by_number(repo_id, number)
                .await?
                .ok_or(ExecutionError::TargetMissing {
                    target: ArtifactSource::Issue { number },
                })?;
            let mut metadata = parse_metadata_block(&issue.body)
                .map_err(metadata_error)?
                .unwrap_or_default();
            if metadata.correlation_key.as_deref() != Some(correlation_key) {
                return Err(ExecutionError::Backend {
                    message: format!("intent child #{number} has an unexpected correlation key"),
                });
            }
            if !metadata.staged {
                // Activation already committed. Do not regress legitimate
                // lifecycle changes that may have happened after the child
                // became dispatchable but before parent progress was saved.
                return Ok(());
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
            metadata.staged = false;
            let body = replace_metadata_block(&issue.body, &metadata).map_err(metadata_error)?;
            match self
                .forge
                .update_issue(
                    &issue.id,
                    UpdateIssue {
                        body: Some(body),
                        add_labels,
                        remove_labels,
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
            message: format!("could not activate staged issue #{number} after concurrent updates"),
        })
    }

    async fn intent_dependencies_unresolved(
        &self,
        child: &CreateIssueIntentChild,
        child_numbers: &BTreeMap<String, (RepositoryId, ItemNumber)>,
    ) -> Result<bool, ExecutionError> {
        for dependency_slug in &child.dependencies {
            let (repository_id, number) = &child_numbers[dependency_slug];
            let issue = self
                .forge
                .get_issue_by_number(repository_id, *number)
                .await?
                .ok_or(ExecutionError::TargetMissing {
                    target: ArtifactSource::Issue { number: *number },
                })?;
            if issue.state != IssueState::Closed {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn activation_labels(
        &self,
        child: &CreateIssueIntentChild,
        dependencies_unresolved: bool,
    ) -> Vec<String> {
        let mut labels = child.final_labels.clone();
        if child.dependencies.is_empty() {
            return labels;
        }
        let kind = decode_intent_body(&child.body_hex)
            .ok()
            .and_then(|body| parse_metadata_block(&body).ok().flatten())
            .and_then(|metadata| metadata.kind);
        for dimension in self
            .workflow
            .state_dimensions()
            .iter()
            .filter(|d| d.exclusive)
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
                let ready_label = ready.label.as_ref().map(|label| label.as_str());
                let blocked_label = blocked.label.as_ref().map(|label| label.as_str());
                if let Some(label) = ready_label {
                    labels.retain(|candidate| candidate != label);
                }
                if let Some(label) = blocked_label {
                    labels.retain(|candidate| candidate != label);
                }
                let desired = if dependencies_unresolved {
                    blocked_label
                } else {
                    ready_label
                };
                if let Some(label) = desired {
                    labels.push(label.to_string());
                }
            }
        }
        normalized_labels(&labels)
    }

    async fn link_intent_parent_dependencies(
        &self,
        repo_id: &RepositoryId,
        parent_number: ItemNumber,
        intent: &CreateIssuesIntent,
        style: ParentDependencyStyle,
    ) -> Result<(), ExecutionError> {
        let parent_issue = self
            .forge
            .get_issue_by_number(repo_id, parent_number)
            .await?
            .ok_or(ExecutionError::TargetMissing {
                target: ArtifactSource::Issue {
                    number: parent_number,
                },
            })?;
        for child in &intent.children {
            let number = child.number.ok_or_else(|| ExecutionError::Backend {
                message: format!("intent child `{}` has no number", child.slug),
            })?;
            let dependency = match style {
                ParentDependencyStyle::Natural if child.repository_id == *repo_id => {
                    ArtifactRef::same_repo(number)
                }
                ParentDependencyStyle::Natural | ParentDependencyStyle::LegacyRepoQualified => {
                    ArtifactRef::in_repo(child.repository_id.clone(), number)
                }
            };
            self.ensure_issue_dependency_metadata(&parent_issue.id, &dependency)
                .await?;
        }
        Ok(())
    }
}

fn create_intent_key(
    target: ArtifactSource,
    transition: &TransitionId,
    effect_index: usize,
    correlation_key: &str,
) -> String {
    let source = match target {
        ArtifactSource::Issue { number } => format!("issue:{}", number.get()),
        ArtifactSource::PullRequest { number } => format!("pull_request:{}", number.get()),
    };
    format!(
        "source:{}:{}/transition:{}:{}/effect:{effect_index}/correlation:{}:{}",
        source.len(),
        source,
        transition.as_str().len(),
        transition.as_str(),
        correlation_key.len(),
        correlation_key
    )
}

fn child_correlation_key(base_correlation_key: &str, slug: &str) -> String {
    format!(
        "{}:{}/child:{}:{}",
        base_correlation_key.len(),
        base_correlation_key,
        slug.len(),
        slug
    )
}

fn normalized_labels(labels: &[String]) -> Vec<String> {
    labels
        .iter()
        .filter_map(|label| {
            let label = label.trim();
            (!label.is_empty()).then(|| label.to_string())
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

fn decode_intent_body(encoded: &str) -> Result<String, ExecutionError> {
    if encoded.len() % 2 != 0 {
        return Err(ExecutionError::Backend {
            message: "intent child body has invalid hex encoding".into(),
        });
    }
    let bytes = encoded.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.chunks_exact(2) {
        let high = hex_digit(pair[0]).ok_or_else(|| ExecutionError::Backend {
            message: "intent child body has invalid hex encoding".into(),
        })?;
        let low = hex_digit(pair[1]).ok_or_else(|| ExecutionError::Backend {
            message: "intent child body has invalid hex encoding".into(),
        })?;
        decoded.push((high << 4) | low);
    }
    String::from_utf8(decoded).map_err(|error| ExecutionError::Backend {
        message: format!("intent child body is not UTF-8: {error}"),
    })
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn annotate_target_repo_error(target_repo: &RepositoryId, error: ExecutionError) -> ExecutionError {
    match error {
        ExecutionError::Backend { message } => ExecutionError::Backend {
            message: format!("cannot ensure issue in target repository `{target_repo}`: {message}"),
        },
        other => other,
    }
}

fn metadata_error(error: impl std::fmt::Display) -> ExecutionError {
    ExecutionError::Backend {
        message: format!("invalid workflow metadata: {error}"),
    }
}

/// Validates that every declared sibling dependency resolves in the same effect.
pub(super) fn validate_child_dependencies(
    transition: &TransitionId,
    effect_index: usize,
    children: &[CreateIssuesChild],
) -> Result<(), ExecutionError> {
    let slugs: HashSet<&str> = children.iter().map(|child| child.slug.as_str()).collect();
    for child in children {
        for dependency in &child.dependencies {
            if !slugs.contains(dependency.as_str()) {
                return Err(ExecutionError::UnknownCreateIssuesDependency {
                    transition: transition.clone(),
                    effect_index,
                    slug: child.slug.clone(),
                    dependency: dependency.clone(),
                });
            }
        }
    }
    Ok(())
}
