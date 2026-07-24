use crate::FilesystemForge;
use crate::lists::sort_ci_jobs_by_name;
use crate::validation::validate_stored_ci_jobs;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use temper_forge_model::{
    ChangeKind, CiJob, CiJobId, CiRetryOutcome, CiRetryRequest, ForgeError, ForgeResult,
    RepositoryId,
};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct CiRetryFixture {
    pub(crate) outcome: CiRetryOutcome,
    #[serde(default)]
    pub(crate) requests: Vec<CiRetryRequest>,
    #[serde(default)]
    pub(crate) accepted: Vec<CiRetryRequest>,
}

impl Default for CiRetryFixture {
    fn default() -> Self {
        Self {
            outcome: CiRetryOutcome::Unsupported,
            requests: Vec::new(),
            accepted: Vec::new(),
        }
    }
}

impl FilesystemForge {
    /// Selects the deterministic exact-attempt retry outcome for this fixture.
    /// The setting and accepted-request receipts are durable across reopened
    /// handles, allowing restart tests to reconcile uncertain operations.
    pub fn set_ci_retry_outcome(&self, outcome: CiRetryOutcome) -> ForgeResult<()> {
        let _guard = self.write_lock()?;
        let mut fixture = self.read_ci_retry_fixture()?;
        fixture.outcome = outcome;
        self.write_json(&self.ci_retry_fixture_file(), &fixture)
    }

    /// Returns exact-attempt retry requests in call order.
    pub fn ci_retry_requests(&self) -> ForgeResult<Vec<CiRetryRequest>> {
        Ok(self.read_ci_retry_fixture()?.requests)
    }

    pub(crate) fn read_ci_retry_fixture(&self) -> ForgeResult<CiRetryFixture> {
        let path = self.ci_retry_fixture_file();
        if path.exists() {
            self.read_json(&path)
        } else {
            Ok(CiRetryFixture::default())
        }
    }

    pub(crate) fn write_ci_retry_fixture(&self, fixture: &CiRetryFixture) -> ForgeResult<()> {
        self.write_json(&self.ci_retry_fixture_file(), fixture)
    }

    fn ci_retry_fixture_file(&self) -> PathBuf {
        self.root().join("ci_retry.json")
    }

    /// Seeds CI jobs for a repository, replacing any previously stored jobs.
    ///
    /// The Forge interface has no CI job creation operation. Tests and local
    /// scenarios use this backend-specific helper to write the same
    /// `ci_jobs.json` fixture file that
    /// [`list_ci_jobs`](temper_forge_model::Forge::list_ci_jobs) reads.
    pub fn seed_ci_jobs(&self, repo_id: &RepositoryId, ci_jobs: Vec<CiJob>) -> ForgeResult<()> {
        let _guard = self.write_lock()?;
        self.require_repository(repo_id)?;
        validate_stored_ci_jobs(repo_id, &ci_jobs)?;
        let Some(path) = self.ci_jobs_file(repo_id) else {
            return Err(ForgeError::Backend(format!(
                "invalid repository id {repo_id} for CI jobs path"
            )));
        };
        self.write_json(&path, &ci_jobs)?;
        self.publish_repo_hint(repo_id, ChangeKind::Ci);
        Ok(())
    }

    pub(crate) fn ci_jobs_file(&self, repo_id: &RepositoryId) -> Option<PathBuf> {
        self.repository_scope_dir(repo_id)
            .map(|repository_dir| repository_dir.join("ci_jobs.json"))
    }

    pub(crate) fn read_ci_jobs_for_existing_repository(
        &self,
        repo_id: &RepositoryId,
    ) -> ForgeResult<Vec<CiJob>> {
        let Some(path) = self.ci_jobs_file(repo_id) else {
            return Err(ForgeError::Backend(format!(
                "invalid repository id {repo_id} for CI jobs path"
            )));
        };
        if !path.exists() {
            return Ok(Vec::new());
        }

        let mut ci_jobs: Vec<CiJob> = self.read_json(&path)?;
        validate_stored_ci_jobs(repo_id, &ci_jobs)?;
        sort_ci_jobs_by_name(&mut ci_jobs);
        Ok(ci_jobs)
    }

    pub(crate) fn find_ci_job_by_id(&self, id: &CiJobId) -> ForgeResult<Option<CiJob>> {
        let mut repositories = self.read_repositories()?;
        repositories.sort_by(|left, right| left.id.cmp(&right.id));

        let mut matches = Vec::new();
        for repository in repositories {
            matches.extend(
                self.read_ci_jobs_for_existing_repository(&repository.id)?
                    .into_iter()
                    .filter(|ci_job| &ci_job.id == id),
            );
        }

        match matches.len() {
            0 => Ok(None),
            1 => Ok(matches.pop()),
            _ => Err(ForgeError::Backend(format!(
                "filesystem storage contains duplicate CI job id {id}"
            ))),
        }
    }
}
