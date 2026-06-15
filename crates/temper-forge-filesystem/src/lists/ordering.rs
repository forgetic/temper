use std::cmp::Ordering;
use temper_forge::{
    CiJob, CiJobQuery, CiJobSortField, Comment, Issue, IssueQuery, ItemSortField, Label,
    PullRequest, PullRequestQuery, PullRequestReview, Repository, RepositoryQuery,
    RepositorySortField, SortDirection,
};

pub(crate) fn sort_repositories(repositories: &mut [Repository], query: RepositoryQuery) {
    repositories.sort_by(|left, right| compare_repositories(left, right, &query));
}

pub(crate) fn sort_ci_jobs(ci_jobs: &mut [CiJob], query: &CiJobQuery) {
    ci_jobs.sort_by(|left, right| compare_ci_jobs(left, right, query));
}

pub(crate) fn sort_ci_jobs_by_name(ci_jobs: &mut [CiJob]) {
    ci_jobs.sort_by(|left, right| {
        compare_ci_job_name(left, right).then_with(|| left.id.cmp(&right.id))
    });
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

pub(crate) fn compare_ci_jobs(left: &CiJob, right: &CiJob, query: &CiJobQuery) -> Ordering {
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

pub(crate) fn compare_ci_job_name(left: &CiJob, right: &CiJob) -> Ordering {
    left.name.cmp(&right.name)
}

pub(crate) fn compare_labels(left: &Label, right: &Label) -> Ordering {
    left.name
        .cmp(&right.name)
        .then_with(|| left.id.cmp(&right.id))
}

pub(crate) fn compare_issues(left: &Issue, right: &Issue, query: &IssueQuery) -> Ordering {
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

pub(crate) fn compare_issue_number(left: &Issue, right: &Issue) -> Ordering {
    left.number.cmp(&right.number)
}

pub(crate) fn compare_pull_requests(
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

pub(crate) fn compare_pull_request_number(left: &PullRequest, right: &PullRequest) -> Ordering {
    left.number.cmp(&right.number)
}

pub(crate) fn compare_comments(left: &Comment, right: &Comment) -> Ordering {
    left.created_at
        .cmp(&right.created_at)
        .then_with(|| left.id.cmp(&right.id))
}

pub(crate) fn compare_reviews(left: &PullRequestReview, right: &PullRequestReview) -> Ordering {
    left.submitted_at
        .cmp(&right.submitted_at)
        .then_with(|| left.id.cmp(&right.id))
}

pub(crate) fn compare_repositories(
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

pub(crate) fn compare_repository_path(left: &Repository, right: &Repository) -> Ordering {
    left.owner
        .cmp(&right.owner)
        .then_with(|| left.name.cmp(&right.name))
}

pub(crate) fn apply_direction(comparison: Ordering, direction: SortDirection) -> Ordering {
    match direction {
        SortDirection::Asc => comparison,
        SortDirection::Desc => comparison.reverse(),
    }
}
