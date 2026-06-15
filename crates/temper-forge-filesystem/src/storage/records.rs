use crate::FilesystemForge;
use crate::errors::backend_error;
use crate::lists::{
    sort_comments, sort_issues_by_number, sort_labels, sort_pull_requests_by_number, sort_reviews,
};
use crate::validation::{
    validate_stored_issue_comments, validate_stored_issues, validate_stored_labels,
    validate_stored_pull_request_comments, validate_stored_pull_request_reviews,
    validate_stored_pull_requests,
};
use std::fs;
use std::path::Path;
use temper_forge_model::{
    Comment, ForgeError, ForgeResult, Issue, IssueId, Label, PullRequest, PullRequestId,
    PullRequestReview, Repository, RepositoryId,
};

impl FilesystemForge {
    pub(crate) fn read_repositories(&self) -> ForgeResult<Vec<Repository>> {
        self.ensure_layout()?;

        let mut repositories = Vec::new();
        for entry in fs::read_dir(self.repositories_dir()).map_err(|error| {
            backend_error(
                format!(
                    "read repositories directory {}",
                    self.repositories_dir().display()
                ),
                error,
            )
        })? {
            let entry =
                entry.map_err(|error| backend_error("read repository directory entry", error))?;
            let path = entry.path();
            if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
                continue;
            }
            if !entry
                .file_type()
                .map_err(|error| {
                    backend_error(format!("read file type for {}", path.display()), error)
                })?
                .is_file()
            {
                continue;
            }

            repositories.push(self.read_repository_file(&path)?);
        }

        Ok(repositories)
    }

    pub(crate) fn read_repository_file(&self, path: &Path) -> ForgeResult<Repository> {
        self.read_json(path)
    }

    pub(crate) fn read_labels_for_existing_repository(
        &self,
        repo_id: &RepositoryId,
    ) -> ForgeResult<Vec<Label>> {
        let Some(path) = self.labels_file(repo_id) else {
            return Err(ForgeError::Backend(format!(
                "invalid repository id {repo_id} for labels path"
            )));
        };
        if !path.exists() {
            return Ok(Vec::new());
        }

        let mut labels: Vec<Label> = self.read_json(&path)?;
        validate_stored_labels(repo_id, &labels)?;
        sort_labels(&mut labels);
        Ok(labels)
    }

    pub(crate) fn write_labels(&self, repo_id: &RepositoryId, labels: &[Label]) -> ForgeResult<()> {
        let Some(path) = self.labels_file(repo_id) else {
            return Err(ForgeError::Backend(format!(
                "invalid repository id {repo_id} for labels path"
            )));
        };

        self.write_json(&path, &labels)
    }

    pub(crate) fn read_issues_for_existing_repository(
        &self,
        repo_id: &RepositoryId,
    ) -> ForgeResult<Vec<Issue>> {
        let Some(path) = self.issues_file(repo_id) else {
            return Err(ForgeError::Backend(format!(
                "invalid repository id {repo_id} for issues path"
            )));
        };
        if !path.exists() {
            return Ok(Vec::new());
        }

        let mut issues: Vec<Issue> = self.read_json(&path)?;
        validate_stored_issues(repo_id, &issues)?;
        sort_issues_by_number(&mut issues);
        Ok(issues)
    }

    pub(crate) fn write_issues(&self, repo_id: &RepositoryId, issues: &[Issue]) -> ForgeResult<()> {
        let Some(path) = self.issues_file(repo_id) else {
            return Err(ForgeError::Backend(format!(
                "invalid repository id {repo_id} for issues path"
            )));
        };

        self.write_json(&path, &issues)
    }

    pub(crate) fn read_pull_requests_for_existing_repository(
        &self,
        repo_id: &RepositoryId,
    ) -> ForgeResult<Vec<PullRequest>> {
        let Some(path) = self.pull_requests_file(repo_id) else {
            return Err(ForgeError::Backend(format!(
                "invalid repository id {repo_id} for pull requests path"
            )));
        };
        if !path.exists() {
            return Ok(Vec::new());
        }

        let mut pull_requests: Vec<PullRequest> = self.read_json(&path)?;
        validate_stored_pull_requests(repo_id, &pull_requests)?;
        sort_pull_requests_by_number(&mut pull_requests);
        Ok(pull_requests)
    }

    pub(crate) fn write_pull_requests(
        &self,
        repo_id: &RepositoryId,
        pull_requests: &[PullRequest],
    ) -> ForgeResult<()> {
        let Some(path) = self.pull_requests_file(repo_id) else {
            return Err(ForgeError::Backend(format!(
                "invalid repository id {repo_id} for pull requests path"
            )));
        };

        self.write_json(&path, &pull_requests)
    }

    pub(crate) fn read_issue_comments_for_existing_issue(
        &self,
        repo_id: &RepositoryId,
        issue_id: &IssueId,
    ) -> ForgeResult<Vec<Comment>> {
        let Some(path) = self.issue_comments_file(repo_id, issue_id) else {
            return Err(ForgeError::Backend(format!(
                "invalid issue id {issue_id} for issue comments path"
            )));
        };
        if !path.exists() {
            return Ok(Vec::new());
        }

        let mut comments: Vec<Comment> = self.read_json(&path)?;
        validate_stored_issue_comments(issue_id, &comments)?;
        sort_comments(&mut comments);
        Ok(comments)
    }

    pub(crate) fn write_issue_comments(
        &self,
        repo_id: &RepositoryId,
        issue_id: &IssueId,
        comments: &[Comment],
    ) -> ForgeResult<()> {
        let Some(path) = self.issue_comments_file(repo_id, issue_id) else {
            return Err(ForgeError::Backend(format!(
                "invalid issue id {issue_id} for issue comments path"
            )));
        };

        self.write_json(&path, &comments)
    }

    pub(crate) fn read_pull_request_comments_for_existing_pull_request(
        &self,
        repo_id: &RepositoryId,
        pull_request_id: &PullRequestId,
    ) -> ForgeResult<Vec<Comment>> {
        let Some(path) = self.pull_request_comments_file(repo_id, pull_request_id) else {
            return Err(ForgeError::Backend(format!(
                "invalid pull request id {pull_request_id} for pull request comments path"
            )));
        };
        if !path.exists() {
            return Ok(Vec::new());
        }

        let mut comments: Vec<Comment> = self.read_json(&path)?;
        validate_stored_pull_request_comments(pull_request_id, &comments)?;
        sort_comments(&mut comments);
        Ok(comments)
    }

    pub(crate) fn write_pull_request_comments(
        &self,
        repo_id: &RepositoryId,
        pull_request_id: &PullRequestId,
        comments: &[Comment],
    ) -> ForgeResult<()> {
        let Some(path) = self.pull_request_comments_file(repo_id, pull_request_id) else {
            return Err(ForgeError::Backend(format!(
                "invalid pull request id {pull_request_id} for pull request comments path"
            )));
        };

        self.write_json(&path, &comments)
    }

    pub(crate) fn read_pull_request_reviews_for_existing_pull_request(
        &self,
        repo_id: &RepositoryId,
        pull_request_id: &PullRequestId,
    ) -> ForgeResult<Vec<PullRequestReview>> {
        let Some(path) = self.pull_request_reviews_file(repo_id, pull_request_id) else {
            return Err(ForgeError::Backend(format!(
                "invalid pull request id {pull_request_id} for pull request reviews path"
            )));
        };
        if !path.exists() {
            return Ok(Vec::new());
        }

        let mut reviews: Vec<PullRequestReview> = self.read_json(&path)?;
        validate_stored_pull_request_reviews(pull_request_id, &reviews)?;
        sort_reviews(&mut reviews);
        Ok(reviews)
    }

    pub(crate) fn write_pull_request_reviews(
        &self,
        repo_id: &RepositoryId,
        pull_request_id: &PullRequestId,
        reviews: &[PullRequestReview],
    ) -> ForgeResult<()> {
        let Some(path) = self.pull_request_reviews_file(repo_id, pull_request_id) else {
            return Err(ForgeError::Backend(format!(
                "invalid pull request id {pull_request_id} for pull request reviews path"
            )));
        };
        self.write_json(&path, &reviews)
    }
}
