//! CI-job operations for [`MemoryForge`](crate::MemoryForge).

use crate::MemoryForge;
use crate::lists::{ci_job_matches_query, sort_ci_jobs};
use temper_forge_model::{CiJob, CiJobId, CiJobListing, CiJobQuery, ForgeResult, RepositoryId};

pub(crate) fn list_ci_jobs(
    forge: &MemoryForge,
    repo_id: &RepositoryId,
    query: CiJobQuery,
) -> ForgeResult<Vec<CiJob>> {
    Ok(list_ci_jobs_with_presence(forge, repo_id, query)?.into_jobs())
}

pub(crate) fn list_ci_jobs_with_presence(
    forge: &MemoryForge,
    repo_id: &RepositoryId,
    query: CiJobQuery,
) -> ForgeResult<CiJobListing> {
    let inner = forge.lock();
    inner.state.require_repository(repo_id)?;
    let stored = inner.state.ci_jobs(repo_id);
    let mut presence_query = query.clone();
    presence_query.status = None;
    let matching_ci_present = stored
        .iter()
        .any(|ci_job| ci_job_matches_query(ci_job, &presence_query))
        || inner.state.ci_run_matches(
            repo_id,
            query.pull_request_id.as_ref(),
            query.commit_sha.as_deref(),
        );
    let mut ci_jobs = stored
        .into_iter()
        .filter(|ci_job| ci_job_matches_query(ci_job, &query))
        .collect::<Vec<_>>();
    sort_ci_jobs(&mut ci_jobs, &query);
    Ok(CiJobListing::new(ci_jobs, matching_ci_present))
}

pub(crate) fn get_ci_job(forge: &MemoryForge, id: &CiJobId) -> ForgeResult<Option<CiJob>> {
    Ok(forge.lock().state.find_ci_job(id))
}
