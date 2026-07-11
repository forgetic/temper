//! Mapping Forgejo runs/tasks into portable [`CiJob`]s, plus attempt grouping
//! and the query-driven job sort.

use crate::ci_match::{
    Target, payload_pr_head_sha, run_created, run_index, run_pr_number, run_updated, sha_matches,
};
use crate::ids::{CiJobCoord, RepoCoord, format_ci_job_id, format_pull_request_id};
use crate::types::{ActionRunDto, ActionTaskDto};
use chrono::{DateTime, Utc};
use std::cmp::Ordering;
use temper_forge_model::{
    CiJob, CiJobConclusion, CiJobQuery, CiJobSortField, CiJobStatus, ItemNumber, PullRequestId,
    RepositoryId, SortDirection,
};

/// Returns the latest attempt's tasks for a run, ordered by canonical task id.
///
/// Tasks are tied to a run by `run_number == run_index`, sorted by monotonic id,
/// then split into attempts: a repeated task name starts a new attempt.
pub(super) fn latest_attempt(tasks: &[ActionTaskDto], run: u64) -> Vec<ActionTaskDto> {
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
pub(super) fn task_to_job(
    repo: &RepoCoord,
    repo_id: &RepositoryId,
    run: &ActionRunDto,
    task: &ActionTaskDto,
    job_index: u64,
    target: &Target,
) -> Option<CiJob> {
    let (status, conclusion) = map_status(&task.status);
    let commit_sha = provider_commit_sha(task, run, target)?;

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

    Some(CiJob {
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
    })
}

/// Selects provider-supplied commit evidence for a mapped task.
///
/// Task fields are preferred for PR-only diagnostics. With an explicit commit,
/// any task/run/payload SHA may prove ownership, but one must safely match the
/// query; target values are never copied into a job as synthetic evidence.
fn provider_commit_sha(
    task: &ActionTaskDto,
    run: &ActionRunDto,
    target: &Target,
) -> Option<String> {
    let payload_sha = payload_pr_head_sha(run);
    let candidates = [
        task.commit_sha.as_str(),
        task.head_sha.as_str(),
        run.commit_sha.as_str(),
        run.head_sha.as_str(),
        payload_sha.as_deref().unwrap_or_default(),
    ];
    if let Some(commit) = target.explicit_commit() {
        return candidates
            .into_iter()
            .find(|candidate| sha_matches(candidate, commit))
            .map(str::to_string);
    }
    first_non_empty(&candidates)
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

/// Sorts jobs by the requested order, mirroring the reference backends.
pub(super) fn sort_jobs(jobs: &mut [CiJob], query: &CiJobQuery) {
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
}
