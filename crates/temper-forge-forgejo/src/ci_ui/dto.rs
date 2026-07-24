//! Live-view JSON DTOs returned by the attempt-qualified Forgejo 15 route
//! (`POST …/runs/{run}/jobs/{job}/attempt/1`) and Forgejo 7's unqualified route.
//!
//! These are version-sensitive web-UI shapes (ADR 0019): every field defaults,
//! so a missing field tolerates rather than failing the read.

use serde::Deserialize;

/// Live-view JSON returned by either supported web-UI route shape.
#[derive(Debug, Default, Deserialize)]
pub(super) struct LiveViewDto {
    #[serde(default)]
    pub(super) state: LiveStateDto,
}

#[derive(Debug, Default, Deserialize)]
pub(super) struct LiveStateDto {
    #[serde(default)]
    pub(super) run: LiveRunDto,
}

#[derive(Debug, Default, Deserialize)]
pub(super) struct LiveRunDto {
    #[serde(default)]
    pub(super) status: String,
    #[serde(default)]
    pub(super) conclusion: String,
    #[serde(default, alias = "failureReason", alias = "failure_reason")]
    pub(super) reason: String,
    #[serde(default, alias = "runAttempt", alias = "run_attempt")]
    pub(super) attempt: u64,
    #[serde(default)]
    pub(super) jobs: Vec<LiveJobDto>,
    #[serde(default)]
    pub(super) commit: LiveCommitDto,
}

#[derive(Debug, Default, Deserialize)]
pub(super) struct LiveJobDto {
    #[serde(default)]
    pub(super) name: String,
    #[serde(default)]
    pub(super) status: String,
    #[serde(default)]
    pub(super) conclusion: String,
    #[serde(default, alias = "failureReason", alias = "failure_reason")]
    pub(super) reason: String,
}

#[derive(Debug, Default, Deserialize)]
pub(super) struct LiveCommitDto {
    #[serde(default, rename = "shortSHA")]
    pub(super) short_sha: String,
    #[serde(default)]
    pub(super) branch: LiveBranchDto,
}

#[derive(Debug, Default, Deserialize)]
pub(super) struct LiveBranchDto {
    #[serde(default)]
    pub(super) name: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_view_decodes_nested_state() {
        let dto: LiveViewDto = serde_json::from_str(
            r#"{"state":{"run":{"status":"completed","conclusion":"failure","runAttempt":2,
               "jobs":[{"name":"build","status":"completed","conclusion":"failure",
               "failureReason":"exit 1"}],"commit":{"shortSHA":"abc1234"}}},"logs":{}}"#,
        )
        .unwrap();
        assert_eq!(dto.state.run.jobs.len(), 1);
        assert_eq!(dto.state.run.attempt, 2);
        assert_eq!(dto.state.run.jobs[0].reason, "exit 1");
        assert_eq!(dto.state.run.commit.short_sha, "abc1234");
    }
}
