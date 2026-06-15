use crate::FilesystemForge;
use crate::lists::{sort_issues_by_number, sort_pull_requests_by_number};
use crate::metadata::next_timestamp;
use temper_forge_model::{
    ForgeError, ForgeResult, Issue, IssueId, ItemNumber, PullRequest, PullRequestId, RepositoryId,
};

pub(crate) fn add_issue_dependency(
    forge: &FilesystemForge,
    id: &IssueId,
    target: ItemNumber,
) -> ForgeResult<Issue> {
    let repo_id = forge
        .find_issue_repository_by_id(id)?
        .ok_or_else(|| ForgeError::NotFound(format!("issue {id}")))?;
    let mut issues = forge.read_issues_for_existing_repository(&repo_id)?;
    let index = issue_index(&issues, id)?;
    if issues[index].dependencies.contains(&target) {
        return Ok(issues[index].clone());
    }
    let pull_requests = forge.read_pull_requests_for_existing_repository(&repo_id)?;
    ensure_target_exists(&repo_id, &issues, &pull_requests, target)?;

    let mut metadata = forge.read_metadata()?;
    let now = next_timestamp(&mut metadata)?;
    insert_dependency(&mut issues[index].dependencies, target);
    issues[index].version = issues[index].version.next();
    issues[index].updated_at = now;
    let updated = issues[index].clone();
    sort_issues_by_number(&mut issues);
    forge.write_issues(&repo_id, &issues)?;
    forge.write_metadata(&metadata)?;
    Ok(updated)
}

pub(crate) fn remove_issue_dependency(
    forge: &FilesystemForge,
    id: &IssueId,
    target: ItemNumber,
) -> ForgeResult<Issue> {
    let repo_id = forge
        .find_issue_repository_by_id(id)?
        .ok_or_else(|| ForgeError::NotFound(format!("issue {id}")))?;
    let mut issues = forge.read_issues_for_existing_repository(&repo_id)?;
    let index = issue_index(&issues, id)?;
    if !issues[index].dependencies.contains(&target) {
        return Ok(issues[index].clone());
    }

    let mut metadata = forge.read_metadata()?;
    let now = next_timestamp(&mut metadata)?;
    issues[index]
        .dependencies
        .retain(|dependency| dependency != &target);
    issues[index].version = issues[index].version.next();
    issues[index].updated_at = now;
    let updated = issues[index].clone();
    sort_issues_by_number(&mut issues);
    forge.write_issues(&repo_id, &issues)?;
    forge.write_metadata(&metadata)?;
    Ok(updated)
}

pub(crate) fn add_pull_request_dependency(
    forge: &FilesystemForge,
    id: &PullRequestId,
    target: ItemNumber,
) -> ForgeResult<PullRequest> {
    let repo_id = forge
        .find_pull_request_repository_by_id(id)?
        .ok_or_else(|| ForgeError::NotFound(format!("pull request {id}")))?;
    let issues = forge.read_issues_for_existing_repository(&repo_id)?;
    let mut pull_requests = forge.read_pull_requests_for_existing_repository(&repo_id)?;
    let index = pull_request_index(&pull_requests, id)?;
    if pull_requests[index].dependencies.contains(&target) {
        return Ok(pull_requests[index].clone());
    }
    ensure_target_exists(&repo_id, &issues, &pull_requests, target)?;

    let mut metadata = forge.read_metadata()?;
    let now = next_timestamp(&mut metadata)?;
    insert_dependency(&mut pull_requests[index].dependencies, target);
    pull_requests[index].version = pull_requests[index].version.next();
    pull_requests[index].updated_at = now;
    let updated = pull_requests[index].clone();
    sort_pull_requests_by_number(&mut pull_requests);
    forge.write_pull_requests(&repo_id, &pull_requests)?;
    forge.write_metadata(&metadata)?;
    Ok(updated)
}

pub(crate) fn remove_pull_request_dependency(
    forge: &FilesystemForge,
    id: &PullRequestId,
    target: ItemNumber,
) -> ForgeResult<PullRequest> {
    let repo_id = forge
        .find_pull_request_repository_by_id(id)?
        .ok_or_else(|| ForgeError::NotFound(format!("pull request {id}")))?;
    let mut pull_requests = forge.read_pull_requests_for_existing_repository(&repo_id)?;
    let index = pull_request_index(&pull_requests, id)?;
    if !pull_requests[index].dependencies.contains(&target) {
        return Ok(pull_requests[index].clone());
    }

    let mut metadata = forge.read_metadata()?;
    let now = next_timestamp(&mut metadata)?;
    pull_requests[index]
        .dependencies
        .retain(|dependency| dependency != &target);
    pull_requests[index].version = pull_requests[index].version.next();
    pull_requests[index].updated_at = now;
    let updated = pull_requests[index].clone();
    sort_pull_requests_by_number(&mut pull_requests);
    forge.write_pull_requests(&repo_id, &pull_requests)?;
    forge.write_metadata(&metadata)?;
    Ok(updated)
}

fn issue_index(issues: &[Issue], id: &IssueId) -> ForgeResult<usize> {
    issues
        .iter()
        .position(|issue| &issue.id == id)
        .ok_or_else(|| ForgeError::NotFound(format!("issue {id}")))
}

fn pull_request_index(pull_requests: &[PullRequest], id: &PullRequestId) -> ForgeResult<usize> {
    pull_requests
        .iter()
        .position(|pull_request| &pull_request.id == id)
        .ok_or_else(|| ForgeError::NotFound(format!("pull request {id}")))
}

fn ensure_target_exists(
    repo_id: &RepositoryId,
    issues: &[Issue],
    pull_requests: &[PullRequest],
    target: ItemNumber,
) -> ForgeResult<()> {
    let issue_exists = issues.iter().any(|issue| issue.number == target);
    let pull_request_exists = pull_requests
        .iter()
        .any(|pull_request| pull_request.number == target);
    if issue_exists || pull_request_exists {
        Ok(())
    } else {
        Err(ForgeError::NotFound(format!(
            "dependency target item {target} in repository {repo_id}"
        )))
    }
}

fn insert_dependency(dependencies: &mut Vec<ItemNumber>, target: ItemNumber) {
    dependencies.push(target);
    dependencies.sort();
    dependencies.dedup();
}
