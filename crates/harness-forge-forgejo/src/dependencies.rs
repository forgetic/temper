//! Native dependency-link operations for issues and pull requests.
//!
//! Forgejo (like Gitea) exposes issue dependencies under
//! `/repos/{owner}/{repo}/issues/{number}/dependencies`. Pull requests share the
//! issue-number namespace on Forgejo, so the same endpoint serves pull-request
//! dependency links with the pull-request number as the source — a
//! provider-specific adaptation documented in `docs/reference/forgejo-backend.md`.
//!
//! These are inherent methods on [`ForgejoForge`] mirroring the
//! [`harness_forge::Forge`] dependency-link surface; the trait implementation is
//! assembled once every phase's methods exist. The endpoint shapes (the
//! `{ "index": <number> }` add/remove payload and the dependency-list response)
//! are best-effort and isolated here so live refinement only edits this module.

use crate::ids::{parse_issue_id, parse_pull_request_id, RepoCoord};
use crate::pulls::response_validator;
use crate::types::{DependencyRefDto, IssueDto};
use crate::{ForgejoForge, HttpClient, HttpMethod};
use harness_forge::{
    ForgeError, ForgeResult, Issue, IssueId, ItemNumber, PullRequest, PullRequestId,
};

impl<C: HttpClient> ForgejoForge<C> {
    /// Adds a dependency link from an issue to another repository item number.
    pub async fn add_issue_dependency(
        &self,
        id: &IssueId,
        target: ItemNumber,
    ) -> ForgeResult<Issue> {
        let (repo, number) = parse_issue_id(id)?;
        let source = self
            .fetch_issue(&repo, number)
            .await?
            .ok_or_else(|| ForgeError::NotFound(format!("issue {id}")))?;
        if source.dependencies.contains(&target) {
            return Ok(source);
        }
        self.ensure_dependency_target_exists(&repo, target).await?;
        self.add_item_dependency(&repo, number, target).await?;
        self.fetch_issue(&repo, number)
            .await?
            .ok_or_else(|| ForgeError::NotFound(format!("issue {id}")))
    }

    /// Removes a dependency link from an issue to another repository item number.
    pub async fn remove_issue_dependency(
        &self,
        id: &IssueId,
        target: ItemNumber,
    ) -> ForgeResult<Issue> {
        let (repo, number) = parse_issue_id(id)?;
        let source = self
            .fetch_issue(&repo, number)
            .await?
            .ok_or_else(|| ForgeError::NotFound(format!("issue {id}")))?;
        if !source.dependencies.contains(&target) {
            return Ok(source);
        }
        self.remove_item_dependency(&repo, number, target).await?;
        self.fetch_issue(&repo, number)
            .await?
            .ok_or_else(|| ForgeError::NotFound(format!("issue {id}")))
    }

    /// Adds a dependency link from a pull request to another item number.
    pub async fn add_pull_request_dependency(
        &self,
        id: &PullRequestId,
        target: ItemNumber,
    ) -> ForgeResult<PullRequest> {
        let (repo, number) = parse_pull_request_id(id)?;
        let source = self
            .fetch_pull_request_with_dependencies(&repo, number)
            .await?
            .ok_or_else(|| ForgeError::NotFound(format!("pull request {id}")))?;
        if source.dependencies.contains(&target) {
            return Ok(source);
        }
        self.ensure_dependency_target_exists(&repo, target).await?;
        self.add_item_dependency(&repo, number, target).await?;
        self.fetch_pull_request_with_dependencies(&repo, number)
            .await?
            .ok_or_else(|| ForgeError::NotFound(format!("pull request {id}")))
    }

    /// Removes a dependency link from a pull request to another item number.
    pub async fn remove_pull_request_dependency(
        &self,
        id: &PullRequestId,
        target: ItemNumber,
    ) -> ForgeResult<PullRequest> {
        let (repo, number) = parse_pull_request_id(id)?;
        let source = self
            .fetch_pull_request_with_dependencies(&repo, number)
            .await?
            .ok_or_else(|| ForgeError::NotFound(format!("pull request {id}")))?;
        if !source.dependencies.contains(&target) {
            return Ok(source);
        }
        self.remove_item_dependency(&repo, number, target).await?;
        self.fetch_pull_request_with_dependencies(&repo, number)
            .await?
            .ok_or_else(|| ForgeError::NotFound(format!("pull request {id}")))
    }

    /// Fetches an issue and enriches it with its dependency links.
    ///
    /// Forgejo serves both issues and pull requests through the issue endpoint,
    /// so this resolves either by number. Returns `None` on `404`.
    pub(crate) async fn fetch_issue(
        &self,
        repo: &RepoCoord,
        number: ItemNumber,
    ) -> ForgeResult<Option<Issue>> {
        let path = format!("/repos/{}/issues/{}", repo.path_segment(), number.get());
        let Some(response) = self
            .request_optional("get issue", HttpMethod::Get, &path, Vec::new(), None)
            .await?
        else {
            return Ok(None);
        };
        let validator = response_validator(&response);
        let dto: IssueDto = Self::decode("get issue", &response)?;
        let mut issue = self.materialize_issue(repo, dto, validator.as_deref());
        issue.dependencies = self.load_item_dependencies(repo, number).await?;
        Ok(Some(issue))
    }

    /// Reads an item's dependency links as sorted, deduplicated item numbers.
    ///
    /// A `404` (the issue has no dependencies, or the provider does not support
    /// the endpoint) maps to an empty list: a safe, documented behavior for
    /// reads. Add/remove never silently treat an unsupported endpoint as
    /// success (see [`Self::add_item_dependency`]).
    pub(crate) async fn load_item_dependencies(
        &self,
        repo: &RepoCoord,
        number: ItemNumber,
    ) -> ForgeResult<Vec<ItemNumber>> {
        let path = dependencies_path(repo, number);
        let Some(response) = self
            .request_optional(
                "list dependencies",
                HttpMethod::Get,
                &path,
                Vec::new(),
                None,
            )
            .await?
        else {
            return Ok(Vec::new());
        };
        let refs: Vec<DependencyRefDto> = Self::decode("list dependencies", &response)?;
        let mut numbers: Vec<ItemNumber> = refs
            .into_iter()
            .map(|dependency| ItemNumber::new(dependency.number))
            .collect();
        numbers.sort();
        numbers.dedup();
        Ok(numbers)
    }

    /// Verifies a dependency target resolves to an issue or pull request.
    ///
    /// Forgejo's issue endpoint serves both issues and pull requests, so a
    /// single lookup covers the portable "issue or pull request in the same
    /// repository" rule. A missing target maps to [`ForgeError::NotFound`].
    async fn ensure_dependency_target_exists(
        &self,
        repo: &RepoCoord,
        target: ItemNumber,
    ) -> ForgeResult<()> {
        let path = format!("/repos/{}/issues/{}", repo.path_segment(), target.get());
        match self
            .request_optional(
                "verify dependency target",
                HttpMethod::Get,
                &path,
                Vec::new(),
                None,
            )
            .await?
        {
            Some(_) => Ok(()),
            None => Err(ForgeError::NotFound(format!(
                "dependency target item {} in repository {}",
                target.get(),
                repo.path_segment()
            ))),
        }
    }

    /// Posts a dependency link `{ "index": target }` to the source item.
    ///
    /// A `404` here means the dependency endpoint is unsupported (the target was
    /// already verified to exist), so it maps to [`ForgeError::InvalidRequest`]
    /// rather than silently claiming success.
    async fn add_item_dependency(
        &self,
        repo: &RepoCoord,
        number: ItemNumber,
        target: ItemNumber,
    ) -> ForgeResult<()> {
        let path = dependencies_path(repo, number);
        let payload = serde_json::json!({ "index": target.get() }).to_string();
        let response = self
            .send(HttpMethod::Post, &path, Vec::new(), Some(payload))
            .await?;
        self.check_dependency_mutation("add dependency", response)
    }

    /// Deletes a dependency link `{ "index": target }` from the source item.
    ///
    /// The caller already confirmed the link is present, so a `404` indicates an
    /// unsupported endpoint and maps to [`ForgeError::InvalidRequest`].
    async fn remove_item_dependency(
        &self,
        repo: &RepoCoord,
        number: ItemNumber,
        target: ItemNumber,
    ) -> ForgeResult<()> {
        let path = dependencies_path(repo, number);
        let payload = serde_json::json!({ "index": target.get() }).to_string();
        let response = self
            .send(HttpMethod::Delete, &path, Vec::new(), Some(payload))
            .await?;
        self.check_dependency_mutation("remove dependency", response)
    }

    /// Classifies a dependency add/remove response, mapping `404` (unsupported
    /// endpoint) to [`ForgeError::InvalidRequest`].
    fn check_dependency_mutation(
        &self,
        context: &str,
        response: crate::HttpResponse,
    ) -> ForgeResult<()> {
        if response.is_success() {
            return Ok(());
        }
        if response.status == 404 {
            return Err(ForgeError::InvalidRequest(format!(
                "{context}: forgejo dependency endpoint is unsupported (HTTP 404)"
            )));
        }
        Err(crate::error::map_status_error(context, &response))
    }
}

/// Returns the dependencies endpoint path for an issue or pull-request number.
fn dependencies_path(repo: &RepoCoord, number: ItemNumber) -> String {
    format!(
        "/repos/{}/issues/{}/dependencies",
        repo.path_segment(),
        number.get()
    )
}
