use crate::lists::{sort_issues_by_number, sort_pull_requests_by_number};
use crate::state::State;
use crate::Inner;
use temper_forge::{
    ForgeError, ForgeResult, Issue, IssueId, ItemNumber, PullRequest, PullRequestId, RepositoryId,
};

pub(crate) fn add_issue_dependency(
    inner: &mut Inner,
    id: &IssueId,
    target: ItemNumber,
) -> ForgeResult<(Issue, bool)> {
    let (repo_id, existing) = inner
        .state
        .find_issue(id)
        .ok_or_else(|| ForgeError::NotFound(format!("issue {id}")))?;
    if existing.dependencies.contains(&target) {
        return Ok((existing, false));
    }
    ensure_target_exists(&inner.state, &repo_id, target)?;

    let now = inner.state.next_timestamp()?;
    let issues = inner.state.issues_mut(&repo_id);
    let issue = issues
        .iter_mut()
        .find(|issue| &issue.id == id)
        .ok_or_else(|| ForgeError::NotFound(format!("issue {id}")))?;
    insert_dependency(&mut issue.dependencies, target);
    issue.version = issue.version.next();
    issue.updated_at = now;
    let updated = issue.clone();
    sort_issues_by_number(issues);
    Ok((updated, true))
}

pub(crate) fn remove_issue_dependency(
    inner: &mut Inner,
    id: &IssueId,
    target: ItemNumber,
) -> ForgeResult<(Issue, bool)> {
    let (repo_id, existing) = inner
        .state
        .find_issue(id)
        .ok_or_else(|| ForgeError::NotFound(format!("issue {id}")))?;
    if !existing.dependencies.contains(&target) {
        return Ok((existing, false));
    }

    let now = inner.state.next_timestamp()?;
    let issues = inner.state.issues_mut(&repo_id);
    let issue = issues
        .iter_mut()
        .find(|issue| &issue.id == id)
        .ok_or_else(|| ForgeError::NotFound(format!("issue {id}")))?;
    issue
        .dependencies
        .retain(|dependency| dependency != &target);
    issue.version = issue.version.next();
    issue.updated_at = now;
    let updated = issue.clone();
    sort_issues_by_number(issues);
    Ok((updated, true))
}

pub(crate) fn add_pull_request_dependency(
    inner: &mut Inner,
    id: &PullRequestId,
    target: ItemNumber,
) -> ForgeResult<(PullRequest, bool)> {
    let (repo_id, existing) = inner
        .state
        .find_pull_request(id)
        .ok_or_else(|| ForgeError::NotFound(format!("pull request {id}")))?;
    if existing.dependencies.contains(&target) {
        return Ok((existing, false));
    }
    ensure_target_exists(&inner.state, &repo_id, target)?;

    let now = inner.state.next_timestamp()?;
    let pull_requests = inner.state.pull_requests_mut(&repo_id);
    let pull_request = pull_requests
        .iter_mut()
        .find(|pull_request| &pull_request.id == id)
        .ok_or_else(|| ForgeError::NotFound(format!("pull request {id}")))?;
    insert_dependency(&mut pull_request.dependencies, target);
    pull_request.version = pull_request.version.next();
    pull_request.updated_at = now;
    let updated = pull_request.clone();
    sort_pull_requests_by_number(pull_requests);
    Ok((updated, true))
}

pub(crate) fn remove_pull_request_dependency(
    inner: &mut Inner,
    id: &PullRequestId,
    target: ItemNumber,
) -> ForgeResult<(PullRequest, bool)> {
    let (repo_id, existing) = inner
        .state
        .find_pull_request(id)
        .ok_or_else(|| ForgeError::NotFound(format!("pull request {id}")))?;
    if !existing.dependencies.contains(&target) {
        return Ok((existing, false));
    }

    let now = inner.state.next_timestamp()?;
    let pull_requests = inner.state.pull_requests_mut(&repo_id);
    let pull_request = pull_requests
        .iter_mut()
        .find(|pull_request| &pull_request.id == id)
        .ok_or_else(|| ForgeError::NotFound(format!("pull request {id}")))?;
    pull_request
        .dependencies
        .retain(|dependency| dependency != &target);
    pull_request.version = pull_request.version.next();
    pull_request.updated_at = now;
    let updated = pull_request.clone();
    sort_pull_requests_by_number(pull_requests);
    Ok((updated, true))
}

fn ensure_target_exists(
    state: &State,
    repo_id: &RepositoryId,
    target: ItemNumber,
) -> ForgeResult<()> {
    let issue_exists = state
        .issues(repo_id)
        .iter()
        .any(|issue| issue.number == target);
    let pull_request_exists = state
        .pull_requests(repo_id)
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
