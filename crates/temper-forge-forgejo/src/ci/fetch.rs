//! REST Actions fetching for the CI read path: workflow runs, strict per-run
//! jobs, and the pull-request head lookup used for run matching.

use crate::ids::RepoCoord;
use crate::map::pr_branch_name;
use crate::types::{ActionJobDto, PullRequestDto};
use crate::{ForgejoForge, HttpClient, HttpMethod};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};
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
    /// Forgejo uses them before a queued job is assigned to a runner. Forgejo's
    /// bare JSON array is required; an explicit `[]` is a successful empty list.
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
        let jobs: Vec<ActionJobDto> = serde_json::from_str(&response.body).map_err(|error| {
            ForgeError::Backend(format!(
                "{context}: failed to decode expected jobs array: {error}"
            ))
        })?;
        validate_jobs(&context, run_id, &jobs)?;
        Ok(current_attempt(jobs))
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
        if !identities.insert((job.id, job.attempt)) {
            return Err(ForgeError::Backend(format!(
                "{context}: duplicate provider job attempt identity"
            )));
        }
    }
    Ok(())
}

/// Selects each stable provider job's largest provider-reported attempt.
///
/// Forgejo's endpoint normally returns one row per `ActionRunJob`, whose
/// `attempt` and `task_id` are already that job's latest values. Attempts may
/// legitimately differ between jobs, so a run-wide maximum must not discard
/// unaffected jobs. Grouping by provider job id also remains deterministic if
/// a provider ever returns more than one attempt row. Names and response order
/// never determine attempts.
fn current_attempt(jobs: Vec<ActionJobDto>) -> Vec<ActionJobDto> {
    let mut current = BTreeMap::new();
    for job in jobs {
        match current.entry(job.id) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(job);
            }
            std::collections::btree_map::Entry::Occupied(mut entry)
                if job.attempt > entry.get().attempt =>
            {
                entry.insert(job);
            }
            std::collections::btree_map::Entry::Occupied(_) => {}
        }
    }
    current.into_values().collect()
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
    fn current_attempt_is_selected_per_provider_job() {
        let jobs = current_attempt(vec![
            job(33, 2, 44, "build"),
            job(11, 1, 41, "build"),
            job(11, 2, 43, "build"),
            job(22, 1, 42, "test"),
        ]);
        assert_eq!(
            jobs.iter().map(|job| job.id).collect::<Vec<_>>(),
            [11, 22, 33]
        );
        assert_eq!(
            jobs.iter().map(|job| job.attempt).collect::<Vec<_>>(),
            [2, 1, 2]
        );
        assert_eq!(jobs[0].task_id, 43);
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
