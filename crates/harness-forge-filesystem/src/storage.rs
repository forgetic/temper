use crate::errors::backend_error;
use crate::lists::{
    sort_comments, sort_issues_by_number, sort_labels, sort_pull_requests_by_number, sort_reviews,
};
use crate::metadata::{default_metadata, Metadata};
use crate::record_ids::is_record_id;
use crate::validation::{
    validate_stored_issue_comments, validate_stored_issues, validate_stored_labels,
    validate_stored_pull_request_comments, validate_stored_pull_request_reviews,
    validate_stored_pull_requests,
};
use crate::FilesystemForge;
use harness_forge::{
    Comment, ForgeError, ForgeResult, Issue, IssueId, Label, PullRequest, PullRequestId,
    PullRequestReview, Repository, RepositoryId, RepositoryPath,
};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};

impl FilesystemForge {
    /// Creates a backend rooted at `root`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Returns the filesystem root used by this backend.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Returns the directory used to store repository records.
    pub fn repositories_dir(&self) -> PathBuf {
        self.root.join("repositories")
    }

    /// Creates the backend directory layout if needed.
    pub fn ensure_layout(&self) -> ForgeResult<()> {
        fs::create_dir_all(&self.root).map_err(|error| {
            backend_error(
                format!("create storage root {}", self.root.display()),
                error,
            )
        })?;
        fs::create_dir_all(self.repositories_dir()).map_err(|error| {
            backend_error(
                format!(
                    "create repositories directory {}",
                    self.repositories_dir().display()
                ),
                error,
            )
        })?;

        let metadata_path = self.metadata_path();
        if metadata_path.exists() {
            self.read_metadata_file()?;
        } else {
            self.write_metadata(&default_metadata())?;
        }
        Ok(())
    }

    pub(crate) fn metadata_path(&self) -> PathBuf {
        self.root.join("metadata.json")
    }

    pub(crate) fn read_metadata(&self) -> ForgeResult<Metadata> {
        self.ensure_layout()?;
        self.read_metadata_file()
    }

    pub(crate) fn read_metadata_file(&self) -> ForgeResult<Metadata> {
        let metadata: Metadata = self.read_json(&self.metadata_path())?;
        metadata.validate()?;
        Ok(metadata)
    }

    pub(crate) fn write_metadata(&self, metadata: &Metadata) -> ForgeResult<()> {
        metadata.validate()?;
        self.write_json(&self.metadata_path(), metadata)
    }

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

    pub(crate) fn find_repository_by_id(
        &self,
        id: &RepositoryId,
    ) -> ForgeResult<Option<Repository>> {
        self.ensure_layout()?;
        let Some(path) = self.repository_file(id) else {
            return Ok(None);
        };
        if !path.exists() {
            return Ok(None);
        }

        Ok(Some(self.read_repository_file(&path)?))
    }

    pub(crate) fn require_repository(&self, repo_id: &RepositoryId) -> ForgeResult<Repository> {
        self.find_repository_by_id(repo_id)?
            .ok_or_else(|| ForgeError::NotFound(format!("repository {repo_id}")))
    }

    pub(crate) fn find_repository_by_path(
        &self,
        path: &RepositoryPath,
    ) -> ForgeResult<Option<Repository>> {
        let mut matches = self
            .read_repositories()?
            .into_iter()
            .filter(|repository| repository.owner == path.owner && repository.name == path.name)
            .collect::<Vec<_>>();

        matches.sort_by(|left, right| left.id.cmp(&right.id));

        match matches.len() {
            0 => Ok(None),
            1 => Ok(matches.pop()),
            _ => Err(ForgeError::Backend(format!(
                "filesystem storage contains duplicate repository path {}/{}",
                path.owner, path.name
            ))),
        }
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

    pub(crate) fn find_issue_by_id(&self, id: &IssueId) -> ForgeResult<Option<Issue>> {
        let mut repositories = self.read_repositories()?;
        repositories.sort_by(|left, right| left.id.cmp(&right.id));

        let mut matches = Vec::new();
        for repository in repositories {
            matches.extend(
                self.read_issues_for_existing_repository(&repository.id)?
                    .into_iter()
                    .filter(|issue| &issue.id == id),
            );
        }

        match matches.len() {
            0 => Ok(None),
            1 => Ok(matches.pop()),
            _ => Err(ForgeError::Backend(format!(
                "filesystem storage contains duplicate issue id {id}"
            ))),
        }
    }

    pub(crate) fn find_issue_repository_by_id(
        &self,
        id: &IssueId,
    ) -> ForgeResult<Option<RepositoryId>> {
        let mut repositories = self.read_repositories()?;
        repositories.sort_by(|left, right| left.id.cmp(&right.id));

        let mut matches = Vec::new();
        for repository in repositories {
            if self
                .read_issues_for_existing_repository(&repository.id)?
                .iter()
                .any(|issue| &issue.id == id)
            {
                matches.push(repository.id);
            }
        }

        match matches.len() {
            0 => Ok(None),
            1 => Ok(matches.pop()),
            _ => Err(ForgeError::Backend(format!(
                "filesystem storage contains duplicate issue id {id}"
            ))),
        }
    }

    pub(crate) fn find_pull_request_by_id(
        &self,
        id: &PullRequestId,
    ) -> ForgeResult<Option<PullRequest>> {
        let mut repositories = self.read_repositories()?;
        repositories.sort_by(|left, right| left.id.cmp(&right.id));

        let mut matches = Vec::new();
        for repository in repositories {
            matches.extend(
                self.read_pull_requests_for_existing_repository(&repository.id)?
                    .into_iter()
                    .filter(|pull_request| &pull_request.id == id),
            );
        }

        match matches.len() {
            0 => Ok(None),
            1 => Ok(matches.pop()),
            _ => Err(ForgeError::Backend(format!(
                "filesystem storage contains duplicate pull request id {id}"
            ))),
        }
    }

    pub(crate) fn find_pull_request_repository_by_id(
        &self,
        id: &PullRequestId,
    ) -> ForgeResult<Option<RepositoryId>> {
        let mut repositories = self.read_repositories()?;
        repositories.sort_by(|left, right| left.id.cmp(&right.id));

        let mut matches = Vec::new();
        for repository in repositories {
            if self
                .read_pull_requests_for_existing_repository(&repository.id)?
                .iter()
                .any(|pull_request| &pull_request.id == id)
            {
                matches.push(repository.id);
            }
        }

        match matches.len() {
            0 => Ok(None),
            1 => Ok(matches.pop()),
            _ => Err(ForgeError::Backend(format!(
                "filesystem storage contains duplicate pull request id {id}"
            ))),
        }
    }

    pub(crate) fn read_json<T>(&self, path: &Path) -> ForgeResult<T>
    where
        T: DeserializeOwned,
    {
        let content = fs::read_to_string(path)
            .map_err(|error| backend_error(format!("read {}", path.display()), error))?;
        serde_json::from_str(&content)
            .map_err(|error| backend_error(format!("parse {}", path.display()), error))
    }

    pub(crate) fn write_json<T>(&self, path: &Path, value: &T) -> ForgeResult<()>
    where
        T: Serialize,
    {
        let parent = path.parent().ok_or_else(|| {
            ForgeError::Backend(format!("path {} has no parent directory", path.display()))
        })?;
        fs::create_dir_all(parent)
            .map_err(|error| backend_error(format!("create {}", parent.display()), error))?;

        let mut content = serde_json::to_string_pretty(value)
            .map_err(|error| backend_error(format!("serialize {}", path.display()), error))?;
        content.push('\n');

        let temporary_path = path.with_extension("tmp");
        fs::write(&temporary_path, content)
            .map_err(|error| backend_error(format!("write {}", temporary_path.display()), error))?;
        fs::rename(&temporary_path, path).map_err(|error| {
            let _ = fs::remove_file(&temporary_path);
            backend_error(
                format!(
                    "replace {} from {}",
                    path.display(),
                    temporary_path.display()
                ),
                error,
            )
        })?;

        Ok(())
    }

    pub(crate) fn next_repository_id(&self, metadata: &mut Metadata) -> ForgeResult<RepositoryId> {
        loop {
            let repository_number = metadata.next_repository_number;
            metadata.next_repository_number = metadata
                .next_repository_number
                .checked_add(1)
                .ok_or_else(|| ForgeError::Backend("repository id counter overflowed".into()))?;

            let id = RepositoryId::new(format!("repo-{repository_number:016}"));
            let path = self.repository_file(&id).ok_or_else(|| {
                ForgeError::Backend(format!("generated invalid repository id {id}"))
            })?;
            if !path.exists() {
                return Ok(id);
            }
        }
    }
}
