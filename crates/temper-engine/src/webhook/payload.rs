// SPDX-License-Identifier: MPL-2.0

//! Provider payload shape helpers for Forgejo/Gitea/GitHub webhook intake.

use serde_json::Value;
use temper_forge::{HintArtifactKind, HintTarget, ItemNumber, RepositoryPath};

use super::WebhookError;

pub(super) fn parse_repo(value: &Value) -> Result<RepositoryPath, WebhookError> {
    parse_repo_object(value.pointer("/repository"))
        .or_else(|| parse_repo_object(value.pointer("/run/repository")))
        .ok_or_else(|| WebhookError::BadPayload("payload has no repository owner/name".into()))
}

fn parse_repo_object(value: Option<&Value>) -> Option<RepositoryPath> {
    let repo = value?;
    if let Some(full) = repo.pointer("/full_name").and_then(Value::as_str) {
        if let Some((owner, name)) = full.split_once('/') {
            return Some(RepositoryPath::new(owner, name));
        }
    }

    let owner = repo
        .pointer("/owner/login")
        .or_else(|| repo.pointer("/owner/username"))
        .and_then(Value::as_str);
    let name = repo.pointer("/name").and_then(Value::as_str);
    match (owner, name) {
        (Some(owner), Some(name)) => Some(RepositoryPath::new(owner, name)),
        _ => None,
    }
}

/// Derives an artifact address only when the event family and payload shape
/// unambiguously establish the item's namespace.
pub(super) fn parse_target(value: &Value, event: &str) -> HintTarget {
    let issue = issue_number(value);
    let pull_request = pull_request_number(value);

    let target = if is_review_event(event) {
        pull_request.map(|number| (HintArtifactKind::PullRequest, number))
    } else if is_ci_event(event) {
        ci_pr_number(value).map(|number| (HintArtifactKind::PullRequest, number))
    } else {
        match event {
            "issues" | "issue" | "issue_dependency" | "issue_dependencies" => {
                issue.map(|number| (HintArtifactKind::Issue, number))
            }
            "pull_request"
            | "pull_request_sync"
            | "pull_request_dependency"
            | "pull_request_dependencies" => {
                pull_request.map(|number| (HintArtifactKind::PullRequest, number))
            }
            // Forgejo may put the shared item number under `issue` for this
            // explicitly PR-scoped event family.
            "pull_request_comment" => pull_request
                .or(issue)
                .map(|number| (HintArtifactKind::PullRequest, number)),
            "issue_comment" => comment_target(value, issue, pull_request),
            "comment" if issue.is_some() && pull_request.is_some() => None,
            "comment" => comment_target(value, issue, pull_request),
            _ => None,
        }
    };

    target
        .map(|(kind, number)| HintTarget::Artifact {
            kind,
            number: ItemNumber::new(number),
        })
        .unwrap_or(HintTarget::Repository)
}

fn comment_target(
    value: &Value,
    issue: Option<u64>,
    pull_request: Option<u64>,
) -> Option<(HintArtifactKind, u64)> {
    if let Some(number) = pull_request {
        return Some((HintArtifactKind::PullRequest, number));
    }
    let number = issue?;
    let kind = if issue_has_pull_request_marker(value) {
        HintArtifactKind::PullRequest
    } else {
        HintArtifactKind::Issue
    };
    Some((kind, number))
}

fn issue_has_pull_request_marker(value: &Value) -> bool {
    value
        .pointer("/issue/pull_request")
        .is_some_and(|marker| !marker.is_null())
}

fn pull_request_number(value: &Value) -> Option<u64> {
    value.pointer("/pull_request/number").and_then(json_u64)
}

fn issue_number(value: &Value) -> Option<u64> {
    value.pointer("/issue/number").and_then(json_u64)
}

pub(super) fn is_review_event(event: &str) -> bool {
    matches!(
        event,
        "pull_request_review"
            | "pull_request_review_approved"
            | "pull_request_review_rejected"
            | "pull_request_review_comment"
            | "pull_request_approved"
            | "pull_request_rejected"
            | "review"
    )
}

pub(super) fn is_ci_event(event: &str) -> bool {
    matches!(
        event,
        "status"
            | "check_run"
            | "workflow_run"
            | "workflow_job"
            | "action_run_failure"
            | "action_run_recover"
            | "action_run_success"
    )
}

pub(super) fn ci_pr_number(value: &Value) -> Option<u64> {
    pull_request_number(value)
        .or_else(|| {
            value
                .pointer("/workflow_run/pull_requests/0/number")
                .and_then(json_u64)
        })
        .or_else(|| {
            value
                .pointer("/check_run/pull_requests/0/number")
                .and_then(json_u64)
        })
        .or_else(|| action_run_payload_pr_number(value))
}

pub(super) fn action_run_payload_pr_number(value: &Value) -> Option<u64> {
    action_run_event_payload(value)
        .as_ref()
        .and_then(|payload| payload.pointer("/pull_request/number"))
        .and_then(json_u64)
}

fn action_run_event_payload(value: &Value) -> Option<Value> {
    let payload = value.pointer("/run/event_payload")?;
    match payload {
        Value::String(raw) if !raw.trim().is_empty() => serde_json::from_str(raw).ok(),
        Value::Object(_) => Some(payload.clone()),
        _ => None,
    }
}

pub(super) fn json_u64(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(|raw| raw.parse().ok()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_run_requires_an_explicit_pull_request_number() {
        let explicit = serde_json::json!({
            "run": {
                "repository": { "full_name": "ai/temper" },
                "event_payload": { "pull_request": { "number": 23 } }
            }
        });
        let ambiguous = serde_json::json!({
            "run": {
                "repository": { "full_name": "ai/temper" },
                "event_payload": { "number": 24 }
            }
        });

        assert_eq!(ci_pr_number(&explicit), Some(23));
        assert_eq!(ci_pr_number(&ambiguous), None);
        assert_eq!(
            parse_target(&ambiguous, "action_run_success"),
            HintTarget::Repository
        );
    }

    #[test]
    fn comments_use_event_family_and_pull_request_marker() {
        let issue = serde_json::json!({ "issue": { "number": 7 } });
        let pr = serde_json::json!({
            "issue": { "number": 8, "pull_request": { "url": "pr" } }
        });

        assert_eq!(
            parse_target(&issue, "issue_comment"),
            HintTarget::Artifact {
                kind: HintArtifactKind::Issue,
                number: ItemNumber::new(7)
            }
        );
        assert_eq!(
            parse_target(&issue, "pull_request_comment"),
            HintTarget::Artifact {
                kind: HintArtifactKind::PullRequest,
                number: ItemNumber::new(7)
            }
        );
        assert_eq!(
            parse_target(&pr, "issue_comment"),
            HintTarget::Artifact {
                kind: HintArtifactKind::PullRequest,
                number: ItemNumber::new(8)
            }
        );
    }

    #[test]
    fn conflicting_issue_and_pr_numbers_are_ambiguous() {
        let value = serde_json::json!({
            "issue": { "number": 7 },
            "pull_request": { "number": 8 }
        });
        assert_eq!(parse_target(&value, "comment"), HintTarget::Repository);
    }
}
