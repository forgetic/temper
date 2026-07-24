use crate::FilesystemForge;
use crate::lists::{ci_job_matches_query, sort_ci_jobs};
use temper_forge_model::{CiJob, CiJobId, CiJobListing, CiJobQuery, ForgeResult, RepositoryId};

pub(crate) fn list_ci_jobs(
    forge: &FilesystemForge,
    repo_id: &RepositoryId,
    query: CiJobQuery,
) -> ForgeResult<Vec<CiJob>> {
    Ok(list_ci_jobs_with_presence(forge, repo_id, query)?.into_jobs())
}

pub(crate) fn list_ci_jobs_with_presence(
    forge: &FilesystemForge,
    repo_id: &RepositoryId,
    query: CiJobQuery,
) -> ForgeResult<CiJobListing> {
    forge.require_repository(repo_id)?;

    let stored = forge.read_ci_jobs_for_existing_repository(repo_id)?;
    let mut presence_query = query.clone();
    presence_query.status = None;
    let matching_ci_present = stored
        .iter()
        .any(|ci_job| ci_job_matches_query(ci_job, &presence_query));
    let mut ci_jobs = stored
        .into_iter()
        .filter(|ci_job| ci_job_matches_query(ci_job, &query))
        .collect::<Vec<_>>();
    sort_ci_jobs(&mut ci_jobs, &query);
    Ok(CiJobListing::new(ci_jobs, matching_ci_present))
}

pub(crate) fn get_ci_job(forge: &FilesystemForge, id: &CiJobId) -> ForgeResult<Option<CiJob>> {
    forge.find_ci_job_by_id(id)
}
