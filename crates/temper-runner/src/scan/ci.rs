//! Narrow current-head CI observations for open pull requests.
//!
//! This path exists for latency-sensitive CI monitoring. It is intentionally
//! smaller than a role, automation, or audit scan: only workflow queues with an
//! exact `ci_passed` or `ci_failed` condition contribute positive pull-request
//! candidate labels, and neither issue nor terminal-history buckets are read.

use chrono::{DateTime, Utc};
use temper_forge::{
    CiJobQuery, Forge, ItemListDetails, ItemNumber, PullRequestState, RepositoryId,
};
use temper_workflow::plan::matches_queue_cheap;
use temper_workflow::{
    CiState, CiStatus, ClassifiedArtifact, Classifier, CompiledWorkflow, QueueManifest,
    ValidatedWorkflow, requires_human_attention,
};

use super::ScanError;
use super::candidate::{ci_candidate_query_plan, ci_pull_request_queues};

/// Current-head native CI state for one open, CI-gated pull request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CiStatusObservation {
    /// Repository-scoped pull-request number.
    pub pull_request_number: ItemNumber,
    /// Non-empty current pull-request head SHA.
    pub head_sha: String,
    /// Whether at least one job identifies the exact current pull-request head.
    ///
    /// This distinguishes an empty (or stale-head-only) result from active
    /// queued/running work without changing the conservative `Pending` gate
    /// state shared by both cases.
    pub current_head_jobs_present: bool,
    /// Aggregate state of the latest current-head job per name.
    pub state: CiState,
    /// Time the complete latest-job set became terminal, when every latest job
    /// supplied a completion timestamp.
    pub completed_at: Option<DateTime<Utc>>,
}

/// Reads current-head CI observations for open pull requests relevant to exact
/// `ci_passed` or `ci_failed` workflow queues.
///
/// Candidate discovery uses one queue-derived open pull-request bucket. Each
/// listed summary is classified and cheap-matched before an exact refresh, and
/// the exact artifact is checked again before its CI jobs are read. Attention-
/// marked, staged, terminal, unclassifiable, unrelated, or headless pull
/// requests are skipped. CI queries are conjunctively scoped to both the pull
/// request and its non-empty head SHA. This function performs no Forge mutation.
pub async fn read_ci_status_observations<F: Forge + ?Sized>(
    forge: &F,
    repo: &RepositoryId,
    workflow: &ValidatedWorkflow,
    compiled: &CompiledWorkflow,
) -> Result<Vec<CiStatusObservation>, ScanError> {
    let queues = ci_pull_request_queues(workflow, compiled);
    if queues.is_empty() {
        return Ok(Vec::new());
    }

    let plan = ci_candidate_query_plan(workflow, compiled);
    let classifier = Classifier::new(workflow);
    let mut candidates = Vec::new();
    for query in plan.pull_request_queries {
        candidates.extend(forge.list_pull_request_candidates(repo, query).await?);
    }
    candidates.sort_by(|left, right| {
        left.number
            .cmp(&right.number)
            .then_with(|| left.id.cmp(&right.id))
    });
    candidates.dedup_by(|left, right| left.id == right.id);

    let mut observations = Vec::new();
    for summary in candidates {
        if summary.state != PullRequestState::Open {
            continue;
        }
        let Ok(classified) = classifier.classify_pull_request(&summary) else {
            continue;
        };
        if !relevant_candidate(&queues, &classified) {
            continue;
        }

        // Forgejo's labelled candidate index intentionally omits branch/SHA
        // details. Refresh only candidates that survived local cheap matching.
        let Some(pull_request) = forge
            .get_pull_request_with_details(&summary.id, ItemListDetails::summary())
            .await?
        else {
            continue;
        };
        if pull_request.state != PullRequestState::Open {
            continue;
        }
        let Ok(classified) = classifier.classify_pull_request(&pull_request) else {
            continue;
        };
        if !relevant_candidate(&queues, &classified) {
            continue;
        }
        let Some(head_sha) = pull_request
            .head_sha
            .as_deref()
            .map(str::trim)
            .filter(|head_sha| !head_sha.is_empty())
            .map(str::to_owned)
        else {
            continue;
        };

        let mut jobs = forge
            .list_ci_jobs(
                repo,
                CiJobQuery {
                    pull_request_id: Some(pull_request.id),
                    commit_sha: Some(head_sha.clone()),
                    ..CiJobQuery::default()
                },
            )
            .await?;
        // Do not trust a provider to enforce both query filters: stale jobs
        // belonging only to a previous head are not current-head presence.
        // Keep this match rule aligned with `CiStatus::from_jobs_for_head` so
        // accepted full/abbreviated SHA pairs cannot disagree with the gate.
        let current_head_jobs_present = jobs
            .iter()
            .any(|job| sha_identifies_head(&job.commit_sha, &head_sha));
        jobs.retain(|job| sha_identifies_head(&job.commit_sha, &head_sha));
        let status = CiStatus::from_jobs(&jobs);
        observations.push(CiStatusObservation {
            pull_request_number: pull_request.number,
            head_sha,
            current_head_jobs_present,
            state: status.state(),
            completed_at: status.completed_at(),
        });
    }

    Ok(observations)
}

fn sha_identifies_head(job_sha: &str, head_sha: &str) -> bool {
    let job_sha = job_sha.trim();
    if job_sha.is_empty() {
        return false;
    }
    if job_sha.eq_ignore_ascii_case(head_sha) {
        return true;
    }

    let (shorter, longer) = if job_sha.len() < head_sha.len() {
        (job_sha, head_sha)
    } else {
        (head_sha, job_sha)
    };
    shorter.len() >= 7
        && longer
            .get(..shorter.len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(shorter))
}

fn relevant_candidate(queues: &[&QueueManifest], classified: &ClassifiedArtifact) -> bool {
    !classified.metadata.staged
        && !requires_human_attention(&classified.labels)
        && queues
            .iter()
            .any(|queue| matches_queue_cheap(*queue, classified))
}
