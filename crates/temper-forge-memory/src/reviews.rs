use crate::MemoryForge;
use crate::ids::pull_request_review_id;
use crate::lists::{normalize_user_set, sort_pull_requests_by_number, sort_reviews};
use crate::util::next_comment_number;
use temper_forge::{
    CreatePullRequestReview, ForgeError, ForgeResult, ItemNumber, PullRequest, PullRequestId,
    PullRequestReview, RepositoryId, RequestReviewers,
};

pub(crate) fn request_reviewers(
    forge: &MemoryForge,
    id: &PullRequestId,
    input: RequestReviewers,
) -> ForgeResult<(PullRequest, bool)> {
    let mut inner = forge.lock();
    let (repo_id, existing) = inner
        .state
        .find_pull_request(id)
        .ok_or_else(|| ForgeError::NotFound(format!("pull request {id}")))?;
    let mut requested = normalize_user_set(existing.requested_reviewers.clone());
    let before = requested.clone();
    for reviewer in normalize_user_set(input.reviewers) {
        if !requested.contains(&reviewer) {
            requested.push(reviewer);
        }
    }
    requested.sort();
    if requested == before {
        return Ok((existing, false));
    }

    let now = inner.state.next_timestamp()?;
    let pull_requests = inner.state.pull_requests_mut(&repo_id);
    let pull_request = pull_requests
        .iter_mut()
        .find(|pull_request| &pull_request.id == id)
        .ok_or_else(|| ForgeError::NotFound(format!("pull request {id}")))?;
    pull_request.requested_reviewers = requested;
    pull_request.version = pull_request.version.next();
    pull_request.updated_at = now;
    let updated = pull_request.clone();
    sort_pull_requests_by_number(pull_requests);
    Ok((updated, true))
}

pub(crate) fn list_reviews(
    forge: &MemoryForge,
    id: &PullRequestId,
) -> ForgeResult<Vec<PullRequestReview>> {
    let inner = forge.lock();
    inner
        .state
        .find_pull_request(id)
        .ok_or_else(|| ForgeError::NotFound(format!("pull request {id}")))?;
    let mut reviews = inner.state.pull_request_reviews(id);
    sort_reviews(&mut reviews);
    Ok(reviews)
}

pub(crate) fn submit_review(
    forge: &MemoryForge,
    id: &PullRequestId,
    input: CreatePullRequestReview,
) -> ForgeResult<(PullRequestReview, RepositoryId, ItemNumber)> {
    let mut inner = forge.lock();
    let (repo_id, pull_request) = inner
        .state
        .find_pull_request(id)
        .ok_or_else(|| ForgeError::NotFound(format!("pull request {id}")))?;
    let now = inner.state.next_timestamp()?;
    let reviewer_id = forge.effective_user(&inner).id;
    let reviews = inner.state.pull_request_reviews_mut(id);
    let number = next_comment_number(reviews.len())?;
    let review = PullRequestReview {
        id: pull_request_review_id(id, number),
        pull_request_id: id.clone(),
        reviewer_id,
        decision: input.decision,
        body: input.body,
        submitted_at: now,
    };
    reviews.push(review.clone());
    sort_reviews(reviews);
    Ok((review, repo_id, pull_request.number))
}
