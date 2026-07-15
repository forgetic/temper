use crate::FilesystemForge;
use crate::lists::{next_pull_request_comment_number, sort_comments};
use crate::merge::merge_pull_request as merge_pull_request_impl;
use crate::metadata::next_timestamp;
use crate::record_ids::pull_request_comment_id;
use crate::reviews::{list_reviews, request_reviewers, submit_review};
use temper_forge_model::{
    ChangeKind, Comment, CreateComment, CreatePullRequestReview, ForgeError, ForgeResult,
    MergePullRequest, MergeRecord, PullRequest, PullRequestId, PullRequestReview, RequestReviewers,
};

pub(crate) fn request_pull_request_reviewers(
    forge: &FilesystemForge,
    id: &PullRequestId,
    input: RequestReviewers,
) -> ForgeResult<PullRequest> {
    let _guard = forge.write_lock()?;
    let pull_request = request_reviewers(forge, id, input)?;
    forge.publish_pull_request_hint(&pull_request, ChangeKind::Review);
    Ok(pull_request)
}

pub(crate) fn list_pull_request_reviews(
    forge: &FilesystemForge,
    id: &PullRequestId,
) -> ForgeResult<Vec<PullRequestReview>> {
    list_reviews(forge, id)
}

pub(crate) fn submit_pull_request_review(
    forge: &FilesystemForge,
    id: &PullRequestId,
    input: CreatePullRequestReview,
) -> ForgeResult<PullRequestReview> {
    let _guard = forge.write_lock()?;
    let review = submit_review(forge, id, input)?;
    if let Some(pull_request) = forge.find_pull_request_by_id(id)? {
        forge.publish_pull_request_hint(&pull_request, ChangeKind::Review);
    }
    Ok(review)
}

pub(crate) fn list_pull_request_comments(
    forge: &FilesystemForge,
    id: &PullRequestId,
) -> ForgeResult<Vec<Comment>> {
    let repo_id = forge
        .find_pull_request_repository_by_id(id)?
        .ok_or_else(|| ForgeError::NotFound(format!("pull request {id}")))?;

    forge.read_pull_request_comments_for_existing_pull_request(&repo_id, id)
}

pub(crate) fn add_pull_request_comment(
    forge: &FilesystemForge,
    id: &PullRequestId,
    input: CreateComment,
) -> ForgeResult<Comment> {
    let _guard = forge.write_lock()?;
    let repo_id = forge
        .find_pull_request_repository_by_id(id)?
        .ok_or_else(|| ForgeError::NotFound(format!("pull request {id}")))?;

    let mut comments = forge.read_pull_request_comments_for_existing_pull_request(&repo_id, id)?;
    let comment_number = next_pull_request_comment_number(id, &comments)?;
    let mut metadata = forge.read_metadata()?;
    let author_id = forge.effective_user(&metadata).id;
    let now = next_timestamp(&mut metadata)?;
    let comment = Comment {
        id: pull_request_comment_id(id, comment_number),
        author_id,
        body: input.body,
        created_at: now,
        updated_at: now,
    };

    comments.push(comment.clone());
    sort_comments(&mut comments);
    forge.write_pull_request_comments(&repo_id, id, &comments)?;
    forge.write_metadata(&metadata)?;
    if let Some(pull_request) = forge.find_pull_request_by_id(id)? {
        forge.publish_pull_request_hint(&pull_request, ChangeKind::Comment);
    }

    Ok(comment)
}

pub(crate) fn merge_pull_request(
    forge: &FilesystemForge,
    id: &PullRequestId,
    input: MergePullRequest,
) -> ForgeResult<MergeRecord> {
    let _guard = forge.write_lock()?;
    merge_pull_request_impl(forge, id, input)
}
