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
//!
//! The orchestration (REST-then-web-UI fallback) lives here; the REST list/path
//! helpers are in [`fetch`] and the run/task → [`CiJob`] mapping and sorting in
//! [`jobs`].

mod fetch;
mod jobs;

use crate::ci_match::{Target, match_run, run_index, sort_runs};
use crate::ids::{
    CiJobCoord, RepoCoord, format_repository_id, parse_ci_job_id, parse_pull_request_id,
    parse_repository_id,
};
use crate::types::{ActionRunDto, ActionTaskDto};
use crate::{ForgejoForge, HttpClient};
use temper_forge::{CiJob, CiJobId, CiJobQuery, ForgeError, ForgeResult, RepositoryId};

pub(crate) use jobs::map_status;
use jobs::{latest_attempt, sort_jobs, task_to_job};

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
            && !commit.is_empty()
        {
            target.commit_sha = Some(commit.to_string());
        }

        // Prefer the REST Actions endpoint (richer, used by newer servers). On a
        // server that does not serve it (Forgejo 7.0.x → 404), or when REST is
        // available but lists no matching run, fall back to the password/web-UI
        // read path when credentials are configured (ADR 0019).
        let runs: Vec<ActionRunDto> = match self
            .try_fetch_actions_array("list Forgejo Actions runs", &fetch::runs_path(&repo))
            .await?
        {
            Some(runs) => runs,
            None => {
                return self
                    .list_ci_jobs_via_web_ui(repo_id, &repo, &target, &query)
                    .await;
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
            .fetch_actions_array("list Forgejo Actions tasks", &fetch::tasks_path(&repo))
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
            && let Some(cached) = self.ci_read_cache().get_terminal(key)
        {
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
            .try_fetch_actions_array("list Forgejo Actions runs", &fetch::runs_path(&coord.repo))
            .await?
        {
            Some(runs) => runs,
            None => return self.get_ci_job_via_web_ui(&coord, &repo_id).await,
        };
        let Some(run) = runs.into_iter().find(|run| run_index(run) == coord.run) else {
            return Ok(None);
        };

        let tasks: Vec<ActionTaskDto> = self
            .fetch_actions_array("list Forgejo Actions tasks", &fetch::tasks_path(&coord.repo))
            .await?;
        let latest = latest_attempt(&tasks, coord.run);
        let target = Target::default();

        // Prefer an exact index + task-id match; fall back to the task id alone
        // in case the attempt enumeration shifted between calls.
        if let Some(task) = latest.get(coord.job_index as usize)
            && task.id == coord.task_id
        {
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
