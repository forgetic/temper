use crate::FilesystemForge;
use crate::lists::{ci_job_matches_query, sort_ci_jobs};
use temper_forge_model::{
    CiJob, CiJobId, CiJobQuery, CiRetryOutcome, CiRetryRejection, CiRetryRequest, ForgeResult,
    RepositoryId,
};

pub(crate) fn list_ci_jobs(
    forge: &FilesystemForge,
    repo_id: &RepositoryId,
    query: CiJobQuery,
) -> ForgeResult<Vec<CiJob>> {
    forge.require_repository(repo_id)?;

    let mut ci_jobs = forge
        .read_ci_jobs_for_existing_repository(repo_id)?
        .into_iter()
        .filter(|ci_job| ci_job_matches_query(ci_job, &query))
        .collect::<Vec<_>>();
    sort_ci_jobs(&mut ci_jobs, &query);
    Ok(ci_jobs)
}

pub(crate) fn retry_ci_attempt(
    forge: &FilesystemForge,
    request: CiRetryRequest,
) -> ForgeResult<CiRetryOutcome> {
    let _guard = forge.write_lock()?;
    forge.require_repository(request.repo_id())?;
    let Some(pull_request) = forge.find_pull_request_by_id(request.pull_request_id())? else {
        return Ok(CiRetryOutcome::Rejected(
            CiRetryRejection::PullRequestMismatch,
        ));
    };
    if &pull_request.target.repository_id != request.repo_id() {
        return Ok(CiRetryOutcome::Rejected(
            CiRetryRejection::RepositoryMismatch,
        ));
    }
    if pull_request.head_sha.as_deref() != Some(request.head_sha()) {
        return Ok(CiRetryOutcome::Rejected(CiRetryRejection::HeadChanged));
    }

    let mut fixture = forge.read_ci_retry_fixture()?;
    fixture.requests.push(request.clone());
    if fixture.accepted.contains(&request) {
        forge.write_ci_retry_fixture(&fixture)?;
        return Ok(CiRetryOutcome::AlreadyObserved);
    }
    let jobs = forge
        .read_ci_jobs_for_existing_repository(request.repo_id())?
        .into_iter()
        .filter(|job| {
            job.run_id.as_deref() == Some(request.run_id())
                && job.attempt.as_deref() == Some(request.attempt())
        })
        .collect::<Vec<_>>();
    if !request.matches_jobs(&jobs) {
        forge.write_ci_retry_fixture(&fixture)?;
        return Ok(CiRetryOutcome::Rejected(CiRetryRejection::JobSetChanged));
    }
    let outcome = fixture.outcome;
    if outcome == CiRetryOutcome::Accepted {
        fixture.accepted.push(request);
    }
    forge.write_ci_retry_fixture(&fixture)?;
    Ok(outcome)
}

pub(crate) fn get_ci_job(forge: &FilesystemForge, id: &CiJobId) -> ForgeResult<Option<CiJob>> {
    forge.find_ci_job_by_id(id)
}
