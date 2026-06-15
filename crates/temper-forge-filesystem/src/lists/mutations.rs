use chrono::{DateTime, Utc};
use temper_forge::{
    ForgeError, ForgeResult, Issue, IssueState, PullRequest, PullRequestState,
    PullRequestUpdateState, UserId,
};

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
