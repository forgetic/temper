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
    RepositoryId, SortDirection, sanitize_ci_provider_evidence,
};

/// Latest task set plus its one-based attempt ordinal.
pub(super) struct LatestAttempt {
    pub(super) tasks: Vec<ActionTaskDto>,
    pub(super) ordinal: u64,
}

/// Returns the latest attempt's tasks for a run, ordered by canonical task id.
///
/// Tasks are tied to a run by `run_number == run_index`, sorted by monotonic id,
/// then split into attempts: a repeated task name starts a new attempt.
pub(super) fn latest_attempt(tasks: &[ActionTaskDto], run: u64) -> LatestAttempt {
    let run_tasks: Vec<ActionTaskDto> = tasks
        .iter()
        .filter(|task| task.run_number == run)
        .cloned()
        .collect();
    let mut attempts = group_attempts(run_tasks);
    let ordinal = attempts.len() as u64;
    LatestAttempt {
        tasks: attempts.pop().unwrap_or_default(),
        ordinal,
    }
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
#[cfg(test)]
pub(crate) fn map_status(status: &str) -> (CiJobStatus, Option<CiJobConclusion>) {
    map_status_evidence(status, "", "", "", "", "")
}

/// Maps separate job and run state/conclusion/reason fields.
///
/// Job-specific terminal evidence wins. A terminal run makes an absent or
/// unrecognized job state terminal too, but a run-level ordinary failure is
/// deliberately projected as `Unknown`: an aggregate failure does not prove
/// that this job reported a source/test failure. Explicit run-wide categories
/// such as cancellation or runner loss can be retained without that inference.
pub(crate) fn map_status_evidence(
    status: &str,
    conclusion: &str,
    reason: &str,
    parent_status: &str,
    parent_conclusion: &str,
    parent_reason: &str,
) -> (CiJobStatus, Option<CiJobConclusion>) {
    let state = normalize(status);
    if let Some(category) = category_from_evidence(status, conclusion, reason) {
        return (CiJobStatus::Completed, Some(category));
    }
    if !conclusion.trim().is_empty() {
        return (CiJobStatus::Completed, Some(CiJobConclusion::Unknown));
    }
    if generic_terminal(&state) {
        let parent_category =
            category_from_evidence(parent_status, parent_conclusion, parent_reason);
        return (
            CiJobStatus::Completed,
            Some(conservative_parent_category(parent_category)),
        );
    }

    if matches!(state.as_str(), "running" | "in_progress" | "in-progress") {
        return (CiJobStatus::Running, None);
    }
    if matches!(
        state.as_str(),
        "waiting" | "queued" | "requested" | "blocked" | "pending"
    ) {
        return (CiJobStatus::Queued, None);
    }

    let parent_state = normalize(parent_status);
    let parent_category = category_from_evidence(parent_status, parent_conclusion, parent_reason);
    if !parent_conclusion.trim().is_empty()
        || generic_terminal(&parent_state)
        || parent_category.is_some()
    {
        return (
            CiJobStatus::Completed,
            Some(conservative_parent_category(parent_category)),
        );
    }

    // Forgejo has accumulated several provider spellings across versions. An
    // unrecognized, non-terminal value is conservatively still waiting.
    (CiJobStatus::Queued, None)
}

fn category_from_evidence(status: &str, conclusion: &str, reason: &str) -> Option<CiJobConclusion> {
    let conclusion_present = !conclusion.trim().is_empty();
    let primary = if conclusion_present {
        terminal_category(conclusion)
    } else {
        terminal_category(status)
    };
    let reason = terminal_category(reason);
    match primary {
        // A broad failure/unknown plus a machine-readable terminal reason is an
        // explicit provider fact. Arbitrary human prose never enters this map.
        Some(CiJobConclusion::Failure | CiJobConclusion::Unknown) => reason
            .filter(|category| {
                !matches!(
                    category,
                    CiJobConclusion::Failure | CiJobConclusion::Unknown
                )
            })
            .or(primary),
        Some(_) => primary,
        // An unrecognized explicit conclusion did not fall back to the broad
        // status above; only a recognized reason may refine it.
        None => reason,
    }
}

fn conservative_parent_category(category: Option<CiJobConclusion>) -> CiJobConclusion {
    match category {
        // A run-level failure is aggregate evidence only. It does not say which
        // job, if any, supplied ordinary source/test-failure evidence.
        Some(CiJobConclusion::Failure) | None => CiJobConclusion::Unknown,
        Some(category) => category,
    }
}

fn generic_terminal(status: &str) -> bool {
    matches!(status, "completed" | "complete" | "done" | "finished")
}

fn terminal_category(value: &str) -> Option<CiJobConclusion> {
    match normalize(value).as_str() {
        "success" => Some(CiJobConclusion::Success),
        "failure" | "failed" => Some(CiJobConclusion::Failure),
        "cancelled" | "canceled" => Some(CiJobConclusion::Cancelled),
        "interrupted" => Some(CiJobConclusion::Interrupted),
        "timeout" | "timed_out" | "timed-out" => Some(CiJobConclusion::TimedOut),
        "runner_lost" | "runner-lost" => Some(CiJobConclusion::RunnerLost),
        "startup_failure" | "startup-failure" => Some(CiJobConclusion::StartupFailure),
        "action_required" | "action-required" => Some(CiJobConclusion::ActionRequired),
        "neutral" => Some(CiJobConclusion::Neutral),
        "skipped" => Some(CiJobConclusion::Skipped),
        "unknown" => Some(CiJobConclusion::Unknown),
        _ => None,
    }
}

fn normalize(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

/// Builds a portable job from a run/task pair at a given attempt index.
pub(super) fn task_to_job(
    repo: &RepoCoord,
    repo_id: &RepositoryId,
    run: &ActionRunDto,
    task: &ActionTaskDto,
    job_index: u64,
    attempt_ordinal: u64,
    target: &Target,
) -> Option<CiJob> {
    let (status, conclusion) = map_status_evidence(
        &task.status,
        &task.conclusion,
        &task.reason,
        &run.status,
        &run.conclusion,
        &run.reason,
    );
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
    let provider_conclusion = (status == CiJobStatus::Completed)
        .then(|| terminal_evidence(task, run))
        .flatten();
    let provider_reason = (status == CiJobStatus::Completed)
        .then(|| first_evidence(&[&task.reason, &run.reason]))
        .flatten();
    let provider_run = if run.id > 0 { run.id } else { run_index(run) };
    let provider_attempt = [task.attempt, run.attempt, attempt_ordinal]
        .into_iter()
        .find(|value| *value > 0);

    Some(CiJob {
        id: format_ci_job_id(&coord),
        repo_id: repo_id.clone(),
        pull_request_id,
        commit_sha,
        name,
        status,
        conclusion,
        provider_conclusion,
        provider_reason,
        run_id: (provider_run > 0).then(|| provider_run.to_string()),
        attempt: provider_attempt.map(|attempt| attempt.to_string()),
        url,
        created_at,
        started_at,
        completed_at,
        updated_at,
    })
}

fn terminal_evidence(task: &ActionTaskDto, run: &ActionRunDto) -> Option<String> {
    let task_status = (!matches!(
        normalize(&task.status).as_str(),
        "completed" | "complete" | "done" | "finished"
    ))
    .then_some(task.status.as_str())
    .unwrap_or_default();
    first_evidence(&[
        &task.conclusion,
        task_status,
        &run.conclusion,
        &run.status,
        &task.status,
    ])
}

fn first_evidence(values: &[&str]) -> Option<String> {
    values
        .iter()
        .find_map(|value| sanitize_ci_provider_evidence(value))
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
        assert_eq!(latest.ordinal, 2);
        assert_eq!(latest.tasks.len(), 1);
        assert_eq!(latest.tasks[0].id, 2);
    }

    #[test]
    fn status_mapping_covers_every_terminal_category() {
        let cases = [
            ("success", CiJobConclusion::Success),
            ("failure", CiJobConclusion::Failure),
            ("cancelled", CiJobConclusion::Cancelled),
            ("interrupted", CiJobConclusion::Interrupted),
            ("timed_out", CiJobConclusion::TimedOut),
            ("runner_lost", CiJobConclusion::RunnerLost),
            ("startup_failure", CiJobConclusion::StartupFailure),
            ("action_required", CiJobConclusion::ActionRequired),
            ("neutral", CiJobConclusion::Neutral),
            ("skipped", CiJobConclusion::Skipped),
            ("unknown", CiJobConclusion::Unknown),
        ];
        for (raw, expected) in cases {
            assert_eq!(
                map_status(raw),
                (CiJobStatus::Completed, Some(expected)),
                "raw status {raw}"
            );
        }
        assert_eq!(map_status("running"), (CiJobStatus::Running, None));
        assert_eq!(map_status("queued"), (CiJobStatus::Queued, None));
        assert_eq!(map_status("mystery"), (CiJobStatus::Queued, None));
        assert_eq!(
            map_status_evidence("completed", "mystery", "", "completed", "", ""),
            (CiJobStatus::Completed, Some(CiJobConclusion::Unknown))
        );
        assert_eq!(
            map_status_evidence("", "", "", "completed", "runner_lost", ""),
            (CiJobStatus::Completed, Some(CiJobConclusion::RunnerLost)),
            "an explicit run-wide infrastructure result terminalizes an absent job result"
        );
        assert_eq!(
            map_status_evidence("", "", "", "failure", "", ""),
            (CiJobStatus::Completed, Some(CiJobConclusion::Unknown)),
            "an aggregate run failure does not invent ordinary job-failure evidence"
        );
        assert_eq!(
            map_status_evidence("failure", "mystery", "", "", "", ""),
            (CiJobStatus::Completed, Some(CiJobConclusion::Unknown)),
            "an explicit unrecognized result cannot become ordinary failure"
        );
        assert_eq!(
            map_status_evidence("failure", "", "runner_lost", "", "", ""),
            (CiJobStatus::Completed, Some(CiJobConclusion::RunnerLost)),
            "a machine-readable provider reason refines a broad failure"
        );
    }
}
