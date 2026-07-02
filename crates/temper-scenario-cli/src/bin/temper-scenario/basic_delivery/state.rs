// SPDX-License-Identifier: MPL-2.0

use temper_forge_model::{CiJobConclusion, IssueState, PullRequestState};

pub(super) fn pr_state_evidence(state: PullRequestState) -> &'static str {
    match state {
        PullRequestState::Open => "open (not merged)",
        PullRequestState::Closed => "closed",
        PullRequestState::Merged => "merged",
    }
}

pub(super) fn pr_state_value(state: PullRequestState) -> &'static str {
    match state {
        PullRequestState::Open => "open",
        PullRequestState::Closed => "closed",
        PullRequestState::Merged => "merged",
    }
}

pub(super) fn issue_state_value(state: IssueState) -> &'static str {
    match state {
        IssueState::Open => "open",
        IssueState::Closed => "closed",
    }
}

pub(super) fn issue_state_word(state: IssueState) -> &'static str {
    match state {
        IssueState::Open => "open/in-progress",
        IssueState::Closed => "closed",
    }
}

pub(super) fn ci_job_conclusion_value(conclusion: CiJobConclusion) -> &'static str {
    match conclusion {
        CiJobConclusion::Success => "success",
        CiJobConclusion::Failure => "failure",
        CiJobConclusion::Cancelled => "cancelled",
        CiJobConclusion::Skipped => "skipped",
        CiJobConclusion::TimedOut => "timed_out",
        CiJobConclusion::Neutral => "neutral",
    }
}

pub(super) fn has_label(labels: &[String], expected: &str) -> bool {
    labels.iter().any(|label| label == expected)
}
