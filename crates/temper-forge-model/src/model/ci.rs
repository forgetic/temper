use crate::ids::{CiJobId, PullRequestId, RepositoryId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Maximum UTF-8 byte length retained for a provider-supplied CI conclusion or reason.
pub const MAX_CI_PROVIDER_EVIDENCE_BYTES: usize = 256;

/// Execution status for a CI job.
#[derive(Copy, Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CiJobStatus {
    Queued,
    Running,
    Completed,
}

/// Typed terminal category for a completed CI job.
///
/// `Failure` is reserved for an ordinary job/test failure. Categories that do
/// not establish a source defect remain distinct so workflow routing does not
/// have to infer their meaning from provider-specific strings.
#[derive(Copy, Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CiJobConclusion {
    Success,
    Failure,
    Cancelled,
    Interrupted,
    TimedOut,
    RunnerLost,
    StartupFailure,
    ActionRequired,
    Neutral,
    Skipped,
    Unknown,
}

/// Bounds and sanitizes provider-supplied CI terminal evidence.
///
/// Leading/trailing whitespace is removed, control characters are replaced by
/// spaces, and the value is truncated at a UTF-8 boundary. Empty evidence is
/// omitted. Providers should retain the original printable spelling and case;
/// normalized strings are used only for typed-category mapping.
pub fn sanitize_ci_provider_evidence(value: &str) -> Option<String> {
    let mut sanitized = String::with_capacity(value.len().min(MAX_CI_PROVIDER_EVIDENCE_BYTES));
    for character in value.trim().chars() {
        let character = if character.is_control() {
            ' '
        } else {
            character
        };
        if sanitized.len() + character.len_utf8() > MAX_CI_PROVIDER_EVIDENCE_BYTES {
            break;
        }
        sanitized.push(character);
    }
    let sanitized = sanitized.trim();
    (!sanitized.is_empty()).then(|| sanitized.to_string())
}

/// CI job associated with a commit and optionally a pull request.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CiJob {
    pub id: CiJobId,
    pub repo_id: RepositoryId,
    pub pull_request_id: Option<PullRequestId>,
    pub commit_sha: String,
    pub name: String,
    pub status: CiJobStatus,
    pub conclusion: Option<CiJobConclusion>,
    /// Sanitized provider conclusion/status spelling for terminal diagnostics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_conclusion: Option<String>,
    /// Sanitized provider-supplied terminal reason, when the API exposes one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_reason: Option<String>,
    /// Opaque, repository-scoped provider workflow-run identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    /// Opaque provider attempt identity within `run_id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt: Option<String>,
    pub url: Option<String>,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn provider_evidence_is_control_free_and_utf8_bounded() {
        let value = format!("  startup\nfailure\0{}é  ", "x".repeat(300));
        let sanitized = sanitize_ci_provider_evidence(&value).unwrap();
        assert!(sanitized.starts_with("startup failure "));
        assert!(sanitized.len() <= MAX_CI_PROVIDER_EVIDENCE_BYTES);
        assert!(!sanitized.chars().any(char::is_control));
        assert!(sanitized.is_char_boundary(sanitized.len()));
    }

    #[test]
    fn legacy_ci_job_json_defaults_terminal_evidence_and_identity() {
        let legacy = json!({
            "id": "ci-1",
            "repo_id": "repo-1",
            "pull_request_id": null,
            "commit_sha": "abc123",
            "name": "test",
            "status": "completed",
            "conclusion": "failure",
            "url": null,
            "created_at": "2026-01-01T00:00:00Z",
            "started_at": null,
            "completed_at": "2026-01-01T00:01:00Z",
            "updated_at": "2026-01-01T00:01:00Z"
        });
        let job: CiJob = serde_json::from_value(legacy).unwrap();
        assert_eq!(job.provider_conclusion, None);
        assert_eq!(job.provider_reason, None);
        assert_eq!(job.run_id, None);
        assert_eq!(job.attempt, None);
    }

    #[test]
    fn typed_terminal_evidence_round_trips() {
        let value = json!({
            "id": "ci-2",
            "repo_id": "repo-1",
            "pull_request_id": null,
            "commit_sha": "def456",
            "name": "test",
            "status": "completed",
            "conclusion": "runner_lost",
            "provider_conclusion": "runner_lost",
            "provider_reason": "runner disconnected",
            "run_id": "run-42",
            "attempt": "3",
            "url": null,
            "created_at": "2026-01-01T00:00:00Z",
            "started_at": null,
            "completed_at": "2026-01-01T00:01:00Z",
            "updated_at": "2026-01-01T00:01:00Z"
        });
        let job: CiJob = serde_json::from_value(value).unwrap();
        assert_eq!(job.conclusion, Some(CiJobConclusion::RunnerLost));
        assert_eq!(serde_json::to_value(job).unwrap()["attempt"], "3");
    }
}
