//! Live-view JSON DTOs returned by `POST …/runs/{run}/jobs/{job}`.
//!
//! These are version-sensitive web-UI shapes (ADR 0019): every field defaults,
//! so a missing field tolerates rather than failing the read.

use serde::Deserialize;

/// Live-view JSON returned by `POST …/runs/{run}/jobs/{job}`.
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
    // Decoded for provider-shape fidelity; the portable verdict is derived from
    // the per-job statuses, so the run-level status is not read directly.
    #[serde(default)]
    #[allow(dead_code)]
    pub(super) status: String,
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
            r#"{"state":{"run":{"status":"success","jobs":[{"name":"build","status":"success"}],
               "commit":{"shortSHA":"abc1234"}}},"logs":{}}"#,
        )
        .unwrap();
        assert_eq!(dto.state.run.jobs.len(), 1);
        assert_eq!(dto.state.run.commit.short_sha, "abc1234");
    }
}
