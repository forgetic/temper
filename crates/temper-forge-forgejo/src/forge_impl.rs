//! The `temper_forge_model::Forge` trait implementation for [`ForgejoForge`].
//!
//! This is the keystone that turns the backend's inherent methods into a usable
//! [`Forge`] backend: every trait method delegates, one line each, to the
//! identically-signed inherent method implemented across [`crate::repos`],
//! [`crate::issues`], [`crate::pulls`], [`crate::dependencies`], and
//! [`crate::ci`]. Keeping the bodies thin makes the inherent methods the single
//! source of truth; this module only asserts signature parity with the trait.
//!
//! The `C: HttpClient` bound carries the `Send + Sync` the trait requires (see
//! [`crate::HttpClient`]), so `ForgejoForge<C>` satisfies `Forge: Send + Sync`.

use crate::{ForgejoForge, HttpClient};
use temper_forge_model::{
    CiJob, CiJobId, CiJobQuery, Comment, CreateComment, CreateIssue, CreatePullRequest,
    CreatePullRequestReview, CreateRepository, Forge, ForgeResult, Issue, IssueId, IssueQuery,
    ItemListDetails, ItemNumber, Label, MergePullRequest, MergeRecord, PullRequest, PullRequestId,
    PullRequestQuery, PullRequestReview, Repository, RepositoryId, RepositoryPath, RepositoryQuery,
    RequestReviewers, UpdateIssue, UpdatePullRequest, UpsertLabel, User, UserId,
};

#[async_trait::async_trait]
impl<C: HttpClient> Forge for ForgejoForge<C> {
    async fn current_user(&self) -> ForgeResult<User> {
        self.current_user().await
    }

    async fn get_user(&self, id: &UserId) -> ForgeResult<Option<User>> {
        self.get_user(id).await
    }

    async fn list_repositories(&self, query: RepositoryQuery) -> ForgeResult<Vec<Repository>> {
        self.list_repositories(query).await
    }

    async fn create_repository(&self, input: CreateRepository) -> ForgeResult<Repository> {
        self.create_repository(input).await
    }

    async fn get_repository(&self, id: &RepositoryId) -> ForgeResult<Option<Repository>> {
        self.get_repository(id).await
    }

    async fn get_repository_by_path(
        &self,
        path: &RepositoryPath,
    ) -> ForgeResult<Option<Repository>> {
        self.get_repository_by_path(path).await
    }

    async fn list_labels(&self, repo_id: &RepositoryId) -> ForgeResult<Vec<Label>> {
        self.list_labels(repo_id).await
    }

    async fn upsert_label(&self, repo_id: &RepositoryId, input: UpsertLabel) -> ForgeResult<Label> {
        self.upsert_label(repo_id, input).await
    }

    async fn list_issues(
        &self,
        repo_id: &RepositoryId,
        query: IssueQuery,
    ) -> ForgeResult<Vec<Issue>> {
        self.list_issues(repo_id, query).await
    }

    async fn create_issue(&self, repo_id: &RepositoryId, input: CreateIssue) -> ForgeResult<Issue> {
        self.create_issue(repo_id, input).await
    }

    async fn get_issue(&self, id: &IssueId) -> ForgeResult<Option<Issue>> {
        self.get_issue(id).await
    }

    async fn get_issue_with_details(
        &self,
        id: &IssueId,
        details: ItemListDetails,
    ) -> ForgeResult<Option<Issue>> {
        self.get_issue_with_details(id, details).await
    }

    async fn get_issue_by_number(
        &self,
        repo_id: &RepositoryId,
        number: ItemNumber,
    ) -> ForgeResult<Option<Issue>> {
        self.get_issue_by_number(repo_id, number).await
    }

    async fn get_issue_by_number_with_details(
        &self,
        repo_id: &RepositoryId,
        number: ItemNumber,
        details: ItemListDetails,
    ) -> ForgeResult<Option<Issue>> {
        self.get_issue_by_number_with_details(repo_id, number, details)
            .await
    }

    async fn update_issue(&self, id: &IssueId, input: UpdateIssue) -> ForgeResult<Issue> {
        self.update_issue(id, input).await
    }

    async fn update_issue_from_snapshot(
        &self,
        current: &Issue,
        input: UpdateIssue,
    ) -> ForgeResult<Issue> {
        self.update_issue_from_snapshot(current, input).await
    }

    async fn add_issue_dependency(&self, id: &IssueId, target: ItemNumber) -> ForgeResult<Issue> {
        self.add_issue_dependency(id, target).await
    }

    async fn remove_issue_dependency(
        &self,
        id: &IssueId,
        target: ItemNumber,
    ) -> ForgeResult<Issue> {
        self.remove_issue_dependency(id, target).await
    }

    async fn list_issue_comments(&self, id: &IssueId) -> ForgeResult<Vec<Comment>> {
        self.list_issue_comments(id).await
    }

    async fn add_issue_comment(&self, id: &IssueId, input: CreateComment) -> ForgeResult<Comment> {
        self.add_issue_comment(id, input).await
    }

    async fn list_pull_requests(
        &self,
        repo_id: &RepositoryId,
        query: PullRequestQuery,
    ) -> ForgeResult<Vec<PullRequest>> {
        self.list_pull_requests(repo_id, query).await
    }

    async fn create_pull_request(
        &self,
        repo_id: &RepositoryId,
        input: CreatePullRequest,
    ) -> ForgeResult<PullRequest> {
        self.create_pull_request(repo_id, input).await
    }

    async fn get_pull_request(&self, id: &PullRequestId) -> ForgeResult<Option<PullRequest>> {
        self.get_pull_request(id).await
    }

    async fn get_pull_request_by_number(
        &self,
        repo_id: &RepositoryId,
        number: ItemNumber,
    ) -> ForgeResult<Option<PullRequest>> {
        self.get_pull_request_by_number(repo_id, number).await
    }

    async fn update_pull_request(
        &self,
        id: &PullRequestId,
        input: UpdatePullRequest,
    ) -> ForgeResult<PullRequest> {
        self.update_pull_request(id, input).await
    }

    async fn add_pull_request_dependency(
        &self,
        id: &PullRequestId,
        target: ItemNumber,
    ) -> ForgeResult<PullRequest> {
        self.add_pull_request_dependency(id, target).await
    }

    async fn remove_pull_request_dependency(
        &self,
        id: &PullRequestId,
        target: ItemNumber,
    ) -> ForgeResult<PullRequest> {
        self.remove_pull_request_dependency(id, target).await
    }

    async fn request_pull_request_reviewers(
        &self,
        id: &PullRequestId,
        input: RequestReviewers,
    ) -> ForgeResult<PullRequest> {
        self.request_pull_request_reviewers(id, input).await
    }

    async fn list_pull_request_reviews(
        &self,
        id: &PullRequestId,
    ) -> ForgeResult<Vec<PullRequestReview>> {
        self.list_pull_request_reviews(id).await
    }

    async fn submit_pull_request_review(
        &self,
        id: &PullRequestId,
        input: CreatePullRequestReview,
    ) -> ForgeResult<PullRequestReview> {
        self.submit_pull_request_review(id, input).await
    }

    async fn list_pull_request_comments(&self, id: &PullRequestId) -> ForgeResult<Vec<Comment>> {
        self.list_pull_request_comments(id).await
    }

    async fn add_pull_request_comment(
        &self,
        id: &PullRequestId,
        input: CreateComment,
    ) -> ForgeResult<Comment> {
        self.add_pull_request_comment(id, input).await
    }

    async fn merge_pull_request(
        &self,
        id: &PullRequestId,
        input: MergePullRequest,
    ) -> ForgeResult<MergeRecord> {
        self.merge_pull_request(id, input).await
    }

    async fn list_ci_jobs(
        &self,
        repo_id: &RepositoryId,
        query: CiJobQuery,
    ) -> ForgeResult<Vec<CiJob>> {
        self.list_ci_jobs(repo_id, query).await
    }

    async fn get_ci_job(&self, id: &CiJobId) -> ForgeResult<Option<CiJob>> {
        self.get_ci_job(id).await
    }
}

/// Compile-time proof that the production backend satisfies the `Forge` trait.
///
/// `ForgejoForge<EngineHttpClient>` is the type the runner constructs; this
/// never-called function fails to type-check if the trait impl or its bounds
/// regress.
const _: fn() = || {
    fn is_forge<T: Forge>() {}
    is_forge::<ForgejoForge>();
};

/// Compile-time proof that the production backend provides the full provisioning
/// surface: [`Forge`] plus the two capability traits, hence
/// [`ProvisioningForge`](temper_forge_model::ProvisioningForge).
const _: fn() = || {
    fn is_provisioning_forge<T: temper_forge_model::ProvisioningForge>() {}
    is_provisioning_forge::<ForgejoForge>();
};
