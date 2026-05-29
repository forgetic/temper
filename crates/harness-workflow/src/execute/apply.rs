//! Effect application and postcondition verification for the [`Executor`].
//!
//! This child module holds the mutation half of the runtime loop: turning a
//! plan's [`WorkflowEffect`]s into Forge calls and verifying the resulting
//! [`Postcondition`]s. It is split from the parent `execute` module to keep both
//! files within the source-size budget; it accesses the parent's private
//! [`Executor`] and [`Loaded`] items as a descendant module.
//!
//! The application discipline is "validate everything, then mutate": every
//! effect is checked (unsupported effects and unbound assignee roles fail here)
//! before any backend call, idempotent comments are posted, the merge (if any)
//! is applied, and finally labels and assignees are folded into one update — the
//! commit point. See the parent module docs for why comments and the merge
//! precede the label flip, and why the merge is at most once.

use super::{ExecutionError, Executor, Loaded};
use crate::ids::{RoleId, TransitionId};
use crate::plan::{Postcondition, TransitionPlan, WorkflowEffect};
use harness_forge::{
    CreateComment, Forge, MergeMethod, MergePullRequest, UpdateIssue, UpdatePullRequest, UserId,
};

impl<F: Forge + ?Sized> Executor<'_, F> {
    /// Applies a plan's effects, refusing partial application of the state flip.
    ///
    /// First it validates *every* effect (rejecting unsupported effects and
    /// assignee roles with no bound user) before any mutation. Then it posts
    /// idempotent comments, and finally folds labels and assignees into a single
    /// backend update — the commit point that flips the artifact's state.
    pub(super) async fn apply(
        &self,
        loaded: &Loaded,
        plan: &TransitionPlan,
    ) -> Result<(), ExecutionError> {
        let prepared = self.prepare_effects(&plan.effects)?;
        self.apply_comments(loaded, &plan.transition, &prepared.comments)
            .await?;
        self.apply_merge(loaded, prepared.merge).await?;
        self.apply_update(loaded, prepared).await
    }

    /// Reloads fresh state and checks every postcondition holds.
    pub(super) async fn verify(
        &self,
        repo_id: &harness_forge::RepositoryId,
        target: crate::classify::ArtifactSource,
        postconditions: &[Postcondition],
    ) -> Result<(), ExecutionError> {
        if postconditions.is_empty() {
            return Ok(());
        }
        let (labels, assignees) = self.current_state(repo_id, target).await?;
        for postcondition in postconditions {
            let satisfied = match postcondition {
                Postcondition::LabelPresent(label) => labels.iter().any(|l| l == label.as_str()),
                Postcondition::LabelAbsent(label) => labels.iter().all(|l| l != label.as_str()),
                Postcondition::AssigneePresent { role } => {
                    let user = self.resolve_assignee(role)?;
                    assignees.contains(&user)
                }
                Postcondition::AssigneeAbsent { role } => {
                    let user = self.resolve_assignee(role)?;
                    !assignees.contains(&user)
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

    /// Partitions plan effects into a backend update, resolving assignee roles.
    ///
    /// Runs before any mutation so an unsupported effect or an unbound assignee
    /// role fails the whole transition cleanly, never half-applied.
    fn prepare_effects(
        &self,
        effects: &[WorkflowEffect],
    ) -> Result<PreparedEffects, ExecutionError> {
        let mut prepared = PreparedEffects::default();
        for effect in effects {
            match effect {
                WorkflowEffect::AddLabel(label) => {
                    prepared.add_labels.push(label.as_str().to_string());
                }
                WorkflowEffect::RemoveLabel(label) => {
                    prepared.remove_labels.push(label.as_str().to_string());
                }
                WorkflowEffect::SetAssignee { role } => {
                    prepared.add_assignees.push(self.resolve_assignee(role)?);
                }
                WorkflowEffect::RemoveAssignee { role } => {
                    prepared.remove_assignees.push(self.resolve_assignee(role)?);
                }
                WorkflowEffect::CreateComment { body } => {
                    prepared.comments.push(body.clone());
                }
                WorkflowEffect::MergePullRequest => {
                    prepared.merge = true;
                }
                other => {
                    return Err(ExecutionError::UnsupportedEffect {
                        effect: other.clone(),
                    })
                }
            }
        }
        Ok(prepared)
    }

    /// Resolves an assignee role to a Forge user through the execution context.
    fn resolve_assignee(&self, role: &RoleId) -> Result<UserId, ExecutionError> {
        self.context
            .resolve_assignee(role)
            .cloned()
            .ok_or_else(|| ExecutionError::UnresolvedAssignee { role: role.clone() })
    }

    /// Merges the target pull request at most once.
    ///
    /// Runs before the label commit point so that the post-merge labels (which
    /// the transition declares as ordinary `add_label` effects) double as the
    /// "already done" marker: once they land, a retry's planner sees them
    /// present and refuses to re-run. A pull request that is already merged
    /// (observed in the freshly loaded state) is skipped, so a crash that lands
    /// the merge but loses the response never merges twice on retry. A merge
    /// effect targeting an issue is impossible under a validated workflow (the
    /// transition's artifact kind maps to a pull-request target); it is rejected
    /// defensively as unsupported rather than silently ignored.
    async fn apply_merge(&self, loaded: &Loaded, merge: bool) -> Result<(), ExecutionError> {
        if !merge {
            return Ok(());
        }
        let Loaded::PullRequest { id, merged, .. } = loaded else {
            return Err(ExecutionError::UnsupportedEffect {
                effect: WorkflowEffect::MergePullRequest,
            });
        };
        if *merged {
            return Ok(());
        }
        let input = MergePullRequest {
            method: MergeMethod::MergeCommit,
            commit_title: None,
            commit_body: None,
        };
        self.forge.merge_pull_request(id, input).await?;
        Ok(())
    }

    /// Posts each planned comment at most once, guarded by a deterministic
    /// marker so a retry never duplicates a comment.
    async fn apply_comments(
        &self,
        loaded: &Loaded,
        transition: &TransitionId,
        comments: &[String],
    ) -> Result<(), ExecutionError> {
        for (index, body) in comments.iter().enumerate() {
            let key = comment_key(transition, index);
            if self.comment_exists(loaded, &key).await? {
                continue;
            }
            let input = CreateComment {
                body: comment_body_with_marker(body, &key),
            };
            match loaded {
                Loaded::Issue { id, .. } => {
                    self.forge.add_issue_comment(id, input).await?;
                }
                Loaded::PullRequest { id, .. } => {
                    self.forge.add_pull_request_comment(id, input).await?;
                }
            }
        }
        Ok(())
    }

    /// Returns `true` when a comment carrying `key`'s marker already exists.
    async fn comment_exists(&self, loaded: &Loaded, key: &str) -> Result<bool, ExecutionError> {
        let marker = comment_marker(key);
        let comments = match loaded {
            Loaded::Issue { id, .. } => self.forge.list_issue_comments(id).await?,
            Loaded::PullRequest { id, .. } => self.forge.list_pull_request_comments(id).await?,
        };
        Ok(comments
            .iter()
            .any(|comment| comment.body.contains(&marker)))
    }

    /// Folds the prepared labels and assignees into a single backend update.
    ///
    /// Skips the call entirely when there is nothing to change (for example a
    /// comment-only transition), so it never issues an empty mutation.
    async fn apply_update(
        &self,
        loaded: &Loaded,
        prepared: PreparedEffects,
    ) -> Result<(), ExecutionError> {
        if !prepared.has_update() {
            return Ok(());
        }
        match loaded {
            Loaded::Issue { id, .. } => {
                let update = UpdateIssue {
                    add_labels: prepared.add_labels,
                    remove_labels: prepared.remove_labels,
                    add_assignees: prepared.add_assignees,
                    remove_assignees: prepared.remove_assignees,
                    ..UpdateIssue::default()
                };
                self.forge.update_issue(id, update).await?;
            }
            Loaded::PullRequest { id, .. } => {
                let update = UpdatePullRequest {
                    add_labels: prepared.add_labels,
                    remove_labels: prepared.remove_labels,
                    add_assignees: prepared.add_assignees,
                    remove_assignees: prepared.remove_assignees,
                    ..UpdatePullRequest::default()
                };
                self.forge.update_pull_request(id, update).await?;
            }
        }
        Ok(())
    }

    /// Reads the artifact's current labels and assignees from fresh Forge state.
    async fn current_state(
        &self,
        repo_id: &harness_forge::RepositoryId,
        target: crate::classify::ArtifactSource,
    ) -> Result<(Vec<String>, Vec<UserId>), ExecutionError> {
        use crate::classify::ArtifactSource;
        match target {
            ArtifactSource::Issue { number } => {
                let issue = self
                    .forge
                    .get_issue_by_number(repo_id, number)
                    .await?
                    .ok_or(ExecutionError::TargetMissing { target })?;
                Ok((issue.labels, issue.assignees))
            }
            ArtifactSource::PullRequest { number } => {
                let pull_request = self
                    .forge
                    .get_pull_request_by_number(repo_id, number)
                    .await?
                    .ok_or(ExecutionError::TargetMissing { target })?;
                Ok((pull_request.labels, pull_request.assignees))
            }
        }
    }
}

/// Effects partitioned into a single backend update plus comment bodies.
///
/// Labels and assignees are applied together in one `UpdateIssue`/
/// `UpdatePullRequest` so the state flip is atomic; comments are posted
/// separately (and idempotently) before that update.
#[derive(Default)]
struct PreparedEffects {
    add_labels: Vec<String>,
    remove_labels: Vec<String>,
    add_assignees: Vec<UserId>,
    remove_assignees: Vec<UserId>,
    comments: Vec<String>,
    /// Whether the plan requests merging the target pull request.
    merge: bool,
}

impl PreparedEffects {
    /// Returns `true` when the single label/assignee update would change
    /// anything, so a comment-only transition never issues an empty mutation.
    fn has_update(&self) -> bool {
        !(self.add_labels.is_empty()
            && self.remove_labels.is_empty()
            && self.add_assignees.is_empty()
            && self.remove_assignees.is_empty())
    }
}

/// Opening text of the HTML comment marker that makes a comment idempotent.
const COMMENT_MARKER_PREFIX: &str = "<!-- harness:comment-key=";
/// Closing text of the comment marker.
const COMMENT_MARKER_SUFFIX: &str = " -->";

/// Builds the idempotency key for the `index`-th comment of a transition.
///
/// The key is deterministic across retries and distinct per comment, so
/// re-executing the same transition against the same artifact never posts a
/// duplicate comment. It deliberately does not include the worker identity:
/// after a crash a different worker may retry, and the comment must still
/// dedupe.
fn comment_key(transition: &TransitionId, index: usize) -> String {
    format!("{transition}:{index}")
}

/// Renders the hidden marker that identifies a previously posted comment.
fn comment_marker(key: &str) -> String {
    format!("{COMMENT_MARKER_PREFIX}{key}{COMMENT_MARKER_SUFFIX}")
}

/// Appends the idempotency marker to a comment body.
///
/// The marker is an HTML comment, so it renders invisibly in Forge markdown
/// while remaining searchable by [`comment_marker`].
fn comment_body_with_marker(body: &str, key: &str) -> String {
    format!("{body}\n\n{}", comment_marker(key))
}
