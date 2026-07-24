//! CI-job operations for [`MemoryForge`](crate::MemoryForge).

use crate::MemoryForge;
use crate::lists::{ci_job_matches_query, sort_ci_jobs};
use temper_forge_model::{
    CiJob, CiJobId, CiJobQuery, CiRetryOutcome, CiRetryRejection, CiRetryRequest, ForgeResult,
    RepositoryId,
};

pub(crate) fn list_ci_jobs(
    forge: &MemoryForge,
    repo_id: &RepositoryId,
    query: CiJobQuery,
) -> ForgeResult<Vec<CiJob>> {
    let inner = forge.lock();
    inner.state.require_repository(repo_id)?;
    let mut ci_jobs = inner
        .state
        .ci_jobs(repo_id)
        .into_iter()
        .filter(|ci_job| ci_job_matches_query(ci_job, &query))
        .collect::<Vec<_>>();
    sort_ci_jobs(&mut ci_jobs, &query);
    Ok(ci_jobs)
}

pub(crate) fn retry_ci_attempt(
    forge: &MemoryForge,
    request: CiRetryRequest,
) -> ForgeResult<CiRetryOutcome> {
    let mut inner = forge.lock();
    if inner.faults.take(crate::FaultOp::RetryCiAttempt).is_err() {
        return Ok(CiRetryOutcome::Uncertain);
    }
    inner.ci_retry_requests.push(request.clone());
    inner.state.require_repository(request.repo_id())?;
    let Some((pull_repo, pull_request)) = inner.state.find_pull_request(request.pull_request_id())
    else {
        return Ok(CiRetryOutcome::Rejected(
            CiRetryRejection::PullRequestMismatch,
        ));
    };
    if &pull_repo != request.repo_id() {
        return Ok(CiRetryOutcome::Rejected(
            CiRetryRejection::RepositoryMismatch,
        ));
    }
    if pull_request.head_sha.as_deref() != Some(request.head_sha()) {
        return Ok(CiRetryOutcome::Rejected(CiRetryRejection::HeadChanged));
    }
    if inner.accepted_ci_retries.contains(&request) {
        return Ok(CiRetryOutcome::AlreadyObserved);
    }
    let jobs = inner
        .state
        .ci_jobs(request.repo_id())
        .into_iter()
        .filter(|job| {
            job.run_id.as_deref() == Some(request.run_id())
                && job.attempt.as_deref() == Some(request.attempt())
        })
        .collect::<Vec<_>>();
    if !request.matches_jobs(&jobs) {
        return Ok(CiRetryOutcome::Rejected(CiRetryRejection::JobSetChanged));
    }
    let outcome = inner.ci_retry_outcome;
    if outcome == CiRetryOutcome::Accepted {
        inner.accepted_ci_retries.push(request);
    }
    Ok(outcome)
}

pub(crate) fn get_ci_job(forge: &MemoryForge, id: &CiJobId) -> ForgeResult<Option<CiJob>> {
    Ok(forge.lock().state.find_ci_job(id))
}
