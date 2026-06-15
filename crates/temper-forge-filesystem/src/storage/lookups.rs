use crate::FilesystemForge;
use temper_forge::{
    ForgeError, ForgeResult, Issue, IssueId, PullRequest, PullRequestId, Repository, RepositoryId,
    RepositoryPath,
};

impl FilesystemForge {
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
}
