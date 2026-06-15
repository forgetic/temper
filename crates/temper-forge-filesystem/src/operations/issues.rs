use crate::FilesystemForge;
use crate::dependencies::{add_issue_dependency, remove_issue_dependency};
use crate::lists::{
    apply_assignee_update, apply_label_update, issue_matches_query, next_issue_comment_number,
    next_issue_number, normalize_string_set, normalize_user_set, sort_comments, sort_issues,
    sort_issues_by_number, update_issue_state,
};
use crate::metadata::next_timestamp;
use crate::record_ids::{issue_comment_id, issue_id};
use crate::validation::check_expected_version;
use temper_forge::{
    ChangeKind, Comment, CreateComment, CreateIssue, ForgeError, ForgeResult, Issue, IssueId,
    IssueQuery, IssueState, ItemNumber, RepositoryId, UpdateIssue, Version,
};

pub(crate) fn list_issues(
    forge: &FilesystemForge,
    repo_id: &RepositoryId,
    query: IssueQuery,
) -> ForgeResult<Vec<Issue>> {
    forge.require_repository(repo_id)?;

    let mut issues = forge
        .read_issues_for_existing_repository(repo_id)?
        .into_iter()
        .filter(|issue| issue_matches_query(issue, &query))
        .collect::<Vec<_>>();
    if !query.details.dependencies {
        for issue in &mut issues {
            issue.dependencies.clear();
        }
    }
    sort_issues(&mut issues, &query);
    Ok(issues)
}

pub(crate) fn create_issue(
    forge: &FilesystemForge,
    repo_id: &RepositoryId,
    input: CreateIssue,
) -> ForgeResult<Issue> {
    let _guard = forge.write_lock()?;
    forge.require_repository(repo_id)?;

    let mut metadata = forge.read_metadata()?;
    let author_id = forge.effective_user(&metadata).id;
    let mut issues = forge.read_issues_for_existing_repository(repo_id)?;
    let number = next_issue_number(repo_id, &issues)?;
    let now = next_timestamp(&mut metadata)?;
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
    sort_issues_by_number(&mut issues);
    forge.write_issues(repo_id, &issues)?;
    forge.write_metadata(&metadata)?;
    forge.publish_item_hint(repo_id, issue.number, ChangeKind::Issue);

    Ok(issue)
}

pub(crate) fn get_issue_by_number(
    forge: &FilesystemForge,
    repo_id: &RepositoryId,
    number: ItemNumber,
) -> ForgeResult<Option<Issue>> {
    if forge.find_repository_by_id(repo_id)?.is_none() {
        return Ok(None);
    }

    Ok(forge
        .read_issues_for_existing_repository(repo_id)?
        .into_iter()
        .find(|issue| issue.number == number))
}

pub(crate) fn update_issue(
    forge: &FilesystemForge,
    id: &IssueId,
    input: UpdateIssue,
) -> ForgeResult<Issue> {
    let _guard = forge.write_lock()?;
    let repo_id = forge
        .find_issue_repository_by_id(id)?
        .ok_or_else(|| ForgeError::NotFound(format!("issue {id}")))?;

    let mut issues = forge.read_issues_for_existing_repository(&repo_id)?;
    let issue = issues
        .iter_mut()
        .find(|issue| &issue.id == id)
        .ok_or_else(|| ForgeError::NotFound(format!("issue {id}")))?;
    check_expected_version("issue", id, input.expected_version, issue.version)?;

    let mut metadata = forge.read_metadata()?;
    let now = next_timestamp(&mut metadata)?;

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
    sort_issues_by_number(&mut issues);
    forge.write_issues(&repo_id, &issues)?;
    forge.write_metadata(&metadata)?;
    forge.publish_item_hint(&repo_id, updated.number, ChangeKind::Issue);

    Ok(updated)
}

pub(crate) fn add_issue_dependency_op(
    forge: &FilesystemForge,
    id: &IssueId,
    target: ItemNumber,
) -> ForgeResult<Issue> {
    let _guard = forge.write_lock()?;
    let issue = add_issue_dependency(forge, id, target)?;
    forge.publish_item_hint(&issue.repo_id, issue.number, ChangeKind::Issue);
    Ok(issue)
}

pub(crate) fn remove_issue_dependency_op(
    forge: &FilesystemForge,
    id: &IssueId,
    target: ItemNumber,
) -> ForgeResult<Issue> {
    let _guard = forge.write_lock()?;
    let issue = remove_issue_dependency(forge, id, target)?;
    forge.publish_item_hint(&issue.repo_id, issue.number, ChangeKind::Issue);
    Ok(issue)
}

pub(crate) fn list_issue_comments(
    forge: &FilesystemForge,
    id: &IssueId,
) -> ForgeResult<Vec<Comment>> {
    let repo_id = forge
        .find_issue_repository_by_id(id)?
        .ok_or_else(|| ForgeError::NotFound(format!("issue {id}")))?;

    forge.read_issue_comments_for_existing_issue(&repo_id, id)
}

pub(crate) fn add_issue_comment(
    forge: &FilesystemForge,
    id: &IssueId,
    input: CreateComment,
) -> ForgeResult<Comment> {
    let _guard = forge.write_lock()?;
    let repo_id = forge
        .find_issue_repository_by_id(id)?
        .ok_or_else(|| ForgeError::NotFound(format!("issue {id}")))?;

    let mut comments = forge.read_issue_comments_for_existing_issue(&repo_id, id)?;
    let comment_number = next_issue_comment_number(id, &comments)?;
    let mut metadata = forge.read_metadata()?;
    let author_id = forge.effective_user(&metadata).id;
    let now = next_timestamp(&mut metadata)?;
    let comment = Comment {
        id: issue_comment_id(id, comment_number),
        author_id,
        body: input.body,
        created_at: now,
        updated_at: now,
    };

    comments.push(comment.clone());
    sort_comments(&mut comments);
    forge.write_issue_comments(&repo_id, id, &comments)?;
    forge.write_metadata(&metadata)?;
    if let Some(issue) = forge.find_issue_by_id(id)? {
        forge.publish_item_hint(&repo_id, issue.number, ChangeKind::Comment);
    }

    Ok(comment)
}
