//! DTOs for GitHub Actions: workflow runs, jobs, and their list envelopes.

use chrono::{DateTime, Utc};
use serde::Deserialize;

/// GitHub Actions workflow run (one element of the `workflow_runs` envelope).
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
pub(crate) struct WorkflowRunDto {
    #[serde(default)]
    pub id: u64,
    #[serde(default)]
    pub run_attempt: u64,
    #[serde(default)]
    pub head_sha: String,
}

/// Envelope wrapping `GET /repos/{owner}/{repo}/actions/runs`.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
pub(crate) struct WorkflowRunsEnvelopeDto {
    #[serde(default)]
    pub workflow_runs: Vec<WorkflowRunDto>,
}

/// GitHub Actions workflow job.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
pub(crate) struct WorkflowJobDto {
    #[serde(default)]
    pub id: u64,
    #[serde(default)]
    pub run_id: u64,
    #[serde(default)]
    pub run_attempt: u64,
    #[serde(default)]
    pub head_sha: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub conclusion: Option<String>,
    /// Some GitHub Enterprise-compatible APIs provide a structured reason even
    /// though github.com usually exposes only `conclusion` for workflow jobs.
    #[serde(default, alias = "failure_reason", alias = "status_reason")]
    pub reason: Option<String>,
    #[serde(default)]
    pub html_url: Option<String>,
    #[serde(default)]
    pub created_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub started_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub completed_at: Option<DateTime<Utc>>,
}

/// Envelope wrapping `GET /repos/{owner}/{repo}/actions/runs/{id}/jobs`.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
pub(crate) struct WorkflowJobsEnvelopeDto {
    #[serde(default)]
    pub jobs: Vec<WorkflowJobDto>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserializes_actions_envelopes() {
        let runs: WorkflowRunsEnvelopeDto = serde_json::from_str(
            r#"{
                "total_count": 1,
                "workflow_runs": [{"id": 30433642, "run_attempt": 3,
                    "head_sha": "abcdef1", "status": "completed"}]
            }"#,
        )
        .unwrap();
        assert_eq!(runs.workflow_runs.len(), 1);
        assert_eq!(runs.workflow_runs[0].id, 30433642);
        assert_eq!(runs.workflow_runs[0].run_attempt, 3);

        let jobs: WorkflowJobsEnvelopeDto = serde_json::from_str(
            r#"{
                "total_count": 1,
                "jobs": [{
                    "id": 399444496,
                    "run_id": 29679449,
                    "run_attempt": 3,
                    "head_sha": "abcdef1",
                    "name": "build",
                    "status": "completed",
                    "conclusion": "startup_failure",
                    "failure_reason": "runner could not start",
                    "html_url": "https://github.com/acme/widgets/runs/399444496",
                    "created_at": "2024-01-02T03:00:00Z",
                    "started_at": "2024-01-02T03:04:05Z",
                    "completed_at": "2024-01-02T03:10:00Z"
                }]
            }"#,
        )
        .unwrap();
        assert_eq!(jobs.jobs.len(), 1);
        assert_eq!(jobs.jobs[0].run_id, 29679449);
        assert_eq!(jobs.jobs[0].run_attempt, 3);
        assert_eq!(jobs.jobs[0].conclusion.as_deref(), Some("startup_failure"));
        assert_eq!(
            jobs.jobs[0].reason.as_deref(),
            Some("runner could not start")
        );
    }
}
