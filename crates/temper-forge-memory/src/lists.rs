//! Pure ordering, filtering, and mutation helpers for the in-memory backend.
//!
//! These operate only on `temper-forge` domain types and contain no storage
//! concerns. They are intentionally a duplicate of the filesystem backend's
//! equivalent helpers (see ADR 0008): both reference backends must apply the
//! same deterministic ordering, the same conjunctive query semantics, and the
//! same label/assignee/state update rules, so workflow tests can swap backends
//! without changing expectations.

use chrono::{DateTime, Utc};
use std::cmp::Ordering;
use temper_forge::{
    CiJob, CiJobQuery, CiJobSortField, Comment, ForgeError, ForgeResult, Issue, IssueQuery,
    IssueState, ItemSortField, Label, PullRequest, PullRequestQuery, PullRequestReview,
    PullRequestState, PullRequestUpdateState, Repository, RepositoryQuery, RepositorySortField,
    SortDirection, UserId,
};

pub(crate) fn sort_repositories(repositories: &mut [Repository], query: &RepositoryQuery) {
    repositories.sort_by(|left, right| compare_repositories(left, right, query));
}

pub(crate) fn sort_labels(labels: &mut [Label]) {
    labels.sort_by(compare_labels);
}

pub(crate) fn sort_issues(issues: &mut [Issue], query: &IssueQuery) {
    issues.sort_by(|left, right| compare_issues(left, right, query));
}

pub(crate) fn sort_issues_by_number(issues: &mut [Issue]) {
    issues.sort_by(|left, right| {
        compare_issue_number(left, right).then_with(|| left.id.cmp(&right.id))
    });
}

pub(crate) fn sort_pull_requests(pull_requests: &mut [PullRequest], query: &PullRequestQuery) {
    pull_requests.sort_by(|left, right| compare_pull_requests(left, right, query));
}

pub(crate) fn sort_pull_requests_by_number(pull_requests: &mut [PullRequest]) {
    pull_requests.sort_by(|left, right| {
        compare_pull_request_number(left, right).then_with(|| left.id.cmp(&right.id))
    });
}

pub(crate) fn sort_comments(comments: &mut [Comment]) {
    comments.sort_by(compare_comments);
}

pub(crate) fn sort_reviews(reviews: &mut [PullRequestReview]) {
    reviews.sort_by(compare_reviews);
}

pub(crate) fn sort_ci_jobs(ci_jobs: &mut [CiJob], query: &CiJobQuery) {
    ci_jobs.sort_by(|left, right| compare_ci_jobs(left, right, query));
}

fn compare_repositories(
    left: &Repository,
    right: &Repository,
    query: &RepositoryQuery,
) -> Ordering {
    let primary = query
        .sort
        .map(|sort| {
            let comparison = match sort.field {
                RepositorySortField::Path => compare_repository_path(left, right),
                RepositorySortField::CreatedAt => left.created_at.cmp(&right.created_at),
                RepositorySortField::UpdatedAt => left.updated_at.cmp(&right.updated_at),
            };
            apply_direction(comparison, sort.direction)
        })
        .unwrap_or_else(|| compare_repository_path(left, right));

    primary
        .then_with(|| compare_repository_path(left, right))
        .then_with(|| left.id.cmp(&right.id))
}

fn compare_repository_path(left: &Repository, right: &Repository) -> Ordering {
    left.owner
        .cmp(&right.owner)
        .then_with(|| left.name.cmp(&right.name))
}

fn compare_labels(left: &Label, right: &Label) -> Ordering {
    left.name
        .cmp(&right.name)
        .then_with(|| left.id.cmp(&right.id))
}

fn compare_issues(left: &Issue, right: &Issue, query: &IssueQuery) -> Ordering {
    let primary = query
        .sort
        .map(|sort| {
            let comparison = match sort.field {
                ItemSortField::Number => compare_issue_number(left, right),
                ItemSortField::CreatedAt => left.created_at.cmp(&right.created_at),
                ItemSortField::UpdatedAt => left.updated_at.cmp(&right.updated_at),
            };
            apply_direction(comparison, sort.direction)
        })
        .unwrap_or_else(|| compare_issue_number(left, right));

    primary
        .then_with(|| compare_issue_number(left, right))
        .then_with(|| left.id.cmp(&right.id))
}

fn compare_issue_number(left: &Issue, right: &Issue) -> Ordering {
    left.number.cmp(&right.number)
}

fn compare_pull_requests(
    left: &PullRequest,
    right: &PullRequest,
    query: &PullRequestQuery,
) -> Ordering {
    let primary = query
        .sort
        .map(|sort| {
            let comparison = match sort.field {
                ItemSortField::Number => compare_pull_request_number(left, right),
                ItemSortField::CreatedAt => left.created_at.cmp(&right.created_at),
                ItemSortField::UpdatedAt => left.updated_at.cmp(&right.updated_at),
            };
            apply_direction(comparison, sort.direction)
        })
        .unwrap_or_else(|| compare_pull_request_number(left, right));

    primary
        .then_with(|| compare_pull_request_number(left, right))
        .then_with(|| left.id.cmp(&right.id))
}

fn compare_pull_request_number(left: &PullRequest, right: &PullRequest) -> Ordering {
    left.number.cmp(&right.number)
}

fn compare_comments(left: &Comment, right: &Comment) -> Ordering {
    left.created_at
        .cmp(&right.created_at)
        .then_with(|| left.id.cmp(&right.id))
}

fn compare_reviews(left: &PullRequestReview, right: &PullRequestReview) -> Ordering {
    left.submitted_at
        .cmp(&right.submitted_at)
        .then_with(|| left.id.cmp(&right.id))
}

fn compare_ci_jobs(left: &CiJob, right: &CiJob, query: &CiJobQuery) -> Ordering {
    let primary = query
        .sort
        .map(|sort| {
            let comparison = match sort.field {
                CiJobSortField::Name => compare_ci_job_name(left, right),
                CiJobSortField::CreatedAt => left.created_at.cmp(&right.created_at),
                CiJobSortField::UpdatedAt => left.updated_at.cmp(&right.updated_at),
            };
            apply_direction(comparison, sort.direction)
        })
        .unwrap_or_else(|| compare_ci_job_name(left, right));

    primary
        .then_with(|| compare_ci_job_name(left, right))
        .then_with(|| left.id.cmp(&right.id))
}

fn compare_ci_job_name(left: &CiJob, right: &CiJob) -> Ordering {
    left.name.cmp(&right.name)
}

pub(crate) fn issue_matches_query(issue: &Issue, query: &IssueQuery) -> bool {
    if let Some(state) = query.state {
        if issue.state != state {
            return false;
        }
    }
    if !query
        .labels
        .iter()
        .all(|required| issue.labels.iter().any(|label| label == required))
    {
        return false;
    }
    if let Some(author_id) = &query.author_id {
        if &issue.author_id != author_id {
            return false;
        }
    }
    if let Some(assignee_id) = &query.assignee_id {
        if !issue
            .assignees
            .iter()
            .any(|assignee| assignee == assignee_id)
        {
            return false;
        }
    }
    true
}

pub(crate) fn pull_request_matches_query(
    pull_request: &PullRequest,
    query: &PullRequestQuery,
) -> bool {
    if let Some(state) = query.state {
        if pull_request.state != state {
            return false;
        }
    }
    if !query
        .labels
        .iter()
        .all(|required| pull_request.labels.iter().any(|label| label == required))
    {
        return false;
    }
    if let Some(author_id) = &query.author_id {
        if &pull_request.author_id != author_id {
            return false;
        }
    }
    if let Some(assignee_id) = &query.assignee_id {
        if !pull_request
            .assignees
            .iter()
            .any(|assignee| assignee == assignee_id)
        {
            return false;
        }
    }
    true
}

pub(crate) fn ci_job_matches_query(ci_job: &CiJob, query: &CiJobQuery) -> bool {
    if let Some(pull_request_id) = &query.pull_request_id {
        if ci_job.pull_request_id.as_ref() != Some(pull_request_id) {
            return false;
        }
    }
    if let Some(commit_sha) = &query.commit_sha {
        if &ci_job.commit_sha != commit_sha {
            return false;
        }
    }
    if let Some(status) = query.status {
        if ci_job.status != status {
            return false;
        }
    }
    true
}

pub(crate) fn update_issue_state(issue: &mut Issue, state: IssueState, now: DateTime<Utc>) {
    match (issue.state, state) {
        (IssueState::Open, IssueState::Closed) => issue.closed_at = Some(now),
        (IssueState::Closed, IssueState::Open) => issue.closed_at = None,
        (_, IssueState::Open) => issue.closed_at = None,
        (_, IssueState::Closed) if issue.closed_at.is_none() => issue.closed_at = Some(now),
        _ => {}
    }
    issue.state = state;
}

pub(crate) fn update_pull_request_state(
    pull_request: &mut PullRequest,
    state: PullRequestUpdateState,
    now: DateTime<Utc>,
) -> ForgeResult<()> {
    if pull_request.state == PullRequestState::Merged {
        return Err(ForgeError::Conflict(format!(
            "pull request {} is merged",
            pull_request.id
        )));
    }

    let state = PullRequestState::from(state);
    match (pull_request.state, state) {
        (PullRequestState::Open, PullRequestState::Closed) => pull_request.closed_at = Some(now),
        (PullRequestState::Closed, PullRequestState::Open) => pull_request.closed_at = None,
        (_, PullRequestState::Open) => pull_request.closed_at = None,
        (_, PullRequestState::Closed) if pull_request.closed_at.is_none() => {
            pull_request.closed_at = Some(now);
        }
        _ => {}
    }
    pull_request.state = state;
    Ok(())
}

pub(crate) fn apply_label_update(
    labels: &mut Vec<String>,
    set_labels: Option<Vec<String>>,
    remove_labels: Vec<String>,
    add_labels: Vec<String>,
) {
    if let Some(set_labels) = set_labels {
        *labels = normalize_string_set(set_labels);
    } else {
        *labels = normalize_string_set(std::mem::take(labels));
    }

    let remove_labels = normalize_string_set(remove_labels);
    labels.retain(|label| !remove_labels.contains(label));

    for label in add_labels {
        if !labels.contains(&label) {
            labels.push(label);
        }
    }
    labels.sort();
}

pub(crate) fn apply_assignee_update(
    assignees: &mut Vec<UserId>,
    remove_assignees: Vec<UserId>,
    add_assignees: Vec<UserId>,
) {
    *assignees = normalize_user_set(std::mem::take(assignees));

    let remove_assignees = normalize_user_set(remove_assignees);
    assignees.retain(|assignee| !remove_assignees.contains(assignee));

    for assignee in add_assignees {
        if !assignees.contains(&assignee) {
            assignees.push(assignee);
        }
    }
    assignees.sort();
}

pub(crate) fn normalize_string_set(mut values: Vec<String>) -> Vec<String> {
    values.sort();
    values.dedup();
    values
}

pub(crate) fn normalize_user_set(mut values: Vec<UserId>) -> Vec<UserId> {
    values.sort();
    values.dedup();
    values
}

fn apply_direction(comparison: Ordering, direction: SortDirection) -> Ordering {
    match direction {
        SortDirection::Asc => comparison,
        SortDirection::Desc => comparison.reverse(),
    }
}
