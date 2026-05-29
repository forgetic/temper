use crate::record_ids::{
    issue_comment_id, issue_id, pull_request_comment_id, pull_request_id,
    stored_issue_comment_number, stored_pull_request_comment_number,
};
use harness_forge::{
    Comment, CommentId, CreateRepository, ForgeError, ForgeResult, Issue, IssueId, Label,
    PullRequest, PullRequestId, PullRequestState, RepositoryId, UpsertLabel,
};

pub(crate) fn validate_create_repository(input: &CreateRepository) -> ForgeResult<()> {
    if input.owner.trim().is_empty() {
        return Err(ForgeError::InvalidRequest(
            "repository owner must not be empty".into(),
        ));
    }
    if input.name.trim().is_empty() {
        return Err(ForgeError::InvalidRequest(
            "repository name must not be empty".into(),
        ));
    }
    if input.default_branch.trim().is_empty() {
        return Err(ForgeError::InvalidRequest(
            "repository default branch must not be empty".into(),
        ));
    }

    Ok(())
}

pub(crate) fn validate_upsert_label(input: &UpsertLabel) -> ForgeResult<()> {
    if input.name.trim().is_empty() {
        return Err(ForgeError::InvalidRequest(
            "label name must not be empty".into(),
        ));
    }

    Ok(())
}

pub(crate) fn validate_stored_labels(repo_id: &RepositoryId, labels: &[Label]) -> ForgeResult<()> {
    for (index, label) in labels.iter().enumerate() {
        if &label.repo_id != repo_id {
            return Err(ForgeError::Backend(format!(
                "label {} belongs to repository {}, expected {repo_id}",
                label.id, label.repo_id
            )));
        }

        if labels[..index]
            .iter()
            .any(|previous| previous.name == label.name)
        {
            return Err(ForgeError::Backend(format!(
                "filesystem storage contains duplicate label {} in repository {repo_id}",
                label.name
            )));
        }
    }

    Ok(())
}

pub(crate) fn validate_stored_issues(repo_id: &RepositoryId, issues: &[Issue]) -> ForgeResult<()> {
    for (index, issue) in issues.iter().enumerate() {
        if &issue.repo_id != repo_id {
            return Err(ForgeError::Backend(format!(
                "issue {} belongs to repository {}, expected {repo_id}",
                issue.id, issue.repo_id
            )));
        }

        if issue.number.get() == 0 {
            return Err(ForgeError::Backend(format!(
                "issue {} in repository {repo_id} has number 0",
                issue.id
            )));
        }

        let expected_id = issue_id(repo_id, issue.number);
        if issue.id.as_str() != expected_id.as_str() {
            return Err(ForgeError::Backend(format!(
                "issue {} in repository {repo_id} should have deterministic id {expected_id}",
                issue.id
            )));
        }

        if issues[..index]
            .iter()
            .any(|previous| previous.id == issue.id)
        {
            return Err(ForgeError::Backend(format!(
                "filesystem storage contains duplicate issue id {} in repository {repo_id}",
                issue.id
            )));
        }

        if issues[..index]
            .iter()
            .any(|previous| previous.number == issue.number)
        {
            return Err(ForgeError::Backend(format!(
                "filesystem storage contains duplicate issue number {} in repository {repo_id}",
                issue.number
            )));
        }
    }

    Ok(())
}

pub(crate) fn validate_stored_pull_requests(
    repo_id: &RepositoryId,
    pull_requests: &[PullRequest],
) -> ForgeResult<()> {
    for (index, pull_request) in pull_requests.iter().enumerate() {
        if &pull_request.repo_id != repo_id {
            return Err(ForgeError::Backend(format!(
                "pull request {} belongs to repository {}, expected {repo_id}",
                pull_request.id, pull_request.repo_id
            )));
        }

        if pull_request.number.get() == 0 {
            return Err(ForgeError::Backend(format!(
                "pull request {} in repository {repo_id} has number 0",
                pull_request.id
            )));
        }

        let expected_id = pull_request_id(repo_id, pull_request.number);
        if pull_request.id.as_str() != expected_id.as_str() {
            return Err(ForgeError::Backend(format!(
                "pull request {} in repository {repo_id} should have deterministic id {expected_id}",
                pull_request.id
            )));
        }

        if pull_requests[..index]
            .iter()
            .any(|previous| previous.id == pull_request.id)
        {
            return Err(ForgeError::Backend(format!(
                "filesystem storage contains duplicate pull request id {} in repository {repo_id}",
                pull_request.id
            )));
        }

        if pull_requests[..index]
            .iter()
            .any(|previous| previous.number == pull_request.number)
        {
            return Err(ForgeError::Backend(format!(
                "filesystem storage contains duplicate pull request number {} in repository {repo_id}",
                pull_request.number
            )));
        }

        match (pull_request.state, &pull_request.merge) {
            (PullRequestState::Merged, Some(merge)) if merge.commit_sha.trim().is_empty() => {
                return Err(ForgeError::Backend(format!(
                    "merged pull request {} in repository {repo_id} has empty merge commit SHA",
                    pull_request.id
                )));
            }
            (PullRequestState::Merged, None) => {
                return Err(ForgeError::Backend(format!(
                    "merged pull request {} in repository {repo_id} has no merge record",
                    pull_request.id
                )));
            }
            (PullRequestState::Open | PullRequestState::Closed, Some(_)) => {
                return Err(ForgeError::Backend(format!(
                    "unmerged pull request {} in repository {repo_id} has a merge record",
                    pull_request.id
                )));
            }
            _ => {}
        }
    }

    Ok(())
}

pub(crate) fn validate_stored_issue_comments(
    issue_id: &IssueId,
    comments: &[Comment],
) -> ForgeResult<()> {
    validate_stored_comments(
        "issue",
        issue_id.as_str(),
        comments,
        |comment| stored_issue_comment_number(issue_id, &comment.id),
        |number| issue_comment_id(issue_id, number),
    )
}

pub(crate) fn validate_stored_pull_request_comments(
    pull_request_id: &PullRequestId,
    comments: &[Comment],
) -> ForgeResult<()> {
    validate_stored_comments(
        "pull request",
        pull_request_id.as_str(),
        comments,
        |comment| stored_pull_request_comment_number(pull_request_id, &comment.id),
        |number| pull_request_comment_id(pull_request_id, number),
    )
}

fn validate_stored_comments(
    target_kind: &str,
    target_id: &str,
    comments: &[Comment],
    mut stored_number: impl FnMut(&Comment) -> ForgeResult<u64>,
    mut expected_id: impl FnMut(u64) -> CommentId,
) -> ForgeResult<()> {
    for (index, comment) in comments.iter().enumerate() {
        let comment_number = stored_number(comment)?;
        if comment_number == 0 {
            return Err(ForgeError::Backend(format!(
                "comment {} on {target_kind} {target_id} has number 0",
                comment.id
            )));
        }

        let expected_comment_id = expected_id(comment_number);
        if comment.id.as_str() != expected_comment_id.as_str() {
            return Err(ForgeError::Backend(format!(
                "comment {} on {target_kind} {target_id} should have deterministic id {expected_comment_id}",
                comment.id
            )));
        }

        if comments[..index]
            .iter()
            .any(|previous| previous.id == comment.id)
        {
            return Err(ForgeError::Backend(format!(
                "filesystem storage contains duplicate comment id {} on {target_kind} {target_id}",
                comment.id
            )));
        }
    }

    Ok(())
}
