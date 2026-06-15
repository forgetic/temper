//! [`Forge`] implementation for [`MemoryForge`](crate::MemoryForge).
//!
//! Every method takes the single interior mutex for the duration of the call,
//! checks the [fault hook](crate::fault), then reads or mutates the in-memory
//! [`State`](crate::state::State). The behaviour mirrors the filesystem backend
//! contract documented in `docs/reference/in-memory-backend.md`.
//!
//! The trait `impl` lives here as a thin facade: each method delegates to a
//! domain-grouped helper (`ci`, `issues`, `pull_requests`, `repositories`) so
//! the operation logic stays organised by responsibility.

mod ci;
mod issues;
mod provisioning;
mod pull_requests;
mod repositories;

use crate::MemoryForge;
use async_trait::async_trait;
use temper_forge_model::{
    CiJob, CiJobId, CiJobQuery, Comment, CreateComment, CreateIssue, CreatePullRequest,
    CreatePullRequestReview, CreateRepository, Forge, ForgeResult, Issue, IssueId, IssueQuery,
    ItemNumber, Label, MergePullRequest, MergeRecord, PullRequest, PullRequestId, PullRequestQuery,
    PullRequestReview, Repository, RepositoryId, RepositoryPath, RepositoryQuery, RequestReviewers,
    UpdateIssue, UpdatePullRequest, UpsertLabel, User, UserId,
};

#[async_trait]
impl Forge for MemoryForge {
    async fn current_user(&self) -> ForgeResult<User> {
        repositories::current_user(self)
    }

    async fn get_user(&self, id: &UserId) -> ForgeResult<Option<User>> {
        repositories::get_user(self, id)
    }

    async fn list_repositories(&self, query: RepositoryQuery) -> ForgeResult<Vec<Repository>> {
        repositories::list_repositories(self, query)
    }

    async fn create_repository(&self, input: CreateRepository) -> ForgeResult<Repository> {
        repositories::create_repository(self, input)
    }

    async fn get_repository(&self, id: &RepositoryId) -> ForgeResult<Option<Repository>> {
        repositories::get_repository(self, id)
    }

    async fn get_repository_by_path(
        &self,
        path: &RepositoryPath,
    ) -> ForgeResult<Option<Repository>> {
        repositories::get_repository_by_path(self, path)
    }

    async fn list_labels(&self, repo_id: &RepositoryId) -> ForgeResult<Vec<Label>> {
        repositories::list_labels(self, repo_id)
    }

    async fn upsert_label(&self, repo_id: &RepositoryId, input: UpsertLabel) -> ForgeResult<Label> {
        repositories::upsert_label(self, repo_id, input)
    }

    async fn list_issues(
        &self,
        repo_id: &RepositoryId,
        query: IssueQuery,
    ) -> ForgeResult<Vec<Issue>> {
        issues::list_issues(self, repo_id, query)
    }

    async fn create_issue(&self, repo_id: &RepositoryId, input: CreateIssue) -> ForgeResult<Issue> {
        issues::create_issue(self, repo_id, input)
    }

    async fn get_issue(&self, id: &IssueId) -> ForgeResult<Option<Issue>> {
        issues::get_issue(self, id)
    }

    async fn get_issue_by_number(
        &self,
        repo_id: &RepositoryId,
        number: ItemNumber,
    ) -> ForgeResult<Option<Issue>> {
        issues::get_issue_by_number(self, repo_id, number)
    }

    async fn update_issue(&self, id: &IssueId, input: UpdateIssue) -> ForgeResult<Issue> {
        issues::update_issue(self, id, input)
    }

    async fn add_issue_dependency(&self, id: &IssueId, target: ItemNumber) -> ForgeResult<Issue> {
        issues::add_dependency(self, id, target)
    }

    async fn remove_issue_dependency(
        &self,
        id: &IssueId,
        target: ItemNumber,
    ) -> ForgeResult<Issue> {
        issues::remove_dependency(self, id, target)
    }

    async fn list_issue_comments(&self, id: &IssueId) -> ForgeResult<Vec<Comment>> {
        issues::list_comments(self, id)
    }

    async fn add_issue_comment(&self, id: &IssueId, input: CreateComment) -> ForgeResult<Comment> {
        issues::add_comment(self, id, input)
    }

    async fn list_pull_requests(
        &self,
        repo_id: &RepositoryId,
        query: PullRequestQuery,
    ) -> ForgeResult<Vec<PullRequest>> {
        pull_requests::list_pull_requests(self, repo_id, query)
    }

    async fn create_pull_request(
        &self,
        repo_id: &RepositoryId,
        input: CreatePullRequest,
    ) -> ForgeResult<PullRequest> {
        pull_requests::create_pull_request(self, repo_id, input)
    }

    async fn get_pull_request(&self, id: &PullRequestId) -> ForgeResult<Option<PullRequest>> {
        pull_requests::get_pull_request(self, id)
    }

    async fn get_pull_request_by_number(
        &self,
        repo_id: &RepositoryId,
        number: ItemNumber,
    ) -> ForgeResult<Option<PullRequest>> {
        pull_requests::get_pull_request_by_number(self, repo_id, number)
    }

    async fn update_pull_request(
        &self,
        id: &PullRequestId,
        input: UpdatePullRequest,
    ) -> ForgeResult<PullRequest> {
        pull_requests::update_pull_request(self, id, input)
    }

    async fn add_pull_request_dependency(
        &self,
        id: &PullRequestId,
        target: ItemNumber,
    ) -> ForgeResult<PullRequest> {
        pull_requests::add_dependency(self, id, target)
    }

    async fn remove_pull_request_dependency(
        &self,
        id: &PullRequestId,
        target: ItemNumber,
    ) -> ForgeResult<PullRequest> {
        pull_requests::remove_dependency(self, id, target)
    }

    async fn request_pull_request_reviewers(
        &self,
        id: &PullRequestId,
        input: RequestReviewers,
    ) -> ForgeResult<PullRequest> {
        pull_requests::request_pull_request_reviewers(self, id, input)
    }

    async fn list_pull_request_reviews(
        &self,
        id: &PullRequestId,
    ) -> ForgeResult<Vec<PullRequestReview>> {
        pull_requests::list_pull_request_reviews(self, id)
    }

    async fn submit_pull_request_review(
        &self,
        id: &PullRequestId,
        input: CreatePullRequestReview,
    ) -> ForgeResult<PullRequestReview> {
        pull_requests::submit_pull_request_review(self, id, input)
    }

    async fn list_pull_request_comments(&self, id: &PullRequestId) -> ForgeResult<Vec<Comment>> {
        pull_requests::list_comments(self, id)
    }

    async fn add_pull_request_comment(
        &self,
        id: &PullRequestId,
        input: CreateComment,
    ) -> ForgeResult<Comment> {
        pull_requests::add_comment(self, id, input)
    }

    async fn merge_pull_request(
        &self,
        id: &PullRequestId,
        input: MergePullRequest,
    ) -> ForgeResult<MergeRecord> {
        pull_requests::merge_pull_request(self, id, input)
    }

    async fn list_ci_jobs(
        &self,
        repo_id: &RepositoryId,
        query: CiJobQuery,
    ) -> ForgeResult<Vec<CiJob>> {
        ci::list_ci_jobs(self, repo_id, query)
    }

    async fn get_ci_job(&self, id: &CiJobId) -> ForgeResult<Option<CiJob>> {
        ci::get_ci_job(self, id)
    }
}
