// SPDX-License-Identifier: MPL-2.0
//! CI job listing and lookup for the Forgejo backend.
//!
//! Forgejo Actions exposes workflow *runs* and *tasks*. This module adapts that
//! surface to the portable [`CiJob`] model: it matches runs to a pull request or
//! commit (see [`crate::ci_match`]), groups a run's tasks into attempts (the
//! latest attempt wins), and maps each task into a job. Log fetching is
//! intentionally out of scope; the portable trait only needs structured status.
//!
//! These are inherent methods on [`ForgejoForge`] matching the
//! [`temper_forge::Forge`] CI signatures; the trait is assembled once every
//! phase's methods exist. See `docs/reference/forgejo-backend.md`.

use crate::ci_match::{
    match_run, run_created, run_index, run_pr_number, run_updated, sort_runs, Target,
};
use crate::ids::{
    format_ci_job_id, format_pull_request_id, format_repository_id, parse_ci_job_id,
    parse_pull_request_id, parse_repository_id, CiJobCoord, RepoCoord,
};
use crate::types::{ActionRunDto, ActionTaskDto, PullRequestDto};
use crate::{ForgejoForge, HttpClient, HttpMethod};
use chrono::{DateTime, Utc};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::cmp::Ordering;
use temper_forge::{
    CiJob, CiJobConclusion, CiJobId, CiJobQuery, CiJobSortField, CiJobStatus, ForgeError,
    ForgeResult, ItemNumber, PullRequestId, RepositoryId, SortDirection,
};

/// Bound on Actions list responses, mirroring the reference TypeScript tooling.
const ACTIONS_LIMIT: &str = "200";
/// Optional diagnostic flag: when set, web-UI CI fallback reads are logged.
const CI_DIAGNOSTICS_ENV: &str = "TEMPER_FORGEJO_CI_DIAGNOSTICS";

impl<C: HttpClient> ForgejoForge<C> {
    /// Lists CI jobs for a repository, filtered by [`CiJobQuery`].
    ///
    /// When `query.pull_request_id` is set, the pull request is fetched first to
    /// learn its head SHA/ref before matching runs. Runs are matched to the
    /// query target, expanded to their latest attempt's tasks, mapped to jobs,
    /// then filtered by status and sorted.
    pub async fn list_ci_jobs(
        &self,
        repo_id: &RepositoryId,
        query: CiJobQuery,
    ) -> ForgeResult<Vec<CiJob>> {
        let repo = parse_repository_id(repo_id)?;
        let mut target = Target::default();
        if let Some(pr_id) = &query.pull_request_id {
            let (pr_repo, number) = parse_pull_request_id(pr_id)?;
            target.pr_id = Some(pr_id.clone());
            target.pr_number = Some(number.get());
            if let Some(head) = self.fetch_pr_head(&pr_repo, number).await? {
                target.pr_head_sha = head.0;
                target.pr_head_ref = head.1;
            }
        }
        if let Some(commit) = query.commit_sha.as_deref()
            && !commit.is_empty() {
                target.commit_sha = Some(commit.to_string());
            }

        // Prefer the REST Actions endpoint (richer, used by newer servers). On a
        // server that does not serve it (Forgejo 7.0.x → 404), or when REST is
        // available but lists no matching run, fall back to the password/web-UI
        // read path when credentials are configured (ADR 0019).
        let runs: Vec<ActionRunDto> = match self
            .try_fetch_actions_array("list Forgejo Actions runs", &runs_path(&repo))
            .await?
        {
            Some(runs) => runs,
            None => {
                return self
                    .list_ci_jobs_via_web_ui(repo_id, &repo, &target, &query)
                    .await
            }
        };
        let mut matched: Vec<ActionRunDto> = if target.has_filter() {
            runs.into_iter()
                .filter(|run| match_run(run, &target).is_some())
                .collect()
        } else {
            runs
        };
        if matched.is_empty() {
            // REST works but found no run for this target; a real run may still
            // exist that REST does not surface — try the web UI before giving up.
            return self
                .list_ci_jobs_via_web_ui(repo_id, &repo, &target, &query)
                .await;
        }
        sort_runs(&mut matched);

        let tasks: Vec<ActionTaskDto> = self
            .fetch_actions_array("list Forgejo Actions tasks", &tasks_path(&repo))
            .await?;
        let mut jobs = Vec::new();
        for run in &matched {
            for (index, task) in latest_attempt(&tasks, run_index(run))
                .into_iter()
                .enumerate()
            {
                jobs.push(task_to_job(
                    &repo,
                    repo_id,
                    run,
                    &task,
                    index as u64,
                    &target,
                ));
            }
        }

        if let Some(status) = query.status {
            jobs.retain(|job| job.status == status);
        }
        sort_jobs(&mut jobs, &query);
        Ok(jobs)
    }

    /// Reads CI jobs through the web UI, applying the query's status/sort.
    ///
    /// When no web-UI credentials are configured this keeps the existing hard
    /// `Backend` error rather than fabricating a verdict (matching the REST path
    /// when Actions is unavailable).
    async fn list_ci_jobs_via_web_ui(
        &self,
        repo_id: &RepositoryId,
        repo: &RepoCoord,
        target: &Target,
        query: &CiJobQuery,
    ) -> ForgeResult<Vec<CiJob>> {
        let Some(credentials) = self.config().web_ui.as_ref() else {
            return Err(ForgeError::Backend(
                "list Forgejo Actions runs: Forgejo Actions unavailable over REST and no \
                 web-UI credentials configured for the CI read fallback"
                    .to_string(),
            ));
        };

        // Idle-read gate (ADR 0019 cost mitigation): when this target's CI was
        // last read as terminal at the same head SHA, reuse it and skip the
        // expensive login+scrape. A non-terminal or changed-SHA read misses and
        // falls through to the live read below. The raw (pre status-filter) jobs
        // are what is cached, so the query's status filter/sort still applies.
        let cache_key = crate::ci_cache::CiReadKey::from_target(repo_id, target);
        if let Some(key) = cache_key.as_ref()
            && let Some(cached) = self.ci_read_cache().get_terminal(key) {
                let mut jobs = cached;
                if let Some(status) = query.status {
                    jobs.retain(|job| job.status == status);
                }
                sort_jobs(&mut jobs, query);
                return Ok(jobs);
            }

        log_web_ui_ci_read(repo, target, "read_ci_jobs_via_web_ui");
        let raw = crate::ci_ui::read_ci_jobs(self, credentials, repo, repo_id, target).await?;
        if let Some(key) = cache_key {
            self.ci_read_cache().store(key, raw.clone());
        }
        let mut jobs = raw;
        if let Some(status) = query.status {
            jobs.retain(|job| job.status == status);
        }
        sort_jobs(&mut jobs, query);
        Ok(jobs)
    }

    /// Looks up a single CI job through the web-UI read path (REST fallback).
    async fn get_ci_job_via_web_ui(
        &self,
        coord: &CiJobCoord,
        repo_id: &RepositoryId,
    ) -> ForgeResult<Option<CiJob>> {
        let Some(credentials) = self.config().web_ui.as_ref() else {
            return Err(ForgeError::Backend(
                "list Forgejo Actions runs: Forgejo Actions unavailable over REST and no \
                 web-UI credentials configured for the CI read fallback"
                    .to_string(),
            ));
        };
        log_web_ui_ci_read(&coord.repo, &Target::default(), "read_ci_job_via_web_ui");
        crate::ci_ui::read_ci_job(self, credentials, coord, repo_id).await
    }

    /// Looks up a single CI job by its encoded id.
    ///
    /// The repository coordinate is parsed out of the id (there is no repo
    /// parameter), so the caller needs only the opaque [`CiJobId`].
    pub async fn get_ci_job(&self, id: &CiJobId) -> ForgeResult<Option<CiJob>> {
        let coord = parse_ci_job_id(id)?;
        let repo_id = format_repository_id(&coord.repo);

        let runs: Vec<ActionRunDto> = match self
            .try_fetch_actions_array("list Forgejo Actions runs", &runs_path(&coord.repo))
            .await?
        {
            Some(runs) => runs,
            None => return self.get_ci_job_via_web_ui(&coord, &repo_id).await,
        };
        let Some(run) = runs.into_iter().find(|run| run_index(run) == coord.run) else {
            return Ok(None);
        };

        let tasks: Vec<ActionTaskDto> = self
            .fetch_actions_array("list Forgejo Actions tasks", &tasks_path(&coord.repo))
            .await?;
        let latest = latest_attempt(&tasks, coord.run);
        let target = Target::default();

        // Prefer an exact index + task-id match; fall back to the task id alone
        // in case the attempt enumeration shifted between calls.
        if let Some(task) = latest.get(coord.job_index as usize)
            && task.id == coord.task_id {
                return Ok(Some(task_to_job(
                    &coord.repo,
                    &repo_id,
                    &run,
                    task,
                    coord.job_index,
                    &target,
                )));
            }
        for (index, task) in latest.iter().enumerate() {
            if task.id == coord.task_id {
                return Ok(Some(task_to_job(
                    &coord.repo,
                    &repo_id,
                    &run,
                    task,
                    index as u64,
                    &target,
                )));
            }
        }
        Ok(None)
    }

    /// Fetches a pull request's head SHA and head ref for run matching.
    ///
    /// Returns `None` when the pull request is absent (`404`). Reuses the
    /// existing [`PullRequestDto`] rather than introducing a CI-only DTO.
    async fn fetch_pr_head(
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
        Ok(Some((
            head.sha.filter(|sha| !sha.is_empty()),
            head.branch.filter(|branch| !branch.is_empty()),
        )))
    }

    /// Fetches an Actions list endpoint and decodes its `workflow_runs` array.
    ///
    /// Treats `403`/`404` as an unavailable backend ([`ForgeError::Backend`]) so
    /// missing Actions support never looks like a passed or failed gate.
    async fn fetch_actions_array<T: DeserializeOwned>(
        &self,
        context: &str,
        path: &str,
    ) -> ForgeResult<Vec<T>> {
        self.try_fetch_actions_array(context, path)
            .await?
            .ok_or_else(|| {
                ForgeError::Backend(format!("{context}: Forgejo Actions unavailable over REST"))
            })
    }

    /// Like [`Self::fetch_actions_array`] but reports REST unavailability as
    /// `Ok(None)` so the caller can fall back to the web-UI read path.
    ///
    /// A `403`/`404` (the endpoint is absent, as on Forgejo 7.0.x) yields
    /// `Ok(None)`; any other non-2xx status is still a hard [`ForgeError`].
    async fn try_fetch_actions_array<T: DeserializeOwned>(
        &self,
        context: &str,
        path: &str,
    ) -> ForgeResult<Option<Vec<T>>> {
        let query = vec![("limit".to_string(), ACTIONS_LIMIT.to_string())];
        let response = self.send(HttpMethod::Get, path, query, None).await?;
        match response.status {
            200..=299 => {
                extract_array(context, &response.body, &["workflow_runs", "runs", "tasks"])
                    .map(Some)
            }
            403 | 404 => Ok(None),
            other => Err(ForgeError::Backend(format!(
                "{context}: unexpected status {other}"
            ))),
        }
    }
}

fn runs_path(repo: &RepoCoord) -> String {
    format!("/repos/{}/actions/runs", repo.path_segment())
}

fn tasks_path(repo: &RepoCoord) -> String {
    format!("/repos/{}/actions/tasks", repo.path_segment())
}

/// Returns the latest attempt's tasks for a run, ordered by canonical task id.
///
/// Tasks are tied to a run by `run_number == run_index`, sorted by monotonic id,
/// then split into attempts: a repeated task name starts a new attempt.
fn latest_attempt(tasks: &[ActionTaskDto], run: u64) -> Vec<ActionTaskDto> {
    let run_tasks: Vec<ActionTaskDto> = tasks
        .iter()
        .filter(|task| task.run_number == run)
        .cloned()
        .collect();
    group_attempts(run_tasks).pop().unwrap_or_default()
}

/// Groups a run's tasks into attempts; a repeated task name starts a new one.
fn group_attempts(mut tasks: Vec<ActionTaskDto>) -> Vec<Vec<ActionTaskDto>> {
    tasks.sort_by_key(|task| task.id);
    let mut attempts: Vec<Vec<ActionTaskDto>> = Vec::new();
    let mut current: Vec<ActionTaskDto> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for task in tasks {
        if seen.contains(&task.name) {
            attempts.push(std::mem::take(&mut current));
            seen.clear();
        }
        seen.push(task.name.clone());
        current.push(task);
    }
    if !current.is_empty() {
        attempts.push(current);
    }
    attempts
}

/// Maps a Forgejo status string to a portable status/conclusion pair.
///
/// Shared with the web-UI CI read path ([`crate::ci_ui`]) so both surfaces map
/// the same provider status vocabulary identically.
pub(crate) fn map_status(status: &str) -> (CiJobStatus, Option<CiJobConclusion>) {
    match status.trim().to_ascii_lowercase().as_str() {
        "success" => (CiJobStatus::Completed, Some(CiJobConclusion::Success)),
        "failure" => (CiJobStatus::Completed, Some(CiJobConclusion::Failure)),
        "cancelled" | "canceled" => (CiJobStatus::Completed, Some(CiJobConclusion::Cancelled)),
        "skipped" => (CiJobStatus::Completed, Some(CiJobConclusion::Skipped)),
        "timeout" | "timed_out" => (CiJobStatus::Completed, Some(CiJobConclusion::TimedOut)),
        "neutral" => (CiJobStatus::Completed, Some(CiJobConclusion::Neutral)),
        "running" | "in_progress" => (CiJobStatus::Running, None),
        "waiting" | "queued" | "requested" | "blocked" | "pending" => (CiJobStatus::Queued, None),
        _ => (CiJobStatus::Queued, None),
    }
}

/// Builds a portable job from a run/task pair at a given attempt index.
fn task_to_job(
    repo: &RepoCoord,
    repo_id: &RepositoryId,
    run: &ActionRunDto,
    task: &ActionTaskDto,
    job_index: u64,
    target: &Target,
) -> CiJob {
    let (status, conclusion) = map_status(&task.status);
    let commit_sha = first_non_empty(&[
        &task.commit_sha,
        &task.head_sha,
        &run.commit_sha,
        &run.head_sha,
    ])
    .or_else(|| target.pr_head_sha.clone())
    .or_else(|| target.commit_sha.clone())
    .unwrap_or_default();

    let pull_request_id: Option<PullRequestId> = target.pr_id.clone().or_else(|| {
        run_pr_number(run).map(|number| format_pull_request_id(repo, ItemNumber::new(number)))
    });

    let name = if task.name.is_empty() {
        format!("job-{job_index}")
    } else {
        task.name.clone()
    };
    let url = first_non_empty(&[&task.html_url, &task.url, &run.html_url, &run.url]);

    let created_at = task
        .created_at
        .or(task.created)
        .or_else(|| run_created(run))
        .unwrap_or_else(epoch);
    let updated_at = task
        .updated_at
        .or(task.updated)
        .or_else(|| run_updated(run))
        .unwrap_or(created_at);
    let started_at = task.run_started_at.or(task.started);
    let completed_at = task
        .stopped
        .or_else(|| (status == CiJobStatus::Completed).then_some(updated_at));

    let coord = CiJobCoord {
        repo: repo.clone(),
        run: run_index(run),
        job_index,
        task_id: task.id,
    };

    CiJob {
        id: format_ci_job_id(&coord),
        repo_id: repo_id.clone(),
        pull_request_id,
        commit_sha,
        name,
        status,
        conclusion,
        url,
        created_at,
        started_at,
        completed_at,
        updated_at,
    }
}

fn first_non_empty(values: &[&str]) -> Option<String> {
    values
        .iter()
        .find(|value| !value.is_empty())
        .map(|value| value.to_string())
}

/// The unix epoch, used as a deterministic fallback for absent timestamps.
fn epoch() -> DateTime<Utc> {
    DateTime::<Utc>::from_timestamp(0, 0).expect("unix epoch is a valid timestamp")
}

/// Tolerantly decodes a JSON array that may be bare or wrapped in an object.
fn extract_array<T: DeserializeOwned>(
    context: &str,
    body: &str,
    keys: &[&str],
) -> ForgeResult<Vec<T>> {
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
    for key in keys {
        match value.get(*key) {
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

/// Sorts jobs by the requested order, mirroring the reference backends.
fn sort_jobs(jobs: &mut [CiJob], query: &CiJobQuery) {
    jobs.sort_by(|left, right| compare_jobs(left, right, query));
}

fn compare_jobs(left: &CiJob, right: &CiJob, query: &CiJobQuery) -> Ordering {
    let primary = query
        .sort
        .map(|sort| {
            let comparison = match sort.field {
                CiJobSortField::Name => left.name.cmp(&right.name),
                CiJobSortField::CreatedAt => left.created_at.cmp(&right.created_at),
                CiJobSortField::UpdatedAt => left.updated_at.cmp(&right.updated_at),
            };
            match sort.direction {
                SortDirection::Asc => comparison,
                SortDirection::Desc => comparison.reverse(),
            }
        })
        .unwrap_or_else(|| left.name.cmp(&right.name));
    primary
        .then_with(|| left.name.cmp(&right.name))
        .then_with(|| left.id.cmp(&right.id))
}

fn log_web_ui_ci_read(repo: &RepoCoord, target: &Target, operation: &str) {
    if std::env::var_os(CI_DIAGNOSTICS_ENV).is_none() {
        return;
    }
    eprintln!(
        "temper-forge-forgejo: {operation} repo={}/{} pr={} head_ref={} commit={}",
        repo.owner,
        repo.name,
        target
            .pr_number
            .map(|number| number.to_string())
            .unwrap_or_else(|| "-".to_string()),
        target.pr_head_ref.as_deref().unwrap_or("-"),
        target.commit_sha.as_deref().unwrap_or("-")
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(id: u64, run_number: u64, name: &str, status: &str) -> ActionTaskDto {
        ActionTaskDto {
            id,
            run_number,
            name: name.to_string(),
            status: status.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn group_attempts_splits_on_repeated_name() {
        let tasks = vec![
            task(1, 1, "build", "success"),
            task(2, 1, "test", "success"),
            task(3, 1, "build", "success"),
            task(4, 1, "test", "failure"),
        ];
        let attempts = group_attempts(tasks);
        assert_eq!(attempts.len(), 2);
        assert_eq!(attempts[1][0].id, 3);
        assert_eq!(attempts[1][1].id, 4);
    }

    #[test]
    fn latest_attempt_filters_by_run_and_returns_last() {
        let tasks = vec![
            task(1, 10, "build", "success"),
            task(2, 10, "build", "failure"),
            task(3, 11, "lint", "success"),
        ];
        let latest = latest_attempt(&tasks, 10);
        assert_eq!(latest.len(), 1);
        assert_eq!(latest[0].id, 2);
    }

    #[test]
    fn status_mapping_covers_states() {
        assert_eq!(
            map_status("success"),
            (CiJobStatus::Completed, Some(CiJobConclusion::Success))
        );
        assert_eq!(
            map_status("FAILURE"),
            (CiJobStatus::Completed, Some(CiJobConclusion::Failure))
        );
        assert_eq!(
            map_status("cancelled"),
            (CiJobStatus::Completed, Some(CiJobConclusion::Cancelled))
        );
        assert_eq!(
            map_status("timed_out"),
            (CiJobStatus::Completed, Some(CiJobConclusion::TimedOut))
        );
        assert_eq!(map_status("running"), (CiJobStatus::Running, None));
        assert_eq!(map_status("queued"), (CiJobStatus::Queued, None));
        assert_eq!(map_status("mystery"), (CiJobStatus::Queued, None));
    }

    #[test]
    fn extract_array_handles_wrapped_bare_and_null() {
        let wrapped: Vec<ActionTaskDto> = extract_array(
            "ctx",
            r#"{"workflow_runs":[{"id":1,"name":"build"}]}"#,
            &["workflow_runs"],
        )
        .unwrap();
        assert_eq!(wrapped.len(), 1);
        let bare: Vec<ActionTaskDto> =
            extract_array("ctx", r#"[{"id":2,"name":"test"}]"#, &["workflow_runs"]).unwrap();
        assert_eq!(bare.len(), 1);
        let null: Vec<ActionTaskDto> =
            extract_array("ctx", r#"{"workflow_runs":null}"#, &["workflow_runs"]).unwrap();
        assert!(null.is_empty());
        let empty: Vec<ActionTaskDto> = extract_array("ctx", "   ", &["workflow_runs"]).unwrap();
        assert!(empty.is_empty());
    }
}
