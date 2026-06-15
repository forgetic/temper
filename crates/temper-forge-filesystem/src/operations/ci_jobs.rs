use crate::FilesystemForge;
use crate::lists::{ci_job_matches_query, sort_ci_jobs};
use temper_forge_model::{CiJob, CiJobId, CiJobQuery, ForgeResult, RepositoryId};

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

pub(crate) fn get_ci_job(forge: &FilesystemForge, id: &CiJobId) -> ForgeResult<Option<CiJob>> {
    forge.find_ci_job_by_id(id)
}
