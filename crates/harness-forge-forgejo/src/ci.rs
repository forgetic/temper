// SPDX-License-Identifier: MPL-2.0
//! CI job listing and lookup for the Forgejo backend.
//!
//! Forgejo Actions exposes workflow *runs* and *tasks*. This module adapts that
//! surface to the portable [`CiJob`] model: it matches runs to a pull request or
//! commit (see [`crate::ci_match`]), groups a run's tasks into attempts (the
//! latest attempt wins), and maps each task into a job. Log fetching is
//! intentionally out of scope; the portable trait only needs structured status.
use serde::de::DeserializeOwned;
use serde_json::Value;

use harness_forge::{
    CiConclusion, CiJob, CiJobId, CiJobQuery, CiJobSort, CiStatus, ForgeError, PullRequestId,
    RepositoryId, Timestamp,
};

use crate::ci_match::{
    match_run, run_created, run_index, run_pr_number, run_updated, sort_runs, Target,
};
use crate::ci_time::opt_timestamp;
use crate::client::ForgejoForge;
use crate::dto::{ForgejoActionRun, ForgejoActionTask, ForgejoPrDetail};
use crate::error::{decode, ensure_status, map_http_error, ApiResult};
use crate::http::{HttpClient, HttpMethod};

impl<C: HttpClient> ForgejoForge<C> {
    /// List CI jobs for a repository, filtered by [`CiJobQuery`].
    pub async fn list_ci_jobs(
        &self,
        repo: &RepositoryId,
        query: &CiJobQuery,
    ) -> ApiResult<Vec<CiJob>> {
        let mut target = Target::default();
        if let Some(pr) = &query.pull_request_id {
            let number = pr.as_str().parse::<i64>().map_err(|_| {
                ForgeError::InvalidRequest(format!("invalid pull request id: {}", pr.as_str()))
            })?;
            target.pr_id = Some(pr.clone());
            target.pr_number = Some(number);
            let pr_dto = self.fetch_pull_request_dto(repo, number).await?;
            if let Some(head) = pr_dto.head {
                if !head.sha.is_empty() {
                    target.pr_head_sha = Some(head.sha);
                }
                if !head.ref_name.is_empty() {
                    target.pr_head_ref = Some(head.ref_name);
                }
            }
        }
        if let Some(commit) = &query.commit_sha {
            if !commit.is_empty() {
                target.commit_sha = Some(commit.clone());
            }
        }

        let runs = self.fetch_action_runs(repo).await?;
        let mut matched: Vec<ForgejoActionRun> = if target.has_filter() {
            runs.into_iter()
                .filter(|run| match_run(run, &target).is_some())
                .collect()
        } else {
            runs
        };
        if matched.is_empty() {
            return Ok(Vec::new());
        }
        sort_runs(&mut matched);

        let all_tasks = self.fetch_action_tasks(repo).await?;
        let mut jobs = Vec::new();
        for run in &matched {
            let run_tasks: Vec<ForgejoActionTask> = all_tasks
                .iter()
                .filter(|task| task.run_number == run.run_number)
                .cloned()
                .collect();
            let attempts = group_attempts(run_tasks);
            if let Some(latest) = attempts.last() {
                for (index, task) in latest.iter().enumerate() {
                    jobs.push(task_to_job(run, task, index, &target));
                }
            }
        }

        if let Some(status) = query.status {
            jobs.retain(|job| job.status == status);
        }
        sort_jobs(&mut jobs, query.sort);
        Ok(jobs)
    }

    /// Look up a single CI job by its encoded id.
    pub async fn get_ci_job(&self, repo: &RepositoryId, id: &CiJobId) -> ApiResult<Option<CiJob>> {
        let Some((run_index_wanted, task_id_wanted, job_index_wanted)) = decode_job_id(id.as_str())
        else {
            return Err(ForgeError::InvalidRequest(format!(
                "invalid CI job id: {}",
                id.as_str()
            )));
        };

        let runs = self.fetch_action_runs(repo).await?;
        let Some(run) = runs.into_iter().find(|run| run_index(run) == run_index_wanted) else {
            return Ok(None);
        };

        let all_tasks = self.fetch_action_tasks(repo).await?;
        let run_tasks: Vec<ForgejoActionTask> = all_tasks
            .into_iter()
            .filter(|task| task.run_number == run.run_number)
            .collect();
        let attempts = group_attempts(run_tasks);
        let Some(latest) = attempts.last() else {
            return Ok(None);
        };

        let target = Target::default();
        // Prefer an exact index + task-id match; fall back to task id alone in
        // case the attempt enumeration shifted between calls.
        if let Some(task) = latest.get(job_index_wanted) {
            if task.id == task_id_wanted {
                return Ok(Some(task_to_job(&run, task, job_index_wanted, &target)));
            }
        }
        for (index, task) in latest.iter().enumerate() {
            if task.id == task_id_wanted {
                return Ok(Some(task_to_job(&run, task, index, &target)));
            }
        }
        Ok(None)
    }

    /// Fetch a minimal pull-request detail for head SHA/ref resolution.
    async fn fetch_pull_request_dto(
        &self,
        repo: &RepositoryId,
        number: i64,
    ) -> ApiResult<ForgejoPrDetail> {
        let url = format!(
            "{}/repos/{}/pulls/{}",
            self.base_url(),
            repo.as_str(),
            number
        );
        let request = self.build_request(HttpMethod::Get, url, None);
        let response = self.http().send(request).await.map_err(map_http_error)?;
        ensure_status(&response, &[200], "get pull request for CI")?;
        decode(&response)
    }

    /// List Forgejo Actions runs for the repository.
    async fn fetch_action_runs(&self, repo: &RepositoryId) -> ApiResult<Vec<ForgejoActionRun>> {
        let url = format!(
            "{}/repos/{}/actions/runs?limit=200",
            self.base_url(),
            repo.as_str()
        );
        let request = self.build_request(HttpMethod::Get, url, None);
        let response = self.http().send(request).await.map_err(map_http_error)?;
        ensure_actions_status(response.status, "list Forgejo Actions runs")?;
        extract_array(&response.body, &["workflow_runs", "runs"])
    }

    /// List Forgejo Actions tasks for the repository.
    async fn fetch_action_tasks(&self, repo: &RepositoryId) -> ApiResult<Vec<ForgejoActionTask>> {
        let url = format!(
            "{}/repos/{}/actions/tasks?limit=200",
            self.base_url(),
            repo.as_str()
        );
        let request = self.build_request(HttpMethod::Get, url, None);
        let response = self.http().send(request).await.map_err(map_http_error)?;
        ensure_actions_status(response.status, "list Forgejo Actions tasks")?;
        extract_array(&response.body, &["workflow_runs", "tasks"])
    }
}

/// Map an Actions endpoint status, treating 403/404 as an unavailable backend.
///
/// CI must never be silently reported as passed/failed, so absent Actions
/// support surfaces as an error and keeps the runner's merge gate closed.
fn ensure_actions_status(status: u16, context: &str) -> ApiResult<()> {
    match status {
        200 => Ok(()),
        403 | 404 => Err(ForgeError::Backend(format!(
            "{context}: Forgejo Actions unavailable (status {status})"
        ))),
        other => Err(ForgeError::Backend(format!(
            "{context}: unexpected status {other}"
        ))),
    }
}

/// Tolerantly decode a JSON array that may be bare or wrapped in an object.
fn extract_array<T: DeserializeOwned>(body: &[u8], keys: &[&str]) -> ApiResult<Vec<T>> {
    let first = body.iter().find(|byte| !byte.is_ascii_whitespace());
    if first == Some(&b'[') {
        return serde_json::from_slice::<Vec<T>>(body)
            .map_err(|err| ForgeError::Backend(format!("failed to decode Actions array: {err}")));
    }
    let value: Value = serde_json::from_slice(body)
        .map_err(|err| ForgeError::Backend(format!("failed to decode Actions response: {err}")))?;
    for key in keys {
        if let Some(array) = value.get(*key) {
            if array.is_null() {
                continue;
            }
            return serde_json::from_value(array.clone()).map_err(|err| {
                ForgeError::Backend(format!("failed to decode Actions `{key}`: {err}"))
            });
        }
    }
    Ok(Vec::new())
}

/// Group a run's tasks into attempts: a repeated task name starts a new attempt.
fn group_attempts(mut tasks: Vec<ForgejoActionTask>) -> Vec<Vec<ForgejoActionTask>> {
    tasks.sort_by_key(|task| task.id);
    let mut attempts: Vec<Vec<ForgejoActionTask>> = Vec::new();
    let mut current: Vec<ForgejoActionTask> = Vec::new();
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

/// Map a Forgejo status string to a portable status/conclusion pair.
fn map_status(status: &str) -> (CiStatus, Option<CiConclusion>) {
    match status.to_ascii_lowercase().as_str() {
        "success" => (CiStatus::Completed, Some(CiConclusion::Success)),
        "failure" => (CiStatus::Completed, Some(CiConclusion::Failure)),
        "cancelled" | "canceled" => (CiStatus::Completed, Some(CiConclusion::Cancelled)),
        "skipped" => (CiStatus::Completed, Some(CiConclusion::Skipped)),
        "timeout" | "timed_out" => (CiStatus::Completed, Some(CiConclusion::TimedOut)),
        "running" | "in_progress" => (CiStatus::Running, None),
        "waiting" | "queued" | "blocked" | "pending" => (CiStatus::Queued, None),
        _ => (CiStatus::Queued, None),
    }
}

/// Build a portable job from a run/task pair at a given attempt index.
fn task_to_job(
    run: &ForgejoActionRun,
    task: &ForgejoActionTask,
    job_index: usize,
    target: &Target,
) -> CiJob {
    let (status, conclusion) = map_status(&task.status);
    let commit_sha = first_non_empty(&[
        &task.commit_sha,
        &task.head_sha,
        &run.commit_sha,
        &run.head_sha,
    ])
    .or_else(|| target.pr_head_sha.clone().filter(|sha| !sha.is_empty()))
    .or_else(|| target.commit_sha.clone().filter(|sha| !sha.is_empty()));

    let pull_request_id = target
        .pr_id
        .clone()
        .or_else(|| run_pr_number(run).map(|number| PullRequestId::new(number.to_string())));

    let name = if task.name.is_empty() {
        format!("job-{job_index}")
    } else {
        task.name.clone()
    };
    let url = first_non_empty(&[&task.html_url, &task.url, &run.html_url, &run.url]);
    let created_at = task_created(task).or_else(|| run_created(run));
    let updated_at = task_updated(task).or_else(|| run_updated(run));

    CiJob {
        id: CiJobId(encode_job_id(run_index(run), task.id, job_index)),
        name,
        status,
        conclusion,
        commit_sha,
        pull_request_id,
        url,
        created_at,
        updated_at,
    }
}

fn first_non_empty(values: &[&str]) -> Option<String> {
    values
        .iter()
        .find(|value| !value.is_empty())
        .map(|value| value.to_string())
}

fn encode_job_id(run: i64, task: i64, job: usize) -> String {
    format!("run/{run}/task/{task}/job/{job}")
}

fn decode_job_id(id: &str) -> Option<(i64, i64, usize)> {
    let parts: Vec<&str> = id.split('/').collect();
    if parts.len() != 6 || parts[0] != "run" || parts[2] != "task" || parts[4] != "job" {
        return None;
    }
    let run = parts[1].parse().ok()?;
    let task = parts[3].parse().ok()?;
    let job = parts[5].parse().ok()?;
    Some((run, task, job))
}

fn task_created(task: &ForgejoActionTask) -> Option<Timestamp> {
    opt_timestamp(&task.created_at).or_else(|| opt_timestamp(&task.created))
}

fn task_updated(task: &ForgejoActionTask) -> Option<Timestamp> {
    opt_timestamp(&task.updated_at).or_else(|| opt_timestamp(&task.updated))
}

/// Sort jobs by the requested order, mirroring the reference backends.
fn sort_jobs(jobs: &mut [CiJob], sort: Option<CiJobSort>) {
    match sort {
        Some(CiJobSort::Name) => {
            jobs.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.0.cmp(&b.id.0)));
        }
        Some(CiJobSort::CreatedDesc) => {
            jobs.sort_by(|a, b| b.created_at.cmp(&a.created_at).then(a.id.0.cmp(&b.id.0)));
        }
        Some(CiJobSort::UpdatedDesc) => {
            jobs.sort_by(|a, b| b.updated_at.cmp(&a.updated_at).then(a.id.0.cmp(&b.id.0)));
        }
        None => {
            jobs.sort_by(|a, b| a.id.0.cmp(&b.id.0));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_attempts_by_repeated_name() {
        let mk = |id: i64, name: &str| ForgejoActionTask {
            id,
            run_number: 1,
            name: name.to_string(),
            status: "success".to_string(),
            ..Default::default()
        };
        let tasks = vec![mk(1, "build"), mk(2, "test"), mk(3, "build"), mk(4, "test")];
        let attempts = group_attempts(tasks);
        assert_eq!(attempts.len(), 2);
        assert_eq!(attempts[1][0].id, 3);
        assert_eq!(attempts[1][1].id, 4);
    }

    #[test]
    fn status_mapping_covers_all_states() {
        assert_eq!(
            map_status("success"),
            (CiStatus::Completed, Some(CiConclusion::Success))
        );
        assert_eq!(
            map_status("failure"),
            (CiStatus::Completed, Some(CiConclusion::Failure))
        );
        assert_eq!(
            map_status("cancelled"),
            (CiStatus::Completed, Some(CiConclusion::Cancelled))
        );
        assert_eq!(
            map_status("skipped"),
            (CiStatus::Completed, Some(CiConclusion::Skipped))
        );
        assert_eq!(map_status("running"), (CiStatus::Running, None));
        assert_eq!(map_status("waiting"), (CiStatus::Queued, None));
    }

    #[test]
    fn encodes_and_decodes_job_id() {
        let id = encode_job_id(12, 34, 1);
        assert_eq!(id, "run/12/task/34/job/1");
        assert_eq!(decode_job_id(&id), Some((12, 34, 1)));
        assert_eq!(decode_job_id("nonsense"), None);
    }
}
