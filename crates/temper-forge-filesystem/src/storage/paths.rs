use crate::FilesystemForge;
use crate::record_ids::is_record_id;
use std::path::PathBuf;
use temper_forge_model::{IssueId, PullRequestId, RepositoryId};

impl FilesystemForge {
    pub(crate) fn metadata_path(&self) -> PathBuf {
        self.root().join("metadata.json")
    }

    pub(crate) fn repository_file(&self, id: &RepositoryId) -> Option<PathBuf> {
        is_record_id(id.as_str()).then(|| self.repositories_dir().join(format!("{id}.json")))
    }

    pub(crate) fn repository_scope_dir(&self, id: &RepositoryId) -> Option<PathBuf> {
        is_record_id(id.as_str()).then(|| self.repositories_dir().join(id.as_str()))
    }

    pub(crate) fn labels_file(&self, repo_id: &RepositoryId) -> Option<PathBuf> {
        self.repository_scope_dir(repo_id)
            .map(|repository_dir| repository_dir.join("labels.json"))
    }

    pub(crate) fn issues_file(&self, repo_id: &RepositoryId) -> Option<PathBuf> {
        self.repository_scope_dir(repo_id)
            .map(|repository_dir| repository_dir.join("issues.json"))
    }

    pub(crate) fn pull_requests_file(&self, repo_id: &RepositoryId) -> Option<PathBuf> {
        self.repository_scope_dir(repo_id)
            .map(|repository_dir| repository_dir.join("pull_requests.json"))
    }

    pub(crate) fn issue_scope_dir(
        &self,
        repo_id: &RepositoryId,
        issue_id: &IssueId,
    ) -> Option<PathBuf> {
        if !is_record_id(issue_id.as_str()) {
            return None;
        }

        self.repository_scope_dir(repo_id)
            .map(|repository_dir| repository_dir.join("issues").join(issue_id.as_str()))
    }

    pub(crate) fn issue_comments_file(
        &self,
        repo_id: &RepositoryId,
        issue_id: &IssueId,
    ) -> Option<PathBuf> {
        self.issue_scope_dir(repo_id, issue_id)
            .map(|issue_dir| issue_dir.join("comments.json"))
    }

    pub(crate) fn pull_request_scope_dir(
        &self,
        repo_id: &RepositoryId,
        pull_request_id: &PullRequestId,
    ) -> Option<PathBuf> {
        if !is_record_id(pull_request_id.as_str()) {
            return None;
        }

        self.repository_scope_dir(repo_id).map(|repository_dir| {
            repository_dir
                .join("pull_requests")
                .join(pull_request_id.as_str())
        })
    }

    pub(crate) fn pull_request_comments_file(
        &self,
        repo_id: &RepositoryId,
        pull_request_id: &PullRequestId,
    ) -> Option<PathBuf> {
        self.pull_request_scope_dir(repo_id, pull_request_id)
            .map(|pull_request_dir| pull_request_dir.join("comments.json"))
    }

    pub(crate) fn pull_request_reviews_file(
        &self,
        repo_id: &RepositoryId,
        pull_request_id: &PullRequestId,
    ) -> Option<PathBuf> {
        self.pull_request_scope_dir(repo_id, pull_request_id)
            .map(|pull_request_dir| pull_request_dir.join("reviews.json"))
    }

    // --- Provisioning record paths (ForgeContent + ForgeAdmin) ---

    /// Store-global provisioned-users record file.
    pub(crate) fn users_file(&self) -> PathBuf {
        self.root().join("users.json")
    }

    /// Store-global minted-tokens record file.
    pub(crate) fn tokens_file(&self) -> PathBuf {
        self.root().join("tokens.json")
    }

    /// Store-global org Owners-team membership record file.
    pub(crate) fn owners_file(&self) -> PathBuf {
        self.root().join("owners.json")
    }

    /// Per-repository webhook record file.
    pub(crate) fn webhooks_file(&self, repo_id: &RepositoryId) -> Option<PathBuf> {
        self.repository_scope_dir(repo_id)
            .map(|repository_dir| repository_dir.join("webhooks.json"))
    }

    /// Per-repository collaborator-grant record file.
    pub(crate) fn grants_file(&self, repo_id: &RepositoryId) -> Option<PathBuf> {
        self.repository_scope_dir(repo_id)
            .map(|repository_dir| repository_dir.join("grants.json"))
    }

    /// Per-repository branch record file.
    pub(crate) fn branches_file(&self, repo_id: &RepositoryId) -> Option<PathBuf> {
        self.repository_scope_dir(repo_id)
            .map(|repository_dir| repository_dir.join("branches.json"))
    }

    /// Per-repository CI-enabled marker file.
    pub(crate) fn ci_enabled_file(&self, repo_id: &RepositoryId) -> Option<PathBuf> {
        self.repository_scope_dir(repo_id)
            .map(|repository_dir| repository_dir.join("ci_enabled.json"))
    }

    /// Committed-content path for a single file on a branch.
    ///
    /// Files live under a `content/<branch>/` tree so a branch's full content is
    /// recoverable from one subtree. The branch and the repository-relative path
    /// are hex-encoded per segment to keep arbitrary names filesystem-safe and
    /// collision-free.
    pub(crate) fn content_file(
        &self,
        repo_id: &RepositoryId,
        branch: &str,
        path: &str,
    ) -> Option<PathBuf> {
        let repository_dir = self.repository_scope_dir(repo_id)?;
        let mut file = repository_dir
            .join("content")
            .join(crate::record_ids::hex_encode(branch.as_bytes()));
        file.push(crate::record_ids::hex_encode(path.as_bytes()));
        Some(file)
    }
}
