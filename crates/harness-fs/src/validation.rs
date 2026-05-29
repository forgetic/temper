use crate::record_ids::{comment_id, issue_id, pull_request_id, stored_comment_number};
use harness_forge::{
    Comment, CreateRepository, ForgeError, ForgeResult, Issue, IssueId, Label, PullRequest,
    RepositoryId, UpsertLabel,
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
    }

    Ok(())
}

pub(crate) fn validate_stored_comments(
    issue_id: &IssueId,
    comments: &[Comment],
) -> ForgeResult<()> {
    for (index, comment) in comments.iter().enumerate() {
        let comment_number = stored_comment_number(issue_id, &comment.id)?;
        if comment_number == 0 {
            return Err(ForgeError::Backend(format!(
                "comment {} on issue {issue_id} has number 0",
                comment.id
            )));
        }

        let expected_id = comment_id(issue_id, comment_number);
        if comment.id.as_str() != expected_id.as_str() {
            return Err(ForgeError::Backend(format!(
                "comment {} on issue {issue_id} should have deterministic id {expected_id}",
                comment.id
            )));
        }

        if comments[..index]
            .iter()
            .any(|previous| previous.id == comment.id)
        {
            return Err(ForgeError::Backend(format!(
                "filesystem storage contains duplicate comment id {} on issue {issue_id}",
                comment.id
            )));
        }
    }

    Ok(())
}
