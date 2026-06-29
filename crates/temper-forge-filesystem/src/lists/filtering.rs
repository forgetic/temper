use temper_forge_model::{CiJob, CiJobQuery, Issue, IssueQuery, PullRequest, PullRequestQuery};

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

    if let Some(needle) = &query.body_contains {
        if !needle.is_empty() && !issue.body.contains(needle) {
            return false;
        }
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

    if let Some(needle) = &query.body_contains {
        if !needle.is_empty() && !pull_request.body.contains(needle) {
            return false;
        }
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
