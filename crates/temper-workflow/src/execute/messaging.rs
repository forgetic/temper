//! Idempotent comment and native-review application for the [`Executor`].
//!
//! This child module holds the at-most-once comment, reviewer-request, and
//! native-review apply paths plus their HTML-comment idempotency markers. It is
//! split from the sibling `apply` module to keep both files within the
//! source-size budget; it accesses the parent's private [`Executor`] and
//! [`Loaded`] items as a descendant module.

use super::{ExecutionError, Executor, Loaded};
use crate::ids::TransitionId;
use crate::plan::WorkflowEffect;
use temper_forge_model::{
    CreateComment, CreatePullRequestReview, Forge, RequestReviewers, ReviewDecision, UserId,
};

/// A concrete, idempotent native-review submission prepared from an
/// `AttachReview` effect plus the runtime [`crate::context::ExecutionContext`].
///
/// The decision is portable workflow vocabulary; the body is the agent-authored
/// work product. The correlation key makes the submission at-most-once across
/// retries through the review idempotency marker.
pub(super) struct PreparedAttachReview {
    pub(super) decision: ReviewDecision,
    pub(super) correlation_key: String,
    pub(super) body: String,
}

impl<F: Forge + ?Sized> Executor<'_, F> {
    /// Requests reviewers on the target pull request idempotently.
    pub(super) async fn apply_review_requests(
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
    pub(super) async fn apply_reviews(
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
        id: &temper_forge_model::PullRequestId,
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
    pub(super) async fn apply_attach_reviews(
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
    pub(super) async fn apply_comments(
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
