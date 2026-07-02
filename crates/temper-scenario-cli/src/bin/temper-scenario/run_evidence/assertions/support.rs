// SPDX-License-Identifier: MPL-2.0

use toml::Value;

use super::super::model::{
    ASSERTION_STATUS_FAILED, ASSERTION_STATUS_PASSED, ASSERTION_STATUS_UNSUPPORTED,
    AssertionResultEvidence, IssueStateEvidence, PullRequestStateEvidence,
};

pub(super) struct SelectedIssue<'a> {
    pub(super) issue: &'a IssueStateEvidence,
    pub(super) note: Option<String>,
}

pub(super) struct SelectedPullRequest<'a> {
    pub(super) pull_request: &'a PullRequestStateEvidence,
    pub(super) note: Option<String>,
}

pub(super) enum SelectionProblem {
    Failed(String),
    Unsupported(String),
}

pub(super) fn select_issue<'a>(
    issues: &'a [IssueStateEvidence],
    id: Option<&str>,
) -> Result<SelectedIssue<'a>, SelectionProblem> {
    if issues.is_empty() {
        return Err(SelectionProblem::Unsupported(
            "run evidence has no final issue facts".to_string(),
        ));
    }
    if let Some(id) = id {
        let matches = issues
            .iter()
            .filter(|issue| issue.id.as_deref() == Some(id))
            .collect::<Vec<_>>();
        return match matches.as_slice() {
            [issue] => Ok(SelectedIssue { issue, note: None }),
            [] if issues.iter().all(|issue| issue.id.is_none()) && issues.len() == 1 => {
                Ok(SelectedIssue {
                    issue: &issues[0],
                    note: Some(format!(
                        "matched sole issue #{} because run evidence has no issue ids",
                        issues[0].number
                    )),
                })
            }
            [] if issues.iter().all(|issue| issue.id.is_none()) => {
                Err(SelectionProblem::Unsupported(format!(
                    "cannot resolve issue artifact `issue:{id}` because run evidence has multiple issues and no issue ids"
                )))
            }
            [] => Err(SelectionProblem::Failed(format!(
                "expected issue artifact `issue:{id}` was not present; observed issue ids {:?}",
                issues
                    .iter()
                    .filter_map(|issue| issue.id.as_deref())
                    .collect::<Vec<_>>()
            ))),
            _ => Err(SelectionProblem::Failed(format!(
                "issue artifact id `issue:{id}` matched multiple issues"
            ))),
        };
    }
    match issues {
        [issue] => Ok(SelectedIssue { issue, note: None }),
        _ => Err(SelectionProblem::Unsupported(
            "pulling an issue assertion without an issue id is ambiguous because multiple issues are present"
                .to_string(),
        )),
    }
}

pub(super) fn select_pull_request<'a>(
    pull_requests: &'a [PullRequestStateEvidence],
    id: Option<&str>,
) -> Result<SelectedPullRequest<'a>, SelectionProblem> {
    if pull_requests.is_empty() {
        return Err(SelectionProblem::Unsupported(
            "run evidence has no final pull request facts".to_string(),
        ));
    }
    if let Some(id) = id {
        let matches = pull_requests
            .iter()
            .filter(|pull_request| pull_request.id.as_deref() == Some(id))
            .collect::<Vec<_>>();
        return match matches.as_slice() {
            [pull_request] => Ok(SelectedPullRequest {
                pull_request,
                note: None,
            }),
            [] if pull_requests
                .iter()
                .all(|pull_request| pull_request.id.is_none())
                && pull_requests.len() == 1 =>
            {
                Ok(SelectedPullRequest {
                    pull_request: &pull_requests[0],
                    note: Some(format!(
                        "matched sole pull request #{} because run evidence has no pull request ids",
                        pull_requests[0].number
                    )),
                })
            }
            [] if pull_requests
                .iter()
                .all(|pull_request| pull_request.id.is_none()) =>
            {
                Err(SelectionProblem::Unsupported(format!(
                    "cannot resolve pull request artifact `pull_request:{id}` because run evidence has multiple pull requests and no pull request ids"
                )))
            }
            [] => Err(SelectionProblem::Failed(format!(
                "expected pull request artifact `pull_request:{id}` was not present; observed pull request ids {:?}",
                pull_requests
                    .iter()
                    .filter_map(|pull_request| pull_request.id.as_deref())
                    .collect::<Vec<_>>()
            ))),
            _ => Err(SelectionProblem::Failed(format!(
                "pull request artifact id `pull_request:{id}` matched multiple pull requests"
            ))),
        };
    }
    match pull_requests {
        [pull_request] => Ok(SelectedPullRequest {
            pull_request,
            note: None,
        }),
        _ => Err(SelectionProblem::Unsupported(
            "pull request assertion without a pull request id is ambiguous because multiple pull requests are present"
                .to_string(),
        )),
    }
}

pub(super) enum ArtifactSelector {
    Issue(Option<String>),
    PullRequest(Option<String>),
    Repo,
    Unknown(String),
}

impl ArtifactSelector {
    pub(super) fn parse(value: &str) -> Self {
        let trimmed = value.trim();
        let (kind, id) = trimmed
            .split_once(':')
            .map_or((trimmed, None), |(kind, id)| (kind, Some(id.to_string())));
        match kind {
            "issue" => Self::Issue(id),
            "pull_request" | "pr" => Self::PullRequest(id),
            "repo" | "repository" => Self::Repo,
            other => Self::Unknown(other.to_string()),
        }
    }
}

pub(super) struct ResultBuilder {
    id: String,
    description: String,
    artifact: Option<String>,
    details: Vec<String>,
    passed: usize,
    failed: usize,
    unsupported: usize,
}

impl ResultBuilder {
    pub(super) fn new(id: String, description: String, artifact: Option<String>) -> Self {
        Self {
            id,
            description,
            artifact,
            details: Vec::new(),
            passed: 0,
            failed: 0,
            unsupported: 0,
        }
    }

    pub(super) fn passed(mut self, detail: impl Into<String>) -> Self {
        self.passed += 1;
        self.details.push(detail.into());
        self
    }

    pub(super) fn failed(mut self, detail: impl Into<String>) -> Self {
        self.failed += 1;
        self.details.push(detail.into());
        self
    }

    pub(super) fn unsupported(mut self, detail: impl Into<String>) -> Self {
        self.unsupported += 1;
        self.details.push(detail.into());
        self
    }

    pub(super) fn build(self) -> AssertionResultEvidence {
        let status = if self.failed > 0 {
            ASSERTION_STATUS_FAILED
        } else if self.unsupported > 0 || self.passed == 0 {
            ASSERTION_STATUS_UNSUPPORTED
        } else {
            ASSERTION_STATUS_PASSED
        };
        AssertionResultEvidence {
            id: self.id,
            status: status.to_string(),
            description: self.description,
            artifact: self.artifact,
            kind: None,
            phase: None,
            command: None,
            cwd: None,
            context_path: None,
            stdout_path: None,
            stderr_path: None,
            status_path: None,
            exit_status: None,
            timeout_ms: None,
            duration_ms: None,
            details: self.details,
        }
    }
}

pub(super) fn nonnegative_integer(value: &Value) -> Result<u64, String> {
    value
        .as_integer()
        .filter(|value| *value >= 0)
        .map(|value| value as u64)
        .ok_or_else(|| "count expectation must be a non-negative integer".to_string())
}

pub(super) fn string_array(value: &Value, field: &str) -> Result<Vec<String>, String> {
    let Some(items) = value.as_array() else {
        return Err(format!("{field} must be an array of strings"));
    };
    items
        .iter()
        .map(|item| {
            item.as_str()
                .map(str::to_string)
                .ok_or_else(|| format!("{field} must be an array of strings"))
        })
        .collect()
}

pub(super) fn has_label(labels: &[String], expected: &str) -> bool {
    labels.iter().any(|label| label == expected)
}

pub(super) fn state_is(actual: Option<&str>, expected: &str) -> bool {
    actual.is_some_and(|actual| same_normalized(actual, expected))
}

pub(super) fn same_normalized(actual: &str, expected: &str) -> bool {
    actual.trim().eq_ignore_ascii_case(expected.trim())
}

pub(super) fn ci_conclusion_passed(conclusion: &str) -> bool {
    matches!(
        conclusion.trim().to_ascii_lowercase().as_str(),
        "success" | "successful" | "passed" | "pass"
    )
}

pub(super) fn ci_conclusion_failed(conclusion: &str) -> bool {
    matches!(
        conclusion.trim().to_ascii_lowercase().as_str(),
        "failure"
            | "failed"
            | "fail"
            | "error"
            | "cancelled"
            | "canceled"
            | "timed_out"
            | "timed-out"
    )
}
