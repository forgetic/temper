//! Idempotent multi-artifact issue fan-out for the [`Executor`].
//!
//! This child module holds the `CreateIssues` effect's apply path: creating each
//! workspace-authored child issue idempotently, linking declared sibling
//! dependencies, and recording parent dependency refs for cross-repository
//! fan-outs. It is split from the sibling `apply` module to keep both files
//! within the source-size budget; it accesses the parent's private [`Executor`]
//! items as a descendant module.

use super::{ExecutionError, Executor};
use crate::artifact::ArtifactRef;
use crate::classify::ArtifactSource;
use crate::context::CreateIssuesChild;
use crate::ids::TransitionId;
use crate::metadata::global_child_correlation_key;
use std::collections::{BTreeMap, HashSet};
use temper_forge_model::{CreateIssue, Forge, ItemNumber, RepositoryId};

/// A concrete, idempotent multi-artifact issue-create request prepared from a
/// `CreateIssues` effect plus the runtime [`crate::context::ExecutionContext`].
///
/// The children are the workspace work product; the base correlation key seeds
/// each child's per-child key so the whole fan-out is at-most-once across
/// retries. Sibling dependency slugs have already been validated to reference a
/// child in the same effect.
pub(super) struct PreparedCreateIssues {
    pub(super) base_correlation_key: String,
    pub(super) children: Vec<CreateIssuesChild>,
}

impl<F: Forge + ?Sized> Executor<'_, F> {
    /// Creates the workspace-authored child issues idempotently before the
    /// label commit point, then links the declared sibling dependencies.
    ///
    /// Each child lands independently through
    /// [`ensure_issue_with_parent`](Self::ensure_issue_with_parent), carrying a
    /// parent back-reference to the artifact the transition acts on and a stable
    /// per-child correlation key. Same-repository children keep the legacy
    /// base-key/slug correlation-key composition and same-repository parent
    /// shorthand for replay compatibility. A child with
    /// [`CreateIssuesChild::target_repo`] set uses the documented global child
    /// correlation key, a repo-qualified parent back-reference, and is created in
    /// that target repository. The fan-out is non-atomic on real forges (the
    /// cross-repo aggregation stance): if a create lands but a later child or the
    /// source label flip crashes, retrying reuses the same keys and resolves the
    /// existing children instead of duplicating them. Sibling dependency
    /// relations are recorded only after every child exists, so a dependency
    /// target's repository and number are always known. When an issue transition
    /// fans out to at least one cross-repository child, the parent issue also
    /// records repo-qualified fallback dependency refs for every child in
    /// declaration order; all-same-repository fan-outs leave the parent metadata
    /// untouched for byte-for-byte compatibility with existing artifacts.
    pub(super) async fn apply_issue_creates(
        &self,
        repo_id: &RepositoryId,
        target: ArtifactSource,
        creates: &[PreparedCreateIssues],
    ) -> Result<(), ExecutionError> {
        let parent_number = target_number(target);
        for create in creates {
            let mut child_numbers_by_slug = BTreeMap::<String, (RepositoryId, ItemNumber)>::new();
            let mut any_cross_repo = false;
            // First pass: every child exists and carries its parent back-reference.
            for child in &create.children {
                let child_repo = child.target_repo.as_ref().unwrap_or(repo_id);
                let same_repo = child_repo == repo_id;
                any_cross_repo |= !same_repo;
                let correlation_key = if same_repo {
                    child_correlation_key(&create.base_correlation_key, &child.slug)
                } else {
                    global_child_correlation_key(repo_id, parent_number, &child.slug)
                };
                let parent = if same_repo {
                    ArtifactRef::same_repo(parent_number)
                } else {
                    ArtifactRef::in_repo(repo_id.clone(), parent_number)
                };
                let outcome = {
                    let result = self
                        .ensure_issue_with_parent(
                            child_repo,
                            &correlation_key,
                            Some(parent),
                            CreateIssue {
                                title: child.title.clone(),
                                body: child.body.clone(),
                                labels: child.labels.clone(),
                                assignees: Vec::new(),
                            },
                        )
                        .await;
                    if same_repo {
                        result?
                    } else {
                        result.map_err(|error| annotate_target_repo_error(child_repo, error))?
                    }
                };
                child_numbers_by_slug.insert(
                    child.slug.clone(),
                    (child_repo.clone(), outcome.into_artifact().number),
                );
            }
            self.link_child_dependencies(&create.children, &child_numbers_by_slug)
                .await?;
            if any_cross_repo {
                self.link_parent_dependencies(
                    repo_id,
                    target,
                    &create.children,
                    &child_numbers_by_slug,
                )
                .await?;
            }
        }
        Ok(())
    }

    /// Second pass: link sibling dependencies now that all numbers resolve.
    async fn link_child_dependencies(
        &self,
        children: &[CreateIssuesChild],
        child_numbers_by_slug: &BTreeMap<String, (RepositoryId, ItemNumber)>,
    ) -> Result<(), ExecutionError> {
        for child in children {
            if child.dependencies.is_empty() {
                continue;
            }
            let (child_repo, child_number) = child_numbers_by_slug[&child.slug].clone();
            let child_issue = self
                .forge
                .get_issue_by_number(&child_repo, child_number)
                .await?
                .ok_or(ExecutionError::TargetMissing {
                    target: ArtifactSource::Issue {
                        number: child_number,
                    },
                })?;
            for dependency_slug in &child.dependencies {
                let (dependency_repo, dependency_number) =
                    child_numbers_by_slug[dependency_slug].clone();
                let dependency = if dependency_repo == child_repo {
                    ArtifactRef::same_repo(dependency_number)
                } else {
                    ArtifactRef::in_repo(dependency_repo, dependency_number)
                };
                self.ensure_issue_dependency_metadata(&child_issue.id, &dependency)
                    .await?;
            }
        }
        Ok(())
    }

    /// Third pass: cross-repo issue fan-out records parent dependency refs.
    async fn link_parent_dependencies(
        &self,
        repo_id: &RepositoryId,
        target: ArtifactSource,
        children: &[CreateIssuesChild],
        child_numbers_by_slug: &BTreeMap<String, (RepositoryId, ItemNumber)>,
    ) -> Result<(), ExecutionError> {
        let ArtifactSource::Issue { number } = target else {
            return Ok(());
        };
        let parent_issue = self
            .forge
            .get_issue_by_number(repo_id, number)
            .await?
            .ok_or(ExecutionError::TargetMissing { target })?;
        for child in children {
            let (child_repo, child_number) = child_numbers_by_slug[&child.slug].clone();
            self.ensure_issue_dependency_metadata(
                &parent_issue.id,
                &ArtifactRef::in_repo(child_repo, child_number),
            )
            .await?;
        }
        Ok(())
    }
}

/// Returns the Forge item number of a transition target.
fn target_number(target: ArtifactSource) -> ItemNumber {
    match target {
        ArtifactSource::Issue { number } | ArtifactSource::PullRequest { number } => number,
    }
}

/// Builds a stable per-child correlation key from the effect's base key and a
/// child slug.
///
/// Length prefixes keep the composition collision-free even when the base key
/// or slug contain separators, so re-running the same effect recomputes the
/// same per-child key and resolves the existing child instead of duplicating it.
fn child_correlation_key(base_correlation_key: &str, slug: &str) -> String {
    format!(
        "{}:{}/child:{}:{}",
        base_correlation_key.len(),
        base_correlation_key,
        slug.len(),
        slug
    )
}

fn annotate_target_repo_error(target_repo: &RepositoryId, error: ExecutionError) -> ExecutionError {
    match error {
        ExecutionError::Backend { message } => ExecutionError::Backend {
            message: format!("cannot ensure issue in target repository `{target_repo}`: {message}"),
        },
        other => other,
    }
}

/// Validates that every child's declared sibling dependency names another child
/// bound in the same effect, before any mutation.
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
