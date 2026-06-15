//! [`Forge`] trait implementation for [`FilesystemForge`].
//!
//! This module is a thin facade: each trait method delegates to a domain
//! function grouped by responsibility — [`repositories`] (users, repositories,
//! labels), [`issues`] (issues, issue dependencies, issue comments),
//! [`pull_requests`] (pull requests and their dependencies),
//! [`pull_request_reviews`] (reviewers, reviews, comments, merges), and
//! [`ci_jobs`] (CI job listing and lookup).

mod ci_jobs;
mod issues;
mod pull_request_reviews;
mod pull_requests;
mod repositories;

use crate::FilesystemForge;
use async_trait::async_trait;
use temper_forge::{
    CiJob, CiJobId, CiJobQuery, Comment, CreateComment, CreateIssue, CreatePullRequest,
    CreatePullRequestReview, CreateRepository, Forge, ForgeResult, Issue, IssueId, IssueQuery,
    ItemNumber, Label, MergePullRequest, MergeRecord, PullRequest, PullRequestId, PullRequestQuery,
    PullRequestReview, Repository, RepositoryId, RepositoryPath, RepositoryQuery, RequestReviewers,
    UpdateIssue, UpdatePullRequest, UpsertLabel, User, UserId,
};

#[async_trait]
impl Forge for FilesystemForge {
    async fn current_user(&self) -> ForgeResult<User> {
        repositories::current_user(self)
    }

    async fn get_user(&self, id: &UserId) -> ForgeResult<Option<User>> {
        let user = self.current_user().await?;
        Ok(repositories::get_user(user, id))
    }

    async fn list_repositories(&self, query: RepositoryQuery) -> ForgeResult<Vec<Repository>> {
        repositories::list_repositories(self, query)
    }

    async fn create_repository(&self, input: CreateRepository) -> ForgeResult<Repository> {
        repositories::create_repository(self, input)
    }

    async fn get_repository(&self, id: &RepositoryId) -> ForgeResult<Option<Repository>> {
        self.find_repository_by_id(id)
    }

    async fn get_repository_by_path(
        &self,
        path: &RepositoryPath,
    ) -> ForgeResult<Option<Repository>> {
        self.find_repository_by_path(path)
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
        self.find_issue_by_id(id)
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
        issues::add_issue_dependency_op(self, id, target)
    }

    async fn remove_issue_dependency(
        &self,
        id: &IssueId,
        target: ItemNumber,
    ) -> ForgeResult<Issue> {
        issues::remove_issue_dependency_op(self, id, target)
    }

    async fn list_issue_comments(&self, id: &IssueId) -> ForgeResult<Vec<Comment>> {
        issues::list_issue_comments(self, id)
    }

    async fn add_issue_comment(&self, id: &IssueId, input: CreateComment) -> ForgeResult<Comment> {
        issues::add_issue_comment(self, id, input)
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
        self.find_pull_request_by_id(id)
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
        pull_requests::add_pull_request_dependency_op(self, id, target)
    }

    async fn remove_pull_request_dependency(
        &self,
        id: &PullRequestId,
        target: ItemNumber,
    ) -> ForgeResult<PullRequest> {
        pull_requests::remove_pull_request_dependency_op(self, id, target)
    }

    async fn request_pull_request_reviewers(
        &self,
        id: &PullRequestId,
        input: RequestReviewers,
    ) -> ForgeResult<PullRequest> {
        pull_request_reviews::request_pull_request_reviewers(self, id, input)
    }

    async fn list_pull_request_reviews(
        &self,
        id: &PullRequestId,
    ) -> ForgeResult<Vec<PullRequestReview>> {
        pull_request_reviews::list_pull_request_reviews(self, id)
    }

    async fn submit_pull_request_review(
        &self,
        id: &PullRequestId,
        input: CreatePullRequestReview,
    ) -> ForgeResult<PullRequestReview> {
        pull_request_reviews::submit_pull_request_review(self, id, input)
    }

    async fn list_pull_request_comments(&self, id: &PullRequestId) -> ForgeResult<Vec<Comment>> {
        pull_request_reviews::list_pull_request_comments(self, id)
    }

    async fn add_pull_request_comment(
        &self,
        id: &PullRequestId,
        input: CreateComment,
    ) -> ForgeResult<Comment> {
        pull_request_reviews::add_pull_request_comment(self, id, input)
    }

    async fn merge_pull_request(
        &self,
        id: &PullRequestId,
        input: MergePullRequest,
    ) -> ForgeResult<MergeRecord> {
        pull_request_reviews::merge_pull_request(self, id, input)
    }

    async fn list_ci_jobs(
        &self,
        repo_id: &RepositoryId,
        query: CiJobQuery,
    ) -> ForgeResult<Vec<CiJob>> {
        ci_jobs::list_ci_jobs(self, repo_id, query)
    }

    async fn get_ci_job(&self, id: &CiJobId) -> ForgeResult<Option<CiJob>> {
        ci_jobs::get_ci_job(self, id)
    }
}
