// SPDX-License-Identifier: MPL-2.0
//! CI job listing and lookup for the Forgejo backend.
//!
//! Forgejo 16 exposes workflow runs and each run's jobs through token-authenticated
//! JSON APIs. Runs are first matched strictly to the requested pull/commit; every
//! matched run is then expanded through
//! `GET /repos/{owner}/{repo}/actions/runs/{provider_run_id}/jobs`. Provider run,
//! job, attempt, and task coordinates form the opaque identity. No repository-wide
//! tasks, HTML, login, live-view, or response-order fallback participates.

mod fetch;
mod jobs;

use crate::ci_match::{Target, match_run, sort_runs};
use crate::ids::{
    format_repository_id, parse_ci_job_id, parse_pull_request_id, parse_repository_id,
};
use crate::types::ActionRunDto;
use crate::{ForgejoForge, HttpClient};
use temper_forge_model::{
    CiJob, CiJobId, CiJobListing, CiJobQuery, CiRetryOutcome, CiRetryRejection, CiRetryRequest,
    ForgeResult, RepositoryId,
};

pub(crate) use jobs::map_status_evidence;
use jobs::{job_to_ci_job, sort_jobs};

impl<C: HttpClient> ForgejoForge<C> {
    /// Lists CI jobs for a repository, filtered by [`CiJobQuery`].
    pub async fn list_ci_jobs(
        &self,
        repo_id: &RepositoryId,
        query: CiJobQuery,
    ) -> ForgeResult<Vec<CiJob>> {
        Ok(self
            .list_ci_jobs_with_presence(repo_id, query)
            .await?
            .into_jobs())
    }

    /// Lists CI jobs while preserving matching provider-run presence before job
    /// assignment.
    pub async fn list_ci_jobs_with_presence(
        &self,
        repo_id: &RepositoryId,
        query: CiJobQuery,
    ) -> ForgeResult<CiJobListing> {
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
        if let Some(commit) = query
            .commit_sha
            .as_deref()
            .filter(|commit| !commit.is_empty())
        {
            target.commit_sha = Some(commit.to_string());
        }

        let runs: Vec<ActionRunDto> = self
            .fetch_actions_runs("list Forgejo Actions runs", &fetch::runs_path(&repo))
            .await?;
        let mut matched: Vec<ActionRunDto> = if target.has_filter() {
            runs.into_iter()
                .filter(|run| match_run(run, &target).is_some())
                .collect()
        } else {
            runs
        };
        if matched.is_empty() {
            return Ok(CiJobListing::new(Vec::new(), false));
        }
        sort_runs(&mut matched);

        let mut jobs = Vec::new();
        for run in &matched {
            // `run.id` is the provider database id required by Forgejo 16. The
            // display coordinates (`index_in_repo`/`run_number`) are never used
            // in the jobs route or opaque identity.
            let dtos = self.fetch_run_jobs(&repo, run.id).await?;
            for dto in dtos {
                if let Some(job) = job_to_ci_job(&repo, repo_id, run, &dto, &target) {
                    jobs.push(job);
                }
            }
        }

        if let Some(status) = query.status {
            jobs.retain(|job| job.status == status);
        }
        sort_jobs(&mut jobs, &query);
        Ok(CiJobListing::new(jobs, true))
    }

    /// Reports exact-attempt retry as unsupported for Forgejo.
    ///
    /// No source mutation or guessed HTTP endpoint is used.
    pub async fn retry_ci_attempt(&self, request: CiRetryRequest) -> ForgeResult<CiRetryOutcome> {
        let repo = parse_repository_id(request.repo_id())?;
        let (pull_repo, _) = parse_pull_request_id(request.pull_request_id())?;
        if repo != pull_repo {
            return Ok(CiRetryOutcome::Rejected(
                CiRetryRejection::RepositoryMismatch,
            ));
        }
        Ok(CiRetryOutcome::Unsupported)
    }

    /// Looks up one job by its provider-backed opaque identity.
    ///
    /// The run list supplies the same ownership/timestamp/URL evidence as list;
    /// the exact provider run is then read through its per-run jobs endpoint.
    pub async fn get_ci_job(&self, id: &CiJobId) -> ForgeResult<Option<CiJob>> {
        let coord = parse_ci_job_id(id)?;
        let repo_id = format_repository_id(&coord.repo);
        let runs: Vec<ActionRunDto> = self
            .fetch_actions_runs("list Forgejo Actions runs", &fetch::runs_path(&coord.repo))
            .await?;
        let Some(run) = runs.into_iter().find(|run| run.id == coord.run_id) else {
            return Ok(None);
        };

        let jobs = self.fetch_run_jobs(&coord.repo, run.id).await?;
        let Some(job) = jobs.into_iter().find(|job| {
            job.id == coord.job_id
                && job.run_id == coord.run_id
                && job.attempt == coord.attempt
                && job.task_id == coord.task_id
        }) else {
            return Ok(None);
        };
        let target = Target::default();
        Ok(job_to_ci_job(&coord.repo, &repo_id, &run, &job, &target))
    }
}
