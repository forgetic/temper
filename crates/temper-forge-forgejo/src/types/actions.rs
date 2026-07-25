//! Forgejo Actions DTOs for workflow runs and per-run jobs.

use crate::ci_time::deserialize_flexible_opt_datetime;
use chrono::{DateTime, Utc};
use serde::Deserialize;

/// Forgejo Actions workflow run.
///
/// Returned by `GET /repos/{owner}/{repo}/actions/runs` (wrapped under a
/// `workflow_runs` array). Fields are lenient because run payloads vary across
/// Forgejo versions; timestamps may be RFC3339 strings (`*_at`) or unix-epoch
/// integers (`created`/`updated`), so they decode through the tolerant helper.
#[derive(Clone, Debug, Default, Deserialize)]
pub(crate) struct ActionRunDto {
    #[serde(default)]
    pub id: u64,
    #[serde(default)]
    pub index_in_repo: u64,
    #[serde(default)]
    pub run_number: u64,
    #[serde(default, alias = "run_attempt", alias = "attempt_number")]
    #[allow(dead_code)]
    pub attempt: u64,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub conclusion: String,
    #[serde(default, alias = "failure_reason", alias = "status_reason")]
    pub reason: String,
    // Decoded for provider-shape fidelity; matching no longer relies on the
    // event kind because push-based PR CI is a first-class fixture shape.
    #[serde(default)]
    #[allow(dead_code)]
    pub event: String,
    #[serde(default)]
    pub prettyref: String,
    #[serde(default)]
    pub head_branch: String,
    #[serde(default)]
    pub head_sha: String,
    #[serde(default)]
    pub commit_sha: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub html_url: String,
    #[serde(default)]
    pub event_payload: String,
    #[serde(default, deserialize_with = "deserialize_flexible_opt_datetime")]
    pub created_at: Option<DateTime<Utc>>,
    #[serde(default, deserialize_with = "deserialize_flexible_opt_datetime")]
    pub created: Option<DateTime<Utc>>,
    #[serde(default, deserialize_with = "deserialize_flexible_opt_datetime")]
    pub updated_at: Option<DateTime<Utc>>,
    #[serde(default, deserialize_with = "deserialize_flexible_opt_datetime")]
    pub updated: Option<DateTime<Utc>>,
}

/// Forgejo 16 Actions job returned by a provider-run jobs endpoint.
///
/// All six fields are required by the v16 contract. In particular, none of the
/// identity coordinates default to zero: a missing field must fail decoding
/// rather than silently turning response order into identity.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub(crate) struct ActionJobDto {
    pub id: u64,
    pub run_id: u64,
    pub attempt: u64,
    pub task_id: u64,
    pub name: String,
    pub status: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserializes_action_run_with_string_and_integer_timestamps() {
        let run: ActionRunDto = serde_json::from_str(
            r##"{
                "id": 900,
                "index_in_repo": 10,
                "run_number": 10,
                "run_attempt": 2,
                "status": "completed",
                "conclusion": "runner_lost",
                "failure_reason": "runner disconnected",
                "event": "pull_request",
                "prettyref": "#7",
                "head_branch": "feature",
                "head_sha": "abcdef1234567",
                "created_at": "2024-01-02T00:00:00Z",
                "updated": 1700000000,
                "extra": true
            }"##,
        )
        .unwrap();
        assert_eq!(run.id, 900);
        assert_eq!(run.index_in_repo, 10);
        assert_eq!(run.attempt, 2);
        assert_eq!(run.conclusion, "runner_lost");
        assert_eq!(run.reason, "runner disconnected");
        assert_eq!(run.prettyref, "#7");
        assert!(run.created_at.is_some());
        assert!(run.updated.is_some());
        assert_eq!(run.created, None);
    }

    #[test]
    fn per_run_job_requires_the_v16_identity_shape() {
        let response: Vec<ActionJobDto> = serde_json::from_str(
            r#"[{"id":31,"run_id":900,"attempt":2,"task_id":44,
                "name":"build","status":"success"}]"#,
        )
        .unwrap();
        assert_eq!(response[0].id, 31);
        assert_eq!(response[0].run_id, 900);
        assert_eq!(response[0].attempt, 2);
        assert_eq!(response[0].task_id, 44);

        assert!(serde_json::from_str::<Vec<ActionJobDto>>(r#"{}"#).is_err());
        assert!(serde_json::from_str::<Vec<ActionJobDto>>(r#"{"jobs":[]}"#).is_err());
        assert!(
            serde_json::from_str::<Vec<ActionJobDto>>(
                r#"[{"id":31,"run_id":900,"attempt":2,"name":"build","status":"success"}]"#
            )
            .is_err()
        );
    }
}
