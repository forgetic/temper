//! CI job listing and lookup for the GitHub backend.
//!
//! GitHub Actions exposes workflow *runs* and *jobs* over REST. This module
//! adapts that surface to the portable [`CiJob`] model: runs are narrowed by
//! head SHA (provider-side via the `head_sha` query parameter), each matched
//! run's latest-attempt jobs are fetched, and every job maps to one portable
//! CI job. Log fetching is intentionally out of scope; the portable trait only
//! needs structured status.

use crate::ids::{
    CiJobCoord, RepoCoord, format_ci_job_id, parse_ci_job_id, parse_pull_request_id,
    parse_repository_id,
};
use crate::types::{WorkflowJobDto, WorkflowJobsEnvelopeDto, WorkflowRunsEnvelopeDto};
use crate::{GitHubForge, HttpClient, HttpMethod};
use chrono::{DateTime, Utc};
use temper_forge::{
    CiJob, CiJobConclusion, CiJobId, CiJobQuery, CiJobSortField, CiJobStatus, ForgeResult,
    PullRequestId, RepositoryId, SortDirection,
};

impl<C: HttpClient> GitHubForge<C> {
    /// Lists CI jobs for a repository, filtered by [`CiJobQuery`].
    ///
    /// When `query.pull_request_id` is set, the pull request is fetched first
    /// to learn its head SHA; an explicit `query.commit_sha` takes precedence.
    /// Runs matching the resolved SHA (or every run when no filter is given)
    /// are expanded to their latest-attempt jobs, mapped, filtered by status,
    /// and sorted.
    pub async fn list_ci_jobs(
        &self,
        repo_id: &RepositoryId,
        query: CiJobQuery,
    ) -> ForgeResult<Vec<CiJob>> {
        let repo = parse_repository_id(repo_id)?;

        let mut head_sha = query.commit_sha.clone().filter(|commit| !commit.is_empty());
        let mut pr_id: Option<PullRequestId> = None;
        if let Some(id) = &query.pull_request_id {
            let (pr_repo, number) = parse_pull_request_id(id)?;
            pr_id = Some(id.clone());
            let Some(pull) = self.fetch_pull_request(&pr_repo, number).await? else {
                // No pull request means no runs can match it.
                return Ok(Vec::new());
            };
            if head_sha.is_none() {
                match pull.head_sha {
                    Some(sha) if !sha.is_empty() => head_sha = Some(sha),
                    // Without a head SHA, runs cannot be tied to the pull
                    // request; report no jobs rather than guessing.
                    _ => return Ok(Vec::new()),
                }
            }
        }

        let runs_path = format!("/repos/{}/actions/runs", repo.path_segment());
        let base_query = match &head_sha {
            Some(sha) => vec![("head_sha".to_string(), sha.clone())],
            None => Vec::new(),
        };
        let runs = self
            .list_all_wrapped(
                "list workflow runs",
                &runs_path,
                base_query,
                |envelope: WorkflowRunsEnvelopeDto| envelope.workflow_runs,
            )
            .await?;

        let mut jobs = Vec::new();
        for run in runs {
            let jobs_path = format!(
                "/repos/{}/actions/runs/{}/jobs",
                repo.path_segment(),
                run.id
            );
            // GitHub's default `filter=latest` returns only the latest attempt.
            let dtos = self
                .list_all_wrapped(
                    "list workflow jobs",
                    &jobs_path,
                    Vec::new(),
                    |envelope: WorkflowJobsEnvelopeDto| envelope.jobs,
                )
                .await?;
            for dto in dtos {
                jobs.push(map_workflow_job(&repo, repo_id, pr_id.clone(), dto));
            }
        }

        if let Some(status) = query.status {
            jobs.retain(|job| job.status == status);
        }
        sort_jobs(&mut jobs, &query);
        Ok(jobs)
    }

    /// Looks up a CI job by stable backend identifier
    /// (`GET /repos/{owner}/{repo}/actions/jobs/{job_id}`); `404` maps to `None`.
    pub async fn get_ci_job(&self, id: &CiJobId) -> ForgeResult<Option<CiJob>> {
        let coord = parse_ci_job_id(id)?;
        let path = format!(
            "/repos/{}/actions/jobs/{}",
            coord.repo.path_segment(),
            coord.job_id
        );
        let Some(response) = self
            .request_optional("get ci job", HttpMethod::Get, &path, Vec::new(), None)
            .await?
        else {
            return Ok(None);
        };
        let dto: WorkflowJobDto = Self::decode("get ci job", &response)?;
        let repo_id = crate::ids::format_repository_id(&coord.repo);
        Ok(Some(map_workflow_job(&coord.repo, &repo_id, None, dto)))
    }
}

/// Maps a GitHub workflow job DTO into a portable [`CiJob`].
///
/// `pull_request_id` is attached when the caller resolved the jobs through a
/// pull-request query; a direct job lookup carries no PR association.
fn map_workflow_job(
    repo: &RepoCoord,
    repo_id: &RepositoryId,
    pull_request_id: Option<PullRequestId>,
    dto: WorkflowJobDto,
) -> CiJob {
    let status = map_job_status(&dto.status);
    let conclusion = if status == CiJobStatus::Completed {
        dto.conclusion.as_deref().and_then(map_job_conclusion)
    } else {
        None
    };
    let created_at = dto
        .created_at
        .or(dto.started_at)
        .or(dto.completed_at)
        .unwrap_or(EPOCH);
    let updated_at = dto
        .completed_at
        .or(dto.started_at)
        .or(dto.created_at)
        .unwrap_or(EPOCH);

    CiJob {
        id: format_ci_job_id(&CiJobCoord {
            repo: repo.clone(),
            job_id: dto.id,
        }),
        repo_id: repo_id.clone(),
        pull_request_id,
        commit_sha: dto.head_sha,
        name: dto.name,
        status,
        conclusion,
        url: dto.html_url.filter(|url| !url.is_empty()),
        created_at,
        started_at: dto.started_at,
        completed_at: dto.completed_at,
        updated_at,
    }
}

/// Fallback timestamp for job payloads missing every timestamp field.
const EPOCH: DateTime<Utc> = DateTime::UNIX_EPOCH;

/// Maps a GitHub job status string to the portable [`CiJobStatus`].
///
/// GitHub's non-terminal, non-running states (`queued`, `waiting`, `pending`,
/// `requested`, and anything unknown) all map to `Queued`: the portable model
/// only distinguishes waiting / running / done.
fn map_job_status(status: &str) -> CiJobStatus {
    match status.trim().to_lowercase().as_str() {
        "completed" => CiJobStatus::Completed,
        "in_progress" => CiJobStatus::Running,
        _ => CiJobStatus::Queued,
    }
}

/// Maps a GitHub job conclusion string to the portable [`CiJobConclusion`].
///
/// `startup_failure` and `action_required` count as failures (the job did not
/// succeed and will not without intervention); `stale` maps to `Neutral`.
/// Unknown conclusions map to `None` rather than guessing.
fn map_job_conclusion(conclusion: &str) -> Option<CiJobConclusion> {
    match conclusion.trim().to_lowercase().as_str() {
        "success" => Some(CiJobConclusion::Success),
        "failure" | "startup_failure" | "action_required" => Some(CiJobConclusion::Failure),
        "cancelled" => Some(CiJobConclusion::Cancelled),
        "skipped" => Some(CiJobConclusion::Skipped),
        "timed_out" => Some(CiJobConclusion::TimedOut),
        "neutral" | "stale" => Some(CiJobConclusion::Neutral),
        _ => None,
    }
}

/// Orders jobs by the requested sort, then by name, then by id.
fn sort_jobs(jobs: &mut [CiJob], query: &CiJobQuery) {
    jobs.sort_by(|left, right| {
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
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_status_mapping() {
        assert_eq!(map_job_status("queued"), CiJobStatus::Queued);
        assert_eq!(map_job_status("waiting"), CiJobStatus::Queued);
        assert_eq!(map_job_status("requested"), CiJobStatus::Queued);
        assert_eq!(map_job_status("in_progress"), CiJobStatus::Running);
        assert_eq!(map_job_status("completed"), CiJobStatus::Completed);
        assert_eq!(map_job_status("mystery"), CiJobStatus::Queued);
    }

    #[test]
    fn job_conclusion_mapping() {
        assert_eq!(
            map_job_conclusion("success"),
            Some(CiJobConclusion::Success)
        );
        assert_eq!(
            map_job_conclusion("failure"),
            Some(CiJobConclusion::Failure)
        );
        assert_eq!(
            map_job_conclusion("startup_failure"),
            Some(CiJobConclusion::Failure)
        );
        assert_eq!(
            map_job_conclusion("action_required"),
            Some(CiJobConclusion::Failure)
        );
        assert_eq!(
            map_job_conclusion("cancelled"),
            Some(CiJobConclusion::Cancelled)
        );
        assert_eq!(
            map_job_conclusion("skipped"),
            Some(CiJobConclusion::Skipped)
        );
        assert_eq!(
            map_job_conclusion("timed_out"),
            Some(CiJobConclusion::TimedOut)
        );
        assert_eq!(
            map_job_conclusion("neutral"),
            Some(CiJobConclusion::Neutral)
        );
        assert_eq!(map_job_conclusion("stale"), Some(CiJobConclusion::Neutral));
        assert_eq!(map_job_conclusion("mystery"), None);
    }
}
