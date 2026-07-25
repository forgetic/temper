//! REST Actions fetching for the CI read path: workflow runs, strict per-run
//! jobs, and the pull-request head lookup used for run matching.

use crate::ids::RepoCoord;
use crate::map::pr_branch_name;
use crate::types::{ActionJobDto, ActionJobsResponseDto, PullRequestDto};
use crate::{ForgejoForge, HttpClient, HttpMethod};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::collections::HashSet;
use temper_forge_model::{ForgeError, ForgeResult, ItemNumber};

/// Bound on Actions run-list responses, mirroring the reference tooling.
const ACTIONS_LIMIT: &str = "200";

impl<C: HttpClient> ForgejoForge<C> {
    /// Fetches a pull request's head SHA and head ref for run matching.
    ///
    /// Returns `None` when the pull request is absent (`404`). Reuses the
    /// existing [`PullRequestDto`] rather than introducing a CI-only DTO.
    pub(super) async fn fetch_pr_head(
        &self,
        repo: &RepoCoord,
        number: ItemNumber,
    ) -> ForgeResult<Option<(Option<String>, Option<String>)>> {
        let path = format!("/repos/{}/pulls/{}", repo.path_segment(), number.get());
        let Some(response) = self
            .request_optional(
                "get pull request for CI",
                HttpMethod::Get,
                &path,
                Vec::new(),
                None,
            )
            .await?
        else {
            return Ok(None);
        };
        let dto: PullRequestDto = Self::decode("get pull request for CI", &response)?;
        let head = dto.head.unwrap_or_default();
        let branch = pr_branch_name(&head);
        Ok(Some((head.sha.filter(|sha| !sha.is_empty()), branch)))
    }

    /// Fetches the Actions runs endpoint and decodes its list response.
    ///
    /// Missing or unsupported Actions APIs fail closed. There is deliberately no
    /// HTML, login, live-view, or repository-wide tasks fallback.
    pub(super) async fn fetch_actions_runs<T: DeserializeOwned>(
        &self,
        context: &str,
        path: &str,
    ) -> ForgeResult<Vec<T>> {
        let query = vec![("limit".to_string(), ACTIONS_LIMIT.to_string())];
        let response = self.send(HttpMethod::Get, path, query, None).await?;
        match response.status {
            200..=299 => extract_runs_array(context, &response.body),
            other => Err(ForgeError::Backend(format!(
                "{context}: Forgejo Actions unavailable (HTTP {other})"
            ))),
        }
    }

    /// Fetches and validates one provider run's Forgejo 16 jobs response.
    ///
    /// The endpoint coordinate is the run database `id`. Every returned row
    /// must carry a non-zero provider run/job identity and must identify that
    /// same run. Provider-reported zero attempt/task values are retained because
    /// Forgejo uses them before a queued job is assigned to a runner. The
    /// explicit response wrapper is required, including for an empty job list.
    pub(super) async fn fetch_run_jobs(
        &self,
        repo: &RepoCoord,
        run_id: u64,
    ) -> ForgeResult<Vec<ActionJobDto>> {
        let context = format!("list Forgejo Actions jobs for provider run {run_id}");
        if run_id == 0 {
            return Err(ForgeError::Backend(format!(
                "{context}: invalid zero provider run id"
            )));
        }
        let path = jobs_path(repo, run_id);
        let response = self.send(HttpMethod::Get, &path, Vec::new(), None).await?;
        if !response.is_success() {
            return Err(ForgeError::Backend(format!(
                "{context}: provider jobs unavailable (HTTP {})",
                response.status
            )));
        }
        let response: ActionJobsResponseDto =
            serde_json::from_str(&response.body).map_err(|error| {
                ForgeError::Backend(format!(
                    "{context}: failed to decode expected jobs response: {error}"
                ))
            })?;
        validate_jobs(&context, run_id, &response.jobs)?;
        Ok(current_attempt(response.jobs))
    }
}

pub(super) fn runs_path(repo: &RepoCoord) -> String {
    format!("/repos/{}/actions/runs", repo.path_segment())
}

pub(super) fn jobs_path(repo: &RepoCoord, run_id: u64) -> String {
    format!("/repos/{}/actions/runs/{run_id}/jobs", repo.path_segment())
}

/// Tolerantly decodes the established runs list shape.
fn extract_runs_array<T: DeserializeOwned>(context: &str, body: &str) -> ForgeResult<Vec<T>> {
    let trimmed = body.trim_start();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    if trimmed.starts_with('[') {
        return serde_json::from_str::<Vec<T>>(body).map_err(|error| {
            ForgeError::Backend(format!("{context}: failed to decode array: {error}"))
        });
    }
    let value: Value = serde_json::from_str(body).map_err(|error| {
        ForgeError::Backend(format!("{context}: failed to decode response: {error}"))
    })?;
    for key in ["workflow_runs", "runs"] {
        match value.get(key) {
            None | Some(Value::Null) => continue,
            Some(array) => {
                return serde_json::from_value(array.clone()).map_err(|error| {
                    ForgeError::Backend(format!("{context}: failed to decode `{key}`: {error}"))
                });
            }
        }
    }
    Ok(Vec::new())
}

fn validate_jobs(context: &str, run_id: u64, jobs: &[ActionJobDto]) -> ForgeResult<()> {
    let mut identities = HashSet::new();
    for job in jobs {
        if job.id == 0 || job.run_id == 0 {
            return Err(ForgeError::Backend(format!(
                "{context}: invalid zero provider run or job identity"
            )));
        }
        if job.run_id != run_id {
            return Err(ForgeError::Backend(format!(
                "{context}: job {} belongs to provider run {}, expected {run_id}",
                job.id, job.run_id
            )));
        }
        if job.name.trim().is_empty() || job.status.trim().is_empty() {
            return Err(ForgeError::Backend(format!(
                "{context}: job {} has an empty required name or status",
                job.id
            )));
        }
        if !identities.insert((job.id, job.run_id, job.attempt, job.task_id)) {
            return Err(ForgeError::Backend(format!(
                "{context}: duplicate provider job identity"
            )));
        }
    }
    Ok(())
}

/// Selects the largest provider-reported attempt and orders its jobs by stable
/// provider identity. Names and response order never determine attempts.
fn current_attempt(mut jobs: Vec<ActionJobDto>) -> Vec<ActionJobDto> {
    let Some(attempt) = jobs.iter().map(|job| job.attempt).max() else {
        return jobs;
    };
    jobs.retain(|job| job.attempt == attempt);
    jobs.sort_by_key(|job| (job.id, job.task_id));
    jobs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(id: u64, attempt: u64, task_id: u64, name: &str) -> ActionJobDto {
        ActionJobDto {
            id,
            run_id: 900,
            attempt,
            task_id,
            name: name.to_string(),
            status: "success".to_string(),
        }
    }

    #[test]
    fn current_attempt_uses_provider_values_not_names_or_order() {
        let jobs = current_attempt(vec![
            job(33, 2, 43, "build"),
            job(11, 1, 41, "build"),
            job(44, 2, 44, "build"),
            job(22, 1, 42, "test"),
        ]);
        assert_eq!(jobs.iter().map(|job| job.id).collect::<Vec<_>>(), [33, 44]);
        assert!(jobs.iter().all(|job| job.attempt == 2));
    }

    #[test]
    fn validates_run_scoped_identity_and_accepts_unassigned_jobs() {
        let valid = job(31, 1, 41, "build");
        assert!(validate_jobs("ctx", 900, std::slice::from_ref(&valid)).is_ok());

        let mut unassigned = valid.clone();
        unassigned.attempt = 0;
        unassigned.task_id = 0;
        assert!(validate_jobs("ctx", 900, &[unassigned]).is_ok());

        let mut mismatch = valid.clone();
        mismatch.run_id = 901;
        assert!(validate_jobs("ctx", 900, &[mismatch]).is_err());

        let mut invalid = valid;
        invalid.id = 0;
        assert!(validate_jobs("ctx", 900, &[invalid]).is_err());
    }
}
