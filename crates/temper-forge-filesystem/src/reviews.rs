use crate::lists::{
    next_pull_request_review_number, normalize_user_set, sort_pull_requests_by_number, sort_reviews,
};
use crate::metadata::next_timestamp;
use crate::record_ids::pull_request_review_id;
use crate::FilesystemForge;
use temper_forge::{
    CreatePullRequestReview, ForgeError, ForgeResult, PullRequest, PullRequestId,
    PullRequestReview, RequestReviewers,
};

pub(crate) fn request_reviewers(
    forge: &FilesystemForge,
    id: &PullRequestId,
    input: RequestReviewers,
) -> ForgeResult<PullRequest> {
    let repo_id = forge
        .find_pull_request_repository_by_id(id)?
        .ok_or_else(|| ForgeError::NotFound(format!("pull request {id}")))?;
    let mut pull_requests = forge.read_pull_requests_for_existing_repository(&repo_id)?;
    let pull_request = pull_requests
        .iter_mut()
        .find(|pull_request| &pull_request.id == id)
        .ok_or_else(|| ForgeError::NotFound(format!("pull request {id}")))?;

    let mut requested = normalize_user_set(pull_request.requested_reviewers.clone());
    let before = requested.clone();
    for reviewer in normalize_user_set(input.reviewers) {
        if !requested.contains(&reviewer) {
            requested.push(reviewer);
        }
    }
    requested.sort();
    if requested == before {
        return Ok(pull_request.clone());
    }

    let mut metadata = forge.read_metadata()?;
    let now = next_timestamp(&mut metadata)?;
    pull_request.requested_reviewers = requested;
    pull_request.version = pull_request.version.next();
    pull_request.updated_at = now;
    let updated = pull_request.clone();
    sort_pull_requests_by_number(&mut pull_requests);
    forge.write_pull_requests(&repo_id, &pull_requests)?;
    forge.write_metadata(&metadata)?;
    Ok(updated)
}

pub(crate) fn list_reviews(
    forge: &FilesystemForge,
    id: &PullRequestId,
) -> ForgeResult<Vec<PullRequestReview>> {
    let repo_id = forge
        .find_pull_request_repository_by_id(id)?
        .ok_or_else(|| ForgeError::NotFound(format!("pull request {id}")))?;
    forge.read_pull_request_reviews_for_existing_pull_request(&repo_id, id)
}

pub(crate) fn submit_review(
    forge: &FilesystemForge,
    id: &PullRequestId,
    input: CreatePullRequestReview,
) -> ForgeResult<PullRequestReview> {
    let repo_id = forge
        .find_pull_request_repository_by_id(id)?
        .ok_or_else(|| ForgeError::NotFound(format!("pull request {id}")))?;
    let mut reviews = forge.read_pull_request_reviews_for_existing_pull_request(&repo_id, id)?;
    let number = next_pull_request_review_number(id, &reviews)?;
    let mut metadata = forge.read_metadata()?;
    let reviewer_id = forge.effective_user(&metadata).id;
    let now = next_timestamp(&mut metadata)?;
    let review = PullRequestReview {
        id: pull_request_review_id(id, number),
        pull_request_id: id.clone(),
        reviewer_id,
        decision: input.decision,
        body: input.body,
        submitted_at: now,
    };
    reviews.push(review.clone());
    sort_reviews(&mut reviews);
    forge.write_pull_request_reviews(&repo_id, id, &reviews)?;
    forge.write_metadata(&metadata)?;
    Ok(review)
}
