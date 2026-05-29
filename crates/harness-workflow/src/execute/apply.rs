//! Effect application and postcondition verification for the [`Executor`].
//!
//! This child module holds the mutation half of the runtime loop: turning a
//! plan's [`WorkflowEffect`]s into Forge calls and verifying the resulting
//! [`Postcondition`]s. It is split from the parent `execute` module to keep both
//! files within the source-size budget; it accesses the parent's private
//! [`Executor`] and [`Loaded`] items as a descendant module.
//!
//! The application discipline is "validate everything, then mutate": unsupported
//! effects, missing create inputs, and unbound assignee roles fail before any
//! backend call. Idempotent comments and pull-request creates run before the
//! merge (if any) and final label/assignee update — the commit point. See the
//! parent module docs for why pre-commit effects are ordered this way.

use super::{ExecutionError, Executor, Loaded};
use crate::classify::ArtifactSource;
use crate::ids::{RoleId, TransitionId};
use crate::plan::{Postcondition, TransitionPlan, WorkflowEffect};
use harness_forge::{
    CreateComment, CreatePullRequest, Forge, MergeMethod, MergePullRequest, RepositoryId,
    UpdateIssue, UpdatePullRequest, UserId,
};
use std::collections::HashSet;

impl<F: Forge + ?Sized> Executor<'_, F> {
    /// Applies a plan's effects, refusing partial application of the state flip.
    ///
    /// First it validates *every* effect (rejecting unsupported effects,
    /// missing create inputs, and assignee roles with no bound user) before any
    /// mutation. Then it posts idempotent comments, ensures requested pull
    /// requests, and finally folds labels and assignees into a single backend
    /// update — the commit point that flips the artifact's state.
    pub(super) async fn apply(
        &self,
        repo_id: &RepositoryId,
        loaded: &Loaded,
        plan: &TransitionPlan,
    ) -> Result<(), ExecutionError> {
        let prepared = self.prepare_effects(plan)?;
        self.apply_comments(loaded, &plan.transition, &prepared.comments)
            .await?;
        self.apply_pull_request_creates(repo_id, &prepared.pull_request_creates)
            .await?;
        self.apply_merge(loaded, prepared.merge).await?;
        self.apply_update(loaded, prepared).await
    }

    /// Reloads fresh state and checks every postcondition holds.
    pub(super) async fn verify(
        &self,
        repo_id: &harness_forge::RepositoryId,
        target: ArtifactSource,
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

    /// Re-applies label effects against fresh state, idempotently.
    ///
    /// This is the reconciler applier's reuse of the executor's label-apply
    /// path: it realizes a partial transition's still-pending labels or a
    /// mechanical unblock's labels without re-running a full transition plan,
    /// rather than hand-rolling a second mutation path. It loads fresh state,
    /// keeps only the not-yet-realized label effects, folds them into the same
    /// single [`apply_update`](Self::apply_update) call the executor uses, then
    /// verifies the resulting labels. Non-label effects are rejected, mirroring
    /// [`prepare_effects`](Self::prepare_effects)'s discipline. Returns the
    /// effects it actually applied; an empty result means every label already
    /// held, so a re-run is a clean no-op (it issues no backend update).
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
        let mut prepared = PreparedEffects::default();
        let mut postconditions = Vec::new();
        let mut applied = Vec::new();
        for effect in effects {
            match effect {
                WorkflowEffect::AddLabel(label) => {
                    postconditions.push(Postcondition::LabelPresent(label.clone()));
                    if !labels.contains(label.as_str()) {
                        prepared.add_labels.push(label.as_str().to_string());
                        applied.push(effect.clone());
                    }
                }
                WorkflowEffect::RemoveLabel(label) => {
                    postconditions.push(Postcondition::LabelAbsent(label.clone()));
                    if labels.contains(label.as_str()) {
                        prepared.remove_labels.push(label.as_str().to_string());
                        applied.push(effect.clone());
                    }
                }
                other => {
                    return Err(ExecutionError::UnsupportedEffect {
                        effect: other.clone(),
                    })
                }
            }
        }
        self.apply_update(&loaded, prepared).await?;
        self.verify(repo_id, target, &postconditions).await?;
        Ok(applied)
    }

    /// Partitions plan effects into concrete backend operations.
    ///
    /// Runs before any mutation so an unsupported effect, missing create input,
    /// missing correlation key, or unbound assignee role fails the whole
    /// transition cleanly, never half-applied.
    fn prepare_effects(&self, plan: &TransitionPlan) -> Result<PreparedEffects, ExecutionError> {
        let mut prepared = PreparedEffects::default();
        let mut pull_request_create_index = 0;
        for effect in &plan.effects {
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
                WorkflowEffect::CreatePullRequest { correlation_key } => {
                    let effect_index = pull_request_create_index;
                    pull_request_create_index += 1;
                    let correlation_key = correlation_key.clone().ok_or_else(|| {
                        ExecutionError::MissingCorrelationKey {
                            effect: effect.clone(),
                        }
                    })?;
                    let input = self
                        .context
                        .pull_request_create(&plan.transition, effect_index)
                        .cloned()
                        .ok_or_else(|| ExecutionError::UnresolvedPullRequestCreate {
                            transition: plan.transition.clone(),
                            effect_index,
                        })?;
                    prepared
                        .pull_request_creates
                        .push(PreparedPullRequestCreate {
                            correlation_key,
                            input,
                        });
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

    /// Creates requested pull requests idempotently before the label commit
    /// point.
    ///
    /// If a create lands but a later effect crashes before the source artifact's
    /// label flip, retrying the transition reuses the same correlation key and
    /// resolves to the existing pull request instead of creating a duplicate.
    async fn apply_pull_request_creates(
        &self,
        repo_id: &RepositoryId,
        creates: &[PreparedPullRequestCreate],
    ) -> Result<(), ExecutionError> {
        for create in creates {
            self.ensure_pull_request(repo_id, &create.correlation_key, create.input.clone())
                .await?;
        }
        Ok(())
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
        target: ArtifactSource,
    ) -> Result<(Vec<String>, Vec<UserId>), ExecutionError> {
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

/// Effects partitioned into a single backend update plus pre-commit effects.
///
/// Labels and assignees are applied together in one `UpdateIssue`/
/// `UpdatePullRequest` so the state flip is atomic; comments and pull-request
/// creates are applied separately (and idempotently) before that update.
#[derive(Default)]
struct PreparedEffects {
    add_labels: Vec<String>,
    remove_labels: Vec<String>,
    add_assignees: Vec<UserId>,
    remove_assignees: Vec<UserId>,
    comments: Vec<String>,
    pull_request_creates: Vec<PreparedPullRequestCreate>,
    /// Whether the plan requests merging the target pull request.
    merge: bool,
}

/// A concrete, idempotent pull-request create request prepared from a plan
/// effect plus the runtime [`crate::context::ExecutionContext`].
struct PreparedPullRequestCreate {
    correlation_key: String,
    input: CreatePullRequest,
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
