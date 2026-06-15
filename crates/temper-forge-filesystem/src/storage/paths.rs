use crate::FilesystemForge;
use crate::record_ids::is_record_id;
use std::path::PathBuf;
use temper_forge::{IssueId, PullRequestId, RepositoryId};

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
}
