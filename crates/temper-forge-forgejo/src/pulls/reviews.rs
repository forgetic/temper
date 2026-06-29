//! Forgejo pull-request review operations: requesting reviewers, listing native
//! review verdicts, and submitting a review in one call.

use super::*;
use crate::ids::{format_review_id, format_user_id};
use crate::map::{map_review, review_event_token};
use crate::types::ReviewDto;
use chrono::Utc;
use temper_forge_model::{CreatePullRequestReview, PullRequestReview, RequestReviewers};

impl<C: HttpClient> ForgejoForge<C> {
    /// Requests reviews from users; idempotent when the set already matches.
    pub async fn request_pull_request_reviewers(
        &self,
        id: &PullRequestId,
        input: RequestReviewers,
    ) -> ForgeResult<PullRequest> {
        let (repo, number) = parse_pull_request_id(id)?;
        let reviewers: Vec<&str> = input
            .reviewers
            .iter()
            .map(temper_forge_model::UserId::as_str)
            .collect();
        let path = format!(
            "/repos/{}/pulls/{}/requested_reviewers",
            repo.path_segment(),
            number.get()
        );
        let payload =
            serde_json::json!({ "reviewers": reviewers, "team_reviewers": [] }).to_string();
        let response = self
            .send(HttpMethod::Post, &path, Vec::new(), Some(payload))
            .await?;

        if response.is_success() {
            return self
                .fetch_pull_request(&repo, number)
                .await?
                .ok_or_else(|| ForgeError::NotFound(format!("pull request {id}")));
        }
        if response.status == 404 {
            return Err(ForgeError::NotFound(format!("pull request {id}")));
        }

        // Forgejo rejects re-requesting an already-requested reviewer. Treat the
        // call as idempotent when the desired reviewers are already present.
        if let Some(pull) = self.fetch_pull_request(&repo, number).await? {
            if input
                .reviewers
                .iter()
                .all(|reviewer| pull.requested_reviewers.contains(reviewer))
            {
                return Ok(pull);
            }
        }
        Err(crate::error::map_status_error(
            "request reviewers",
            &response,
        ))
    }

    /// Lists native review verdicts in chronological order, skipping only
    /// non-verdict (review-request/pending) events. Dismissed and stale verdicts
    /// are **kept** so history (e.g. a changes-requested review later auto-
    /// dismissed by an approval) matches the reference backends; see
    /// [`map_review`](crate::map::map_review).
    pub async fn list_pull_request_reviews(
        &self,
        id: &PullRequestId,
    ) -> ForgeResult<Vec<PullRequestReview>> {
        let (repo, number) = parse_pull_request_id(id)?;
        let path = format!(
            "/repos/{}/pulls/{}/reviews",
            repo.path_segment(),
            number.get()
        );
        let dtos: Vec<ReviewDto> = self.list_all("list reviews", &path, Vec::new()).await?;
        let mut reviews: Vec<PullRequestReview> = dtos
            .into_iter()
            .filter_map(|dto| map_review(&repo, id, dto))
            .collect();
        reviews.sort_by(|left, right| {
            left.submitted_at
                .cmp(&right.submitted_at)
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(reviews)
    }

    /// Submits a native review in one call. Pending is rejected as invalid.
    pub async fn submit_pull_request_review(
        &self,
        id: &PullRequestId,
        input: CreatePullRequestReview,
    ) -> ForgeResult<PullRequestReview> {
        let (repo, number) = parse_pull_request_id(id)?;
        let event = review_event_token(input.decision)?;
        let body = input.body.clone().unwrap_or_default();
        let path = format!(
            "/repos/{}/pulls/{}/reviews",
            repo.path_segment(),
            number.get()
        );
        let payload = serde_json::json!({ "event": event, "body": body }).to_string();
        let response = self
            .request_checked(
                "submit review",
                HttpMethod::Post,
                &path,
                Vec::new(),
                Some(payload),
            )
            .await?;
        let dto: ReviewDto = Self::decode("submit review", &response)?;

        if let Some(review) = map_review(&repo, id, dto.clone()) {
            return Ok(review);
        }
        // Fall back to the decision we submitted if the provider echo is sparse.
        let submitted_at = dto
            .submitted_at
            .or(dto.updated_at)
            .or(dto.created_at)
            .unwrap_or_else(Utc::now);
        Ok(PullRequestReview {
            id: format_review_id(&repo, dto.id),
            pull_request_id: id.clone(),
            reviewer_id: format_user_id(&dto.user.login),
            decision: input.decision,
            body: input.body,
            submitted_at,
        })
    }
}
