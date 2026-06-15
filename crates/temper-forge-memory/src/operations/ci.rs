//! CI-job operations for [`MemoryForge`](crate::MemoryForge).

use crate::MemoryForge;
use crate::lists::{ci_job_matches_query, sort_ci_jobs};
use temper_forge_model::{CiJob, CiJobId, CiJobQuery, ForgeResult, RepositoryId};

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

pub(crate) fn get_ci_job(forge: &MemoryForge, id: &CiJobId) -> ForgeResult<Option<CiJob>> {
    Ok(forge.lock().state.find_ci_job(id))
}
