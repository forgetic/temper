//! Mapping a run's live-view jobs into portable [`CiJob`]s.

use super::dto::LiveRunDto;
use crate::ci::map_status_evidence;
use crate::ci_match::{Target, sha_matches};
use crate::ids::{CiJobCoord, RepoCoord, format_ci_job_id};
use chrono::{DateTime, Utc};
use temper_forge_model::{
    CiJob, CiJobStatus, PullRequestId, RepositoryId, sanitize_ci_provider_evidence,
};

/// Maps a run's live-view jobs to portable [`CiJob`]s.
pub(super) fn live_run_to_jobs(
    repo: &RepoCoord,
    repo_id: &RepositoryId,
    run: u64,
    live: &LiveRunDto,
    target: &Target,
) -> Vec<CiJob> {
    let commit_sha = live.commit.short_sha.trim().to_string();
    if commit_sha.is_empty()
        || target
            .explicit_commit()
            .is_some_and(|commit| !sha_matches(&commit_sha, commit))
    {
        return Vec::new();
    }
    let pull_request_id: Option<PullRequestId> = target.pr_id.clone();

    live.jobs
        .iter()
        .enumerate()
        .map(|(index, job)| {
            let (status, conclusion) = map_status_evidence(
                &job.status,
                &job.conclusion,
                &job.reason,
                &live.status,
                &live.conclusion,
                &live.reason,
            );
            let name = if job.name.is_empty() {
                format!("job-{index}")
            } else {
                job.name.clone()
            };
            let coord = CiJobCoord {
                repo: repo.clone(),
                run,
                job_index: index as u64,
                // The web UI exposes no stable task id; the run id is the page's
                // job coordinate, so reuse it as the encoded task id.
                task_id: run,
            };
            // The live view exposes no per-job timestamp, but CI runs are created
            // in execution order, so the run id is monotonic in time. Derive
            // `created_at`/`updated_at` from it (epoch + run seconds) so the
            // portable ordering by `created_at` reflects "older run before newer".
            // This is what lets `CiStatus::from_jobs` pick the latest run per job
            // (the merge gate) and the `ci_fails_then_passes` assert read the
            // failing run before the fixed, passing one — both keyed on real
            // ordering rather than a shared epoch that would tie every run.
            let run_time = run_ordering_time(run);
            let completed_at = (status == CiJobStatus::Completed).then_some(run_time);
            let provider_conclusion = (status == CiJobStatus::Completed)
                .then(|| {
                    let job_status = (!matches!(
                        job.status.trim().to_ascii_lowercase().as_str(),
                        "completed" | "complete" | "done" | "finished"
                    ))
                    .then_some(job.status.as_str())
                    .unwrap_or_default();
                    [
                        job.conclusion.as_str(),
                        job_status,
                        live.conclusion.as_str(),
                        live.status.as_str(),
                        job.status.as_str(),
                    ]
                    .into_iter()
                    .find_map(sanitize_ci_provider_evidence)
                })
                .flatten();
            let provider_reason = (status == CiJobStatus::Completed)
                .then(|| {
                    [job.reason.as_str(), live.reason.as_str()]
                        .into_iter()
                        .find_map(sanitize_ci_provider_evidence)
                })
                .flatten();
            CiJob {
                id: format_ci_job_id(&coord),
                repo_id: repo_id.clone(),
                pull_request_id: pull_request_id.clone(),
                commit_sha: commit_sha.clone(),
                name,
                status,
                conclusion,
                provider_conclusion,
                provider_reason,
                run_id: Some(run.to_string()),
                // The compatibility route is explicitly attempt-qualified with
                // attempt 1; newer live payloads may expose a later attempt.
                attempt: Some(live.attempt.max(1).to_string()),
                url: None,
                created_at: run_time,
                started_at: None,
                completed_at,
                updated_at: run_time,
            }
        })
        .collect()
}

/// A monotonic ordering timestamp derived from a run id.
///
/// The web UI exposes no per-job timestamp; run ids increase with creation time,
/// so `epoch + run_id` seconds gives a stable, run-ordered timestamp. It is an
/// **ordering** key, not a wall-clock truth — only the relative order of runs is
/// meaningful (newer run ⇒ later timestamp).
fn run_ordering_time(run: u64) -> DateTime<Utc> {
    DateTime::<Utc>::from_timestamp(run as i64, 0).unwrap_or_else(epoch)
}

/// The unix epoch fallback.
fn epoch() -> DateTime<Utc> {
    DateTime::<Utc>::from_timestamp(0, 0).expect("unix epoch is a valid timestamp")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ci_ui::dto::{LiveBranchDto, LiveCommitDto, LiveJobDto, LiveRunDto};
    use temper_forge_model::{CiJobConclusion, MAX_CI_PROVIDER_EVIDENCE_BYTES};

    #[test]
    fn live_run_maps_jobs_to_portable_status() {
        let repo = RepoCoord::new("acme", "widgets");
        let repo_id = RepositoryId::new("forgejo:acme/widgets");
        let live = LiveRunDto {
            status: "failure".to_string(),
            conclusion: String::new(),
            reason: "runner returned\nexit 1".to_string(),
            attempt: 2,
            jobs: vec![
                LiveJobDto {
                    name: "build".to_string(),
                    status: "failure".to_string(),
                    ..Default::default()
                },
                LiveJobDto {
                    name: String::new(),
                    status: "running".to_string(),
                    ..Default::default()
                },
            ],
            commit: LiveCommitDto {
                short_sha: "c456eec18b".to_string(),
                branch: LiveBranchDto {
                    name: "main".to_string(),
                },
            },
        };
        let target = Target {
            pr_id: Some(PullRequestId::new("forgejo:acme/widgets:pull:7")),
            ..Default::default()
        };
        let jobs = live_run_to_jobs(&repo, &repo_id, 1, &live, &target);
        assert_eq!(jobs.len(), 2);
        assert_eq!(jobs[0].name, "build");
        assert_eq!(jobs[0].status, CiJobStatus::Completed);
        assert_eq!(jobs[0].conclusion, Some(CiJobConclusion::Failure));
        assert_eq!(jobs[0].provider_conclusion.as_deref(), Some("failure"));
        assert_eq!(
            jobs[0].provider_reason.as_deref(),
            Some("runner returned exit 1")
        );
        assert!(jobs[0].provider_reason.as_ref().unwrap().len() <= MAX_CI_PROVIDER_EVIDENCE_BYTES);
        assert_eq!(jobs[0].run_id.as_deref(), Some("1"));
        assert_eq!(jobs[0].attempt.as_deref(), Some("2"));
        assert_eq!(jobs[0].id.as_str(), "forgejo:acme/widgets:actions:1:0:1");
        assert_eq!(jobs[0].commit_sha, "c456eec18b");
        assert_eq!(
            jobs[0].pull_request_id.as_ref().unwrap().as_str(),
            "forgejo:acme/widgets:pull:7"
        );
        // An explicit non-terminal per-job state wins over the terminal parent;
        // providers may expose a partially updated run while jobs still move.
        assert_eq!(jobs[1].name, "job-1");
        assert_eq!(jobs[1].status, CiJobStatus::Running);
        assert_eq!(jobs[1].conclusion, None);
        assert_eq!(jobs[1].provider_conclusion, None);
    }
}
