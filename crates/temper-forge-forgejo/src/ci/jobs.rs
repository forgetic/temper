//! Mapping Forgejo provider runs/jobs into portable [`CiJob`]s and applying
//! query-driven deterministic sorting.

use crate::ci_match::{
    Target, payload_pr_head_sha, run_created, run_pr_number, run_updated, sha_matches,
};
use crate::ids::{CiJobCoord, RepoCoord, format_ci_job_id, format_pull_request_id};
use crate::types::{ActionJobDto, ActionRunDto};
use chrono::{DateTime, Utc};
use std::cmp::Ordering;
use temper_forge_model::{
    CiJob, CiJobConclusion, CiJobQuery, CiJobSortField, CiJobStatus, ItemNumber, PullRequestId,
    RepositoryId, SortDirection, sanitize_ci_provider_evidence,
};

/// Maps a Forgejo status string to a portable status/conclusion pair.
#[cfg(test)]
pub(crate) fn map_status(status: &str) -> (CiJobStatus, Option<CiJobConclusion>) {
    map_status_evidence(status, "", "", "", "", "")
}

/// Maps separate job and run state/conclusion/reason fields.
///
/// Forgejo 16 jobs expose only `status`; they do not expose a detailed job
/// conclusion or reason. A status-only `failure` therefore remains terminal
/// `Unknown`, preserving the interrupted-run recovery behavior. Aggregate run
/// evidence may terminalize a generic/absent job state but cannot turn an
/// ambiguous failure into ordinary source/test failure.
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

    // An unrecognized, non-terminal value is conservatively still waiting.
    (CiJobStatus::Queued, None)
}

fn category_from_evidence(status: &str, conclusion: &str, reason: &str) -> Option<CiJobConclusion> {
    let explicit_conclusion = !conclusion.trim().is_empty();
    let primary = if explicit_conclusion {
        terminal_category(conclusion)
    } else {
        terminal_category(status)
    };
    let specific_reason = terminal_category(reason).filter(|category| {
        !matches!(
            category,
            CiJobConclusion::Failure | CiJobConclusion::Unknown
        )
    });
    match primary {
        Some(CiJobConclusion::Failure | CiJobConclusion::Unknown) if explicit_conclusion => {
            specific_reason.or(primary)
        }
        // A bare failure is not ordinary source/test-failure evidence. Forgejo
        // used this exact shape when run #591 lost its runner.
        Some(CiJobConclusion::Failure | CiJobConclusion::Unknown) => {
            specific_reason.or(Some(CiJobConclusion::Unknown))
        }
        Some(_) => primary,
        None => specific_reason,
    }
}

fn conservative_parent_category(category: Option<CiJobConclusion>) -> CiJobConclusion {
    match category {
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

/// Builds a portable job from a strictly validated provider run/job pair.
///
/// Commit, pull, timestamp, and URL evidence comes only from `run`. The query
/// target is used solely to re-check explicit commit ownership; its values are
/// never copied into the returned job.
pub(super) fn job_to_ci_job(
    repo: &RepoCoord,
    repo_id: &RepositoryId,
    run: &ActionRunDto,
    job: &ActionJobDto,
    target: &Target,
) -> Option<CiJob> {
    let (status, conclusion) = map_status_evidence(
        &job.status,
        "",
        "",
        &run.status,
        &run.conclusion,
        &run.reason,
    );
    let commit_sha = provider_commit_sha(run, target)?;
    let pull_request_id: Option<PullRequestId> =
        run_pr_number(run).map(|number| format_pull_request_id(repo, ItemNumber::new(number)));
    let url = first_non_empty(&[&run.html_url, &run.url]);

    let created_at = run_created(run).unwrap_or_else(epoch);
    let updated_at = run_updated(run).unwrap_or(created_at);
    let completed_at = (status == CiJobStatus::Completed).then_some(updated_at);
    let provider_conclusion = (status == CiJobStatus::Completed)
        .then(|| terminal_evidence(job, run))
        .flatten();
    let provider_reason = (status == CiJobStatus::Completed)
        .then(|| sanitize_ci_provider_evidence(&run.reason))
        .flatten();

    Some(CiJob {
        id: format_ci_job_id(&CiJobCoord {
            repo: repo.clone(),
            run_id: run.id,
            job_id: job.id,
            attempt: job.attempt,
            task_id: job.task_id,
        }),
        repo_id: repo_id.clone(),
        pull_request_id,
        commit_sha,
        name: job.name.clone(),
        status,
        conclusion,
        provider_conclusion,
        provider_reason,
        run_id: Some(run.id.to_string()),
        attempt: Some(job.attempt.to_string()),
        url,
        created_at,
        started_at: None,
        completed_at,
        updated_at,
    })
}

fn terminal_evidence(job: &ActionJobDto, run: &ActionRunDto) -> Option<String> {
    let job_status = (!generic_terminal(&normalize(&job.status)))
        .then_some(job.status.as_str())
        .unwrap_or_default();
    first_evidence(&[job_status, &run.conclusion, &run.status, &job.status])
}

fn first_evidence(values: &[&str]) -> Option<String> {
    values
        .iter()
        .find_map(|value| sanitize_ci_provider_evidence(value))
}

/// Selects commit evidence from the already-matched provider run.
fn provider_commit_sha(run: &ActionRunDto, target: &Target) -> Option<String> {
    let payload_sha = payload_pr_head_sha(run);
    let candidates = [
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

fn epoch() -> DateTime<Utc> {
    DateTime::<Utc>::from_timestamp(0, 0).expect("unix epoch is a valid timestamp")
}

/// Sorts jobs by the requested order, then stable provider-backed opaque id.
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

    #[test]
    fn status_mapping_covers_every_terminal_category_conservatively() {
        let cases = [
            ("success", CiJobConclusion::Success),
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
        assert_eq!(
            map_status("failure"),
            (CiJobStatus::Completed, Some(CiJobConclusion::Unknown)),
            "a bare Forgejo failure status is ambiguous"
        );
        assert_eq!(
            map_status_evidence("failure", "failure", "", "", "", ""),
            (CiJobStatus::Completed, Some(CiJobConclusion::Failure)),
            "an explicit provider conclusion preserves ordinary failure"
        );
        assert_eq!(map_status("running"), (CiJobStatus::Running, None));
        assert_eq!(map_status("queued"), (CiJobStatus::Queued, None));
        assert_eq!(map_status("mystery"), (CiJobStatus::Queued, None));
        assert_eq!(
            map_status_evidence("completed", "mystery", "", "completed", "", ""),
            (CiJobStatus::Completed, Some(CiJobConclusion::Unknown))
        );
        assert_eq!(
            map_status_evidence("", "", "", "completed", "runner_lost", ""),
            (CiJobStatus::Completed, Some(CiJobConclusion::RunnerLost))
        );
        assert_eq!(
            map_status_evidence("", "", "", "failure", "", ""),
            (CiJobStatus::Completed, Some(CiJobConclusion::Unknown))
        );
    }

    #[test]
    fn mapping_uses_only_provider_run_ownership_and_identity() {
        let repo = RepoCoord::new("acme", "widgets");
        let repo_id = RepositoryId::new("forgejo:acme/widgets");
        let run = ActionRunDto {
            id: 900,
            index_in_repo: 10,
            run_number: 10,
            prettyref: "#7".to_string(),
            head_sha: "provider-sha".to_string(),
            status: "success".to_string(),
            ..Default::default()
        };
        let job = ActionJobDto {
            id: 31,
            run_id: 900,
            attempt: 2,
            task_id: 44,
            name: "build".to_string(),
            status: "success".to_string(),
        };
        let mapped = job_to_ci_job(&repo, &repo_id, &run, &job, &Target::default()).unwrap();
        assert_eq!(mapped.commit_sha, "provider-sha");
        assert_eq!(
            mapped.id.as_str(),
            "forgejo:acme/widgets:actions:900:31:2:44"
        );
        assert_eq!(
            mapped.pull_request_id.unwrap().as_str(),
            "forgejo:acme/widgets:pull:7"
        );
        assert_eq!(mapped.run_id.as_deref(), Some("900"));
        assert_eq!(mapped.attempt.as_deref(), Some("2"));
    }
}
