// SPDX-License-Identifier: MPL-2.0

use async_trait::async_trait;
use temper_forge::{
    Comment, Forge, ForgeResult, Issue, IssueId, IssueQuery, ItemNumber, PullRequest,
    PullRequestId, PullRequestQuery, RepositoryId,
};

/// Narrow read capability used by artifact-context collection.
///
/// Keeping this separate from `Forge` makes it impossible for the resolver to
/// mutate forge state. The blanket implementation is the only production
/// adapter; tests can use any normal Forge, including `MemoryForge`.
#[async_trait]
pub trait ArtifactContextForge: Send + Sync {
    async fn issue(
        &self,
        repository: &RepositoryId,
        number: ItemNumber,
    ) -> ForgeResult<Option<Issue>>;
    async fn pull_request(
        &self,
        repository: &RepositoryId,
        number: ItemNumber,
    ) -> ForgeResult<Option<PullRequest>>;
    async fn issues(&self, repository: &RepositoryId, query: IssueQuery)
    -> ForgeResult<Vec<Issue>>;
    async fn pull_requests(
        &self,
        repository: &RepositoryId,
        query: PullRequestQuery,
    ) -> ForgeResult<Vec<PullRequest>>;
    async fn issue_comments(&self, issue: &IssueId) -> ForgeResult<Vec<Comment>>;
    async fn pull_request_comments(
        &self,
        pull_request: &PullRequestId,
    ) -> ForgeResult<Vec<Comment>>;
}

#[async_trait]
impl<T: Forge + ?Sized> ArtifactContextForge for T {
    async fn issue(
        &self,
        repository: &RepositoryId,
        number: ItemNumber,
    ) -> ForgeResult<Option<Issue>> {
        self.get_issue_by_number(repository, number).await
    }

    async fn pull_request(
        &self,
        repository: &RepositoryId,
        number: ItemNumber,
    ) -> ForgeResult<Option<PullRequest>> {
        self.get_pull_request_by_number(repository, number).await
    }

    async fn issues(
        &self,
        repository: &RepositoryId,
        query: IssueQuery,
    ) -> ForgeResult<Vec<Issue>> {
        self.list_issues(repository, query).await
    }

    async fn pull_requests(
        &self,
        repository: &RepositoryId,
        query: PullRequestQuery,
    ) -> ForgeResult<Vec<PullRequest>> {
        self.list_pull_requests(repository, query).await
    }

    async fn issue_comments(&self, issue: &IssueId) -> ForgeResult<Vec<Comment>> {
        self.list_issue_comments(issue).await
    }

    async fn pull_request_comments(
        &self,
        pull_request: &PullRequestId,
    ) -> ForgeResult<Vec<Comment>> {
        self.list_pull_request_comments(pull_request).await
    }
}
