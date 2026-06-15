use crate::record_ids::{
    stored_issue_comment_number, stored_pull_request_comment_number,
    stored_pull_request_review_number,
};
use temper_forge_model::{
    Comment, CommentId, ForgeError, ForgeResult, Issue, IssueId, ItemNumber, PullRequest,
    PullRequestId, PullRequestReview, RepositoryId,
};

pub(crate) fn next_issue_number(
    repo_id: &RepositoryId,
    issues: &[Issue],
) -> ForgeResult<ItemNumber> {
    let next = issues
        .iter()
        .map(|issue| issue.number.get())
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .ok_or_else(|| {
            ForgeError::Backend(format!(
                "issue number counter overflowed for repository {repo_id}"
            ))
        })?;

    Ok(ItemNumber::new(next))
}

pub(crate) fn next_pull_request_number(
    repo_id: &RepositoryId,
    pull_requests: &[PullRequest],
) -> ForgeResult<ItemNumber> {
    let next = pull_requests
        .iter()
        .map(|pull_request| pull_request.number.get())
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .ok_or_else(|| {
            ForgeError::Backend(format!(
                "pull request number counter overflowed for repository {repo_id}"
            ))
        })?;

    Ok(ItemNumber::new(next))
}

pub(crate) fn next_issue_comment_number(
    issue_id: &IssueId,
    comments: &[Comment],
) -> ForgeResult<u64> {
    next_comment_number("issue", issue_id.as_str(), comments, |comment_id| {
        stored_issue_comment_number(issue_id, comment_id)
    })
}

pub(crate) fn next_pull_request_comment_number(
    pull_request_id: &PullRequestId,
    comments: &[Comment],
) -> ForgeResult<u64> {
    next_comment_number(
        "pull request",
        pull_request_id.as_str(),
        comments,
        |comment_id| stored_pull_request_comment_number(pull_request_id, comment_id),
    )
}

pub(crate) fn next_pull_request_review_number(
    pull_request_id: &PullRequestId,
    reviews: &[PullRequestReview],
) -> ForgeResult<u64> {
    reviews
        .iter()
        .map(|review| stored_pull_request_review_number(pull_request_id, &review.id))
        .try_fold(0, |highest, number| number.map(|n| highest.max(n)))?
        .checked_add(1)
        .ok_or_else(|| {
            ForgeError::Backend(format!(
                "review id counter overflowed for pull request {pull_request_id}"
            ))
        })
}

fn next_comment_number(
    target_kind: &str,
    target_id: &str,
    comments: &[Comment],
    mut stored_number: impl FnMut(&CommentId) -> ForgeResult<u64>,
) -> ForgeResult<u64> {
    comments
        .iter()
        .map(|comment| stored_number(&comment.id))
        .try_fold(0, |highest, number| {
            number.map(|number| highest.max(number))
        })?
        .checked_add(1)
        .ok_or_else(|| {
            ForgeError::Backend(format!(
                "comment id counter overflowed for {target_kind} {target_id}"
            ))
        })
}
