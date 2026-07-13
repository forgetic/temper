//! Issue and issue-comment operations for [`MemoryForge`](crate::MemoryForge).

use crate::MemoryForge;
use crate::dependencies::{add_issue_dependency, remove_issue_dependency};
use crate::fault::FaultOp;
use crate::ids::{issue_comment_id, issue_id};
use crate::lists::{
    apply_assignee_update, apply_label_update, issue_matches_query, normalize_string_set,
    normalize_user_set, sort_comments, sort_issues, sort_issues_by_number, update_issue_state,
};
use crate::util::{check_expected_version, next_comment_number, next_item_number};
use temper_forge_model::{
    ChangeKind, Comment, CreateComment, CreateIssue, ForgeError, ForgeResult, Issue, IssueId,
    IssueQuery, IssueState, ItemNumber, RepositoryId, UpdateIssue, Version,
};

pub(crate) fn list_issues(
    forge: &MemoryForge,
    repo_id: &RepositoryId,
    query: IssueQuery,
) -> ForgeResult<Vec<Issue>> {
    let mut inner = forge.lock();
    inner.faults.take(FaultOp::ListIssues)?;
    inner.state.require_repository(repo_id)?;
    let mut issues = inner
        .state
        .issues(repo_id)
        .into_iter()
        .filter(|issue| issue_matches_query(issue, &query))
        .collect::<Vec<_>>();
    if !query.details.dependencies {
        for issue in &mut issues {
            issue.dependencies.clear();
        }
    }
    sort_issues(&mut issues, &query);
    if let Some(limit) = query.limit {
        issues.truncate(limit);
    }
    Ok(issues)
}

pub(crate) fn create_issue(
    forge: &MemoryForge,
    repo_id: &RepositoryId,
    input: CreateIssue,
) -> ForgeResult<Issue> {
    let mut inner = forge.lock();
    inner.faults.take(FaultOp::CreateIssue)?;
    inner.state.require_repository(repo_id)?;
    let now = inner.state.next_timestamp()?;
    let author_id = forge.effective_user(&inner).id;
    let issues = inner.state.issues_mut(repo_id);
    let number = next_item_number(issues.iter().map(|issue| issue.number))?;
    let issue = Issue {
        id: issue_id(repo_id, number),
        repo_id: repo_id.clone(),
        number,
        title: input.title,
        body: input.body,
        state: IssueState::Open,
        author_id,
        labels: normalize_string_set(input.labels),
        assignees: normalize_user_set(input.assignees),
        dependencies: Vec::new(),
        version: Version::INITIAL,
        created_at: now,
        updated_at: now,
        closed_at: None,
    };
    issues.push(issue.clone());
    sort_issues_by_number(issues);
    inner.publish_issue_hint(repo_id, issue.number, ChangeKind::Created);
    Ok(issue)
}

pub(crate) fn get_issue(forge: &MemoryForge, id: &IssueId) -> ForgeResult<Option<Issue>> {
    Ok(forge.lock().state.find_issue(id).map(|(_, issue)| issue))
}

pub(crate) fn get_issue_by_number(
    forge: &MemoryForge,
    repo_id: &RepositoryId,
    number: ItemNumber,
) -> ForgeResult<Option<Issue>> {
    let mut inner = forge.lock();
    inner.faults.take(FaultOp::GetIssueByNumber)?;
    if !inner.state.repository_exists(repo_id) {
        return Ok(None);
    }
    Ok(inner
        .state
        .issues(repo_id)
        .into_iter()
        .find(|issue| issue.number == number))
}

pub(crate) fn update_issue(
    forge: &MemoryForge,
    id: &IssueId,
    input: UpdateIssue,
) -> ForgeResult<Issue> {
    let mut inner = forge.lock();
    inner.faults.take(FaultOp::UpdateIssue)?;
    let (repo_id, existing) = inner
        .state
        .find_issue(id)
        .ok_or_else(|| ForgeError::NotFound(format!("issue {id}")))?;
    check_expected_version("issue", id, input.expected_version, existing.version)?;
    let now = inner.state.next_timestamp()?;
    let issues = inner.state.issues_mut(&repo_id);
    let issue = issues
        .iter_mut()
        .find(|issue| &issue.id == id)
        .ok_or_else(|| ForgeError::NotFound(format!("issue {id}")))?;

    if let Some(title) = input.title {
        issue.title = title;
    }
    if let Some(body) = input.body {
        issue.body = body;
    }
    if let Some(state) = input.state {
        update_issue_state(issue, state, now);
    }
    apply_label_update(
        &mut issue.labels,
        input.set_labels,
        input.remove_labels,
        input.add_labels,
    );
    apply_assignee_update(
        &mut issue.assignees,
        input.remove_assignees,
        input.add_assignees,
    );
    issue.version = issue.version.next();
    issue.updated_at = now;
    let updated = issue.clone();
    sort_issues_by_number(issues);
    inner.publish_issue_hint(&repo_id, updated.number, ChangeKind::Edited);
    Ok(updated)
}

pub(crate) fn add_dependency(
    forge: &MemoryForge,
    id: &IssueId,
    target: ItemNumber,
) -> ForgeResult<Issue> {
    let mut inner = forge.lock();
    let (issue, changed) = add_issue_dependency(&mut inner, id, target)?;
    if changed {
        inner.publish_issue_hint(&issue.repo_id, issue.number, ChangeKind::Dependency);
    }
    Ok(issue)
}

pub(crate) fn remove_dependency(
    forge: &MemoryForge,
    id: &IssueId,
    target: ItemNumber,
) -> ForgeResult<Issue> {
    let mut inner = forge.lock();
    let (issue, changed) = remove_issue_dependency(&mut inner, id, target)?;
    if changed {
        inner.publish_issue_hint(&issue.repo_id, issue.number, ChangeKind::Dependency);
    }
    Ok(issue)
}

pub(crate) fn list_comments(forge: &MemoryForge, id: &IssueId) -> ForgeResult<Vec<Comment>> {
    let inner = forge.lock();
    inner
        .state
        .find_issue(id)
        .ok_or_else(|| ForgeError::NotFound(format!("issue {id}")))?;
    let mut comments = inner.state.issue_comments(id);
    sort_comments(&mut comments);
    Ok(comments)
}

pub(crate) fn add_comment(
    forge: &MemoryForge,
    id: &IssueId,
    input: CreateComment,
) -> ForgeResult<Comment> {
    let mut inner = forge.lock();
    inner
        .state
        .find_issue(id)
        .ok_or_else(|| ForgeError::NotFound(format!("issue {id}")))?;
    let now = inner.state.next_timestamp()?;
    let author_id = forge.effective_user(&inner).id;
    let comments = inner.state.issue_comments_mut(id);
    let number = next_comment_number(comments.len())?;
    let comment = Comment {
        id: issue_comment_id(id, number),
        author_id,
        body: input.body,
        created_at: now,
        updated_at: now,
    };
    comments.push(comment.clone());
    sort_comments(comments);
    if let Some((repo_id, issue)) = inner.state.find_issue(id) {
        inner.publish_issue_hint(&repo_id, issue.number, ChangeKind::Comment);
    }
    Ok(comment)
}
