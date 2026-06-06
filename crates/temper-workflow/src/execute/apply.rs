//! Effect application and postcondition verification for the [`Executor`].
//!
//! This child module holds the mutation half of the runtime loop: turning a
//! plan's [`WorkflowEffect`]s into Forge calls and verifying the committed
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
use std::collections::HashSet;
use temper_forge::{
    CreateComment, CreatePullRequest, CreatePullRequestReview, Forge, RepositoryId,
    RequestReviewers, ReviewDecision, UpdateIssue, UpdatePullRequest, UserId,
};

impl<F: Forge + ?Sized> Executor<'_, F> {
    /// Applies a plan's effects, refusing partial application of the state flip.
    ///
    /// Validates effects before mutating, runs pre-commit idempotent effects,
    /// then folds labels and assignees into one backend update. Postconditions
    /// are checked against that update result, not a later reload that another
    /// worker may already have advanced.
    pub(super) async fn apply(
        &self,
        repo_id: &RepositoryId,
        loaded: &Loaded,
        plan: &TransitionPlan,
    ) -> Result<(), ExecutionError> {
        let prepared = self.prepare_effects(plan)?;
        validate_pull_request_effects(loaded, &prepared)?;
        self.apply_comments(loaded, &plan.transition, &prepared.comments)
            .await?;
        self.apply_pull_request_creates(repo_id, &prepared.pull_request_creates)
            .await?;
        self.apply_review_requests(loaded, &prepared.review_requests)
            .await?;
        self.apply_reviews(loaded, &plan.transition, &prepared.reviews)
            .await?;
        self.apply_attach_reviews(loaded, &prepared.attach_reviews)
            .await?;
        self.apply_merge(repo_id, loaded, prepared.merge).await?;
        let committed = self.apply_update(loaded, prepared).await?;
        if let Some(state) = committed {
            self.verify_state(&state, &plan.postconditions)?;
        } else {
            self.verify_current(repo_id, loaded.classified().source, &plan.postconditions)
                .await?;
        }
        Ok(())
    }

    async fn verify_current(
        &self,
        repo_id: &temper_forge::RepositoryId,
        target: ArtifactSource,
        postconditions: &[Postcondition],
    ) -> Result<(), ExecutionError> {
        if postconditions.is_empty() {
            return Ok(());
        }
        let state = self.current_state(repo_id, target).await?;
        self.verify_state(&state, postconditions)
    }

    fn verify_state(
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
        let committed = self.apply_update(&loaded, prepared).await?;
        if let Some(state) = committed {
            self.verify_state(&state, &postconditions)?;
        } else {
            self.verify_current(repo_id, target, &postconditions)
                .await?;
        }
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
        let mut set_body_index = 0;
        let mut attach_review_index = 0;
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
                    let correlation_key = correlation_key
                        .clone()
                        .or_else(|| {
                            self.context
                                .pull_request_correlation_key(&plan.transition, effect_index)
                                .map(str::to_string)
                        })
                        .ok_or_else(|| ExecutionError::MissingCorrelationKey {
                            effect: effect.clone(),
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
                WorkflowEffect::RequestReviewers { roles } => {
                    for role in roles {
                        let reviewer =
                            self.context
                                .resolve_assignee(role)
                                .cloned()
                                .ok_or_else(|| ExecutionError::UnresolvedReviewer {
                                    role: role.clone(),
                                })?;
                        prepared.review_requests.push(reviewer);
                    }
                }
                WorkflowEffect::SubmitReview { decision } => {
                    prepared.reviews.push(*decision);
                }
                WorkflowEffect::SetBody { correlation_key: _ } => {
                    let effect_index = set_body_index;
                    set_body_index += 1;
                    // The authored body is the runtime work product; an effect
                    // with no bound body fails before any mutation, exactly like
                    // a `CreatePullRequest` with no bound create input. The
                    // correlation key is accepted for symmetry but `set_body`
                    // overwrites and so is naturally idempotent across retries.
                    let body = self
                        .context
                        .set_body(&plan.transition, effect_index)
                        .map(str::to_string)
                        .ok_or_else(|| ExecutionError::UnresolvedSetBody {
                            transition: plan.transition.clone(),
                            effect_index,
                        })?;
                    prepared.body = Some(body);
                }
                WorkflowEffect::AttachReview {
                    decision,
                    correlation_key,
                } => {
                    let effect_index = attach_review_index;
                    attach_review_index += 1;
                    let correlation_key = correlation_key
                        .clone()
                        .or_else(|| {
                            self.context
                                .attach_review_correlation_key(&plan.transition, effect_index)
                                .map(str::to_string)
                        })
                        .ok_or_else(|| ExecutionError::MissingCorrelationKey {
                            effect: effect.clone(),
                        })?;
                    let body = self
                        .context
                        .attach_review(&plan.transition, effect_index)
                        .map(str::to_string)
                        .ok_or_else(|| ExecutionError::UnresolvedAttachReview {
                            transition: plan.transition.clone(),
                            effect_index,
                        })?;
                    prepared.attach_reviews.push(PreparedAttachReview {
                        decision: *decision,
                        correlation_key,
                        body,
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

    /// Requests reviewers on the target pull request idempotently.
    async fn apply_review_requests(
        &self,
        loaded: &Loaded,
        reviewers: &[UserId],
    ) -> Result<(), ExecutionError> {
        if reviewers.is_empty() {
            return Ok(());
        }
        let Loaded::PullRequest { id, .. } = loaded else {
            return Err(ExecutionError::UnsupportedEffect {
                effect: WorkflowEffect::RequestReviewers { roles: Vec::new() },
            });
        };
        self.forge
            .request_pull_request_reviewers(
                id,
                RequestReviewers {
                    reviewers: reviewers.to_vec(),
                },
            )
            .await?;
        Ok(())
    }

    /// Submits planned native reviews at most once per transition review effect.
    async fn apply_reviews(
        &self,
        loaded: &Loaded,
        transition: &TransitionId,
        reviews: &[ReviewDecision],
    ) -> Result<(), ExecutionError> {
        let Loaded::PullRequest { id, .. } = loaded else {
            if reviews.is_empty() {
                return Ok(());
            }
            return Err(ExecutionError::UnsupportedEffect {
                effect: WorkflowEffect::SubmitReview {
                    decision: ReviewDecision::Commented,
                },
            });
        };
        for (index, decision) in reviews.iter().enumerate() {
            let key = review_key(transition, index);
            if self.review_exists(id, &key).await? {
                continue;
            }
            self.forge
                .submit_pull_request_review(
                    id,
                    CreatePullRequestReview {
                        decision: *decision,
                        body: Some(review_marker(&key)),
                    },
                )
                .await?;
        }
        Ok(())
    }

    async fn review_exists(
        &self,
        id: &temper_forge::PullRequestId,
        key: &str,
    ) -> Result<bool, ExecutionError> {
        let marker = review_marker(key);
        let reviews = self.forge.list_pull_request_reviews(id).await?;
        Ok(reviews.iter().any(|review| {
            review
                .body
                .as_deref()
                .is_some_and(|body| body.contains(&marker))
        }))
    }

    /// Submits each `AttachReview` effect's native review at most once.
    ///
    /// Unlike [`apply_reviews`](Self::apply_reviews), the review carries an
    /// agent-authored body from the workspace work product, and the idempotency
    /// marker is keyed by the effect's correlation key (a work-item-scoped
    /// token) so a retry after a crash dedupes even from a different worker —
    /// the same discipline [`apply_pull_request_creates`](Self::apply_pull_request_creates)
    /// uses for content-bearing creates.
    async fn apply_attach_reviews(
        &self,
        loaded: &Loaded,
        reviews: &[PreparedAttachReview],
    ) -> Result<(), ExecutionError> {
        let Loaded::PullRequest { id, .. } = loaded else {
            if reviews.is_empty() {
                return Ok(());
            }
            return Err(ExecutionError::UnsupportedEffect {
                effect: WorkflowEffect::AttachReview {
                    decision: reviews
                        .first()
                        .map(|review| review.decision)
                        .unwrap_or(ReviewDecision::Commented),
                    correlation_key: None,
                },
            });
        };
        for review in reviews {
            let key = attach_review_key(&review.correlation_key);
            if self.review_exists(id, &key).await? {
                continue;
            }
            self.forge
                .submit_pull_request_review(
                    id,
                    CreatePullRequestReview {
                        decision: review.decision,
                        body: Some(attach_review_body_with_marker(&review.body, &key)),
                    },
                )
                .await?;
        }
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
    /// comment-only transition), so it never issues an empty mutation. When it
    /// does update, returns the backend's committed artifact state for
    /// postcondition checks.
    async fn apply_update(
        &self,
        loaded: &Loaded,
        prepared: PreparedEffects,
    ) -> Result<Option<AppliedState>, ExecutionError> {
        if !prepared.has_update() {
            return Ok(None);
        }
        match loaded {
            Loaded::Issue { id, .. } => {
                let update = UpdateIssue {
                    body: prepared.body,
                    add_labels: prepared.add_labels,
                    remove_labels: prepared.remove_labels,
                    add_assignees: prepared.add_assignees,
                    remove_assignees: prepared.remove_assignees,
                    ..UpdateIssue::default()
                };
                let issue = self.forge.update_issue(id, update).await?;
                Ok(Some(AppliedState {
                    labels: issue.labels,
                    assignees: issue.assignees,
                }))
            }
            Loaded::PullRequest { id, .. } => {
                let update = UpdatePullRequest {
                    body: prepared.body,
                    add_labels: prepared.add_labels,
                    remove_labels: prepared.remove_labels,
                    add_assignees: prepared.add_assignees,
                    remove_assignees: prepared.remove_assignees,
                    ..UpdatePullRequest::default()
                };
                let pull_request = self.forge.update_pull_request(id, update).await?;
                Ok(Some(AppliedState {
                    labels: pull_request.labels,
                    assignees: pull_request.assignees,
                }))
            }
        }
    }

    /// Reads the artifact's current labels and assignees from fresh Forge state.
    async fn current_state(
        &self,
        repo_id: &temper_forge::RepositoryId,
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

/// Labels and assignees returned by the backend immediately after the commit update.
struct AppliedState {
    labels: Vec<String>,
    assignees: Vec<UserId>,
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
    review_requests: Vec<UserId>,
    reviews: Vec<ReviewDecision>,
    attach_reviews: Vec<PreparedAttachReview>,
    /// Agent-authored body to write onto the target, folded into the same
    /// atomic label/assignee commit update. The last `SetBody` effect wins.
    body: Option<String>,
    /// Whether the plan requests merging the target pull request.
    merge: bool,
}

/// A concrete, idempotent pull-request create request prepared from a plan
/// effect plus the runtime [`crate::context::ExecutionContext`].
struct PreparedPullRequestCreate {
    correlation_key: String,
    input: CreatePullRequest,
}

/// A concrete, idempotent native-review submission prepared from an
/// `AttachReview` effect plus the runtime [`crate::context::ExecutionContext`].
///
/// The decision is portable workflow vocabulary; the body is the agent-authored
/// work product. The correlation key makes the submission at-most-once across
/// retries through the review idempotency marker.
struct PreparedAttachReview {
    decision: ReviewDecision,
    correlation_key: String,
    body: String,
}

fn validate_pull_request_effects(
    loaded: &Loaded,
    prepared: &PreparedEffects,
) -> Result<(), ExecutionError> {
    if matches!(loaded, Loaded::PullRequest { .. })
        || (prepared.review_requests.is_empty()
            && prepared.reviews.is_empty()
            && prepared.attach_reviews.is_empty())
    {
        return Ok(());
    }
    let effect = if !prepared.review_requests.is_empty() {
        WorkflowEffect::RequestReviewers { roles: Vec::new() }
    } else if let Some(decision) = prepared.reviews.first().copied() {
        WorkflowEffect::SubmitReview { decision }
    } else {
        WorkflowEffect::AttachReview {
            decision: prepared
                .attach_reviews
                .first()
                .map(|review| review.decision)
                .unwrap_or(ReviewDecision::Commented),
            correlation_key: None,
        }
    };
    Err(ExecutionError::UnsupportedEffect { effect })
}

impl PreparedEffects {
    /// Returns `true` when the single label/assignee/body update would change
    /// anything, so a comment-only transition never issues an empty mutation.
    fn has_update(&self) -> bool {
        !(self.add_labels.is_empty()
            && self.remove_labels.is_empty()
            && self.add_assignees.is_empty()
            && self.remove_assignees.is_empty()
            && self.body.is_none())
    }
}

/// Opening text of the HTML comment marker that makes a comment idempotent.
const COMMENT_MARKER_PREFIX: &str = "<!-- temper:comment-key=";
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

const REVIEW_MARKER_PREFIX: &str = "<!-- temper:review-key=";
const REVIEW_MARKER_SUFFIX: &str = " -->";

fn review_key(transition: &TransitionId, index: usize) -> String {
    format!("{transition}:{index}")
}

fn review_marker(key: &str) -> String {
    format!("{REVIEW_MARKER_PREFIX}{key}{REVIEW_MARKER_SUFFIX}")
}

/// Idempotency key for an `AttachReview` submission.
///
/// Keyed by the effect's correlation key — a work-item-scoped token, not the
/// transition/index pair — so a retry dedupes the authored review even when a
/// different worker resumes after a crash.
fn attach_review_key(correlation_key: &str) -> String {
    format!("attach-review:{correlation_key}")
}

/// Appends the review idempotency marker to an agent-authored review body.
///
/// The marker is an HTML comment, so it renders invisibly in Forge markdown
/// while remaining searchable by [`review_marker`] / [`review_exists`].
fn attach_review_body_with_marker(body: &str, key: &str) -> String {
    format!("{body}\n\n{}", review_marker(key))
}
