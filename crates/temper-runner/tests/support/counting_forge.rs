use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Mutex;
use temper_forge::{
    CiJob, CiJobId, CiJobQuery, Comment, CreateComment, CreateIssue, CreatePullRequest,
    CreatePullRequestReview, CreateRepository, Forge, ForgeError, ForgeResult, Issue, IssueId,
    IssueQuery, ItemNumber, Label, MergePullRequest, MergeRecord, PullRequest, PullRequestId,
    PullRequestQuery, PullRequestReview, Repository, RepositoryId, RepositoryPath, RepositoryQuery,
    RequestReviewers, UpdateIssue, UpdatePullRequest, UpsertLabel, User, UserId,
};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum CountedForgeOp {
    GetIssueByNumber,
    GetPullRequestByNumber,
    ListCiJobs,
    ListIssues,
    ListPullRequestReviews,
    ListPullRequests,
    MergePullRequest,
}

pub struct CountingForge<F: Forge> {
    inner: F,
    counts: Mutex<HashMap<CountedForgeOp, usize>>,
    merge_conflicts: Mutex<HashMap<PullRequestId, String>>,
    issue_queries: Mutex<Vec<IssueQuery>>,
    pull_request_queries: Mutex<Vec<PullRequestQuery>>,
}

impl<F: Forge> CountingForge<F> {
    pub fn new(inner: F) -> Self {
        Self {
            inner,
            counts: Mutex::new(HashMap::new()),
            merge_conflicts: Mutex::new(HashMap::new()),
            issue_queries: Mutex::new(Vec::new()),
            pull_request_queries: Mutex::new(Vec::new()),
        }
    }

    pub fn count(&self, op: CountedForgeOp) -> usize {
        *self
            .counts
            .lock()
            .expect("counts mutex")
            .get(&op)
            .unwrap_or(&0)
    }

    #[allow(dead_code)]
    pub fn reject_merge_for(&self, id: PullRequestId, message: impl Into<String>) {
        self.merge_conflicts
            .lock()
            .expect("merge conflicts mutex")
            .insert(id, message.into());
    }

    #[allow(dead_code)]
    pub fn issue_queries(&self) -> Vec<IssueQuery> {
        self.issue_queries
            .lock()
            .expect("issue query mutex")
            .clone()
    }

    #[allow(dead_code)]
    pub fn pull_request_queries(&self) -> Vec<PullRequestQuery> {
        self.pull_request_queries
            .lock()
            .expect("pull request query mutex")
            .clone()
    }

    fn tick(&self, op: CountedForgeOp) {
        let mut counts = self.counts.lock().expect("counts mutex");
        *counts.entry(op).or_insert(0) += 1;
    }

    fn record_issue_query(&self, query: &IssueQuery) {
        self.issue_queries
            .lock()
            .expect("issue query mutex")
            .push(query.clone());
    }

    fn record_pull_request_query(&self, query: &PullRequestQuery) {
        self.pull_request_queries
            .lock()
            .expect("pull request query mutex")
            .push(query.clone());
    }
}

#[async_trait]
impl<F: Forge> Forge for CountingForge<F> {
    async fn current_user(&self) -> ForgeResult<User> {
        self.inner.current_user().await
    }

    async fn get_user(&self, id: &UserId) -> ForgeResult<Option<User>> {
        self.inner.get_user(id).await
    }

    async fn list_repositories(&self, query: RepositoryQuery) -> ForgeResult<Vec<Repository>> {
        self.inner.list_repositories(query).await
    }

    async fn create_repository(&self, input: CreateRepository) -> ForgeResult<Repository> {
        self.inner.create_repository(input).await
    }

    async fn get_repository(&self, id: &RepositoryId) -> ForgeResult<Option<Repository>> {
        self.inner.get_repository(id).await
    }

    async fn get_repository_by_path(
        &self,
        path: &RepositoryPath,
    ) -> ForgeResult<Option<Repository>> {
        self.inner.get_repository_by_path(path).await
    }

    async fn list_labels(&self, repo_id: &RepositoryId) -> ForgeResult<Vec<Label>> {
        self.inner.list_labels(repo_id).await
    }

    async fn upsert_label(&self, repo_id: &RepositoryId, input: UpsertLabel) -> ForgeResult<Label> {
        self.inner.upsert_label(repo_id, input).await
    }

    async fn list_issues(
        &self,
        repo_id: &RepositoryId,
        query: IssueQuery,
    ) -> ForgeResult<Vec<Issue>> {
        self.tick(CountedForgeOp::ListIssues);
        self.record_issue_query(&query);
        self.inner.list_issues(repo_id, query).await
    }

    async fn create_issue(&self, repo_id: &RepositoryId, input: CreateIssue) -> ForgeResult<Issue> {
        self.inner.create_issue(repo_id, input).await
    }

    async fn get_issue(&self, id: &IssueId) -> ForgeResult<Option<Issue>> {
        self.inner.get_issue(id).await
    }

    async fn get_issue_by_number(
        &self,
        repo_id: &RepositoryId,
        number: ItemNumber,
    ) -> ForgeResult<Option<Issue>> {
        self.tick(CountedForgeOp::GetIssueByNumber);
        self.inner.get_issue_by_number(repo_id, number).await
    }

    async fn update_issue(&self, id: &IssueId, input: UpdateIssue) -> ForgeResult<Issue> {
        self.inner.update_issue(id, input).await
    }

    async fn add_issue_dependency(&self, id: &IssueId, target: ItemNumber) -> ForgeResult<Issue> {
        self.inner.add_issue_dependency(id, target).await
    }

    async fn remove_issue_dependency(
        &self,
        id: &IssueId,
        target: ItemNumber,
    ) -> ForgeResult<Issue> {
        self.inner.remove_issue_dependency(id, target).await
    }

    async fn list_issue_comments(&self, id: &IssueId) -> ForgeResult<Vec<Comment>> {
        self.inner.list_issue_comments(id).await
    }

    async fn add_issue_comment(&self, id: &IssueId, input: CreateComment) -> ForgeResult<Comment> {
        self.inner.add_issue_comment(id, input).await
    }

    async fn list_pull_requests(
        &self,
        repo_id: &RepositoryId,
        query: PullRequestQuery,
    ) -> ForgeResult<Vec<PullRequest>> {
        self.tick(CountedForgeOp::ListPullRequests);
        self.record_pull_request_query(&query);
        self.inner.list_pull_requests(repo_id, query).await
    }

    async fn create_pull_request(
        &self,
        repo_id: &RepositoryId,
        input: CreatePullRequest,
    ) -> ForgeResult<PullRequest> {
        self.inner.create_pull_request(repo_id, input).await
    }

    async fn get_pull_request(&self, id: &PullRequestId) -> ForgeResult<Option<PullRequest>> {
        self.inner.get_pull_request(id).await
    }

    async fn get_pull_request_by_number(
        &self,
        repo_id: &RepositoryId,
        number: ItemNumber,
    ) -> ForgeResult<Option<PullRequest>> {
        self.tick(CountedForgeOp::GetPullRequestByNumber);
        self.inner.get_pull_request_by_number(repo_id, number).await
    }

    async fn update_pull_request(
        &self,
        id: &PullRequestId,
        input: UpdatePullRequest,
    ) -> ForgeResult<PullRequest> {
        self.inner.update_pull_request(id, input).await
    }

    async fn add_pull_request_dependency(
        &self,
        id: &PullRequestId,
        target: ItemNumber,
    ) -> ForgeResult<PullRequest> {
        self.inner.add_pull_request_dependency(id, target).await
    }

    async fn remove_pull_request_dependency(
        &self,
        id: &PullRequestId,
        target: ItemNumber,
    ) -> ForgeResult<PullRequest> {
        self.inner.remove_pull_request_dependency(id, target).await
    }

    async fn request_pull_request_reviewers(
        &self,
        id: &PullRequestId,
        input: RequestReviewers,
    ) -> ForgeResult<PullRequest> {
        self.inner.request_pull_request_reviewers(id, input).await
    }

    async fn list_pull_request_reviews(
        &self,
        id: &PullRequestId,
    ) -> ForgeResult<Vec<PullRequestReview>> {
        self.tick(CountedForgeOp::ListPullRequestReviews);
        self.inner.list_pull_request_reviews(id).await
    }

    async fn submit_pull_request_review(
        &self,
        id: &PullRequestId,
        input: CreatePullRequestReview,
    ) -> ForgeResult<PullRequestReview> {
        self.inner.submit_pull_request_review(id, input).await
    }

    async fn list_pull_request_comments(&self, id: &PullRequestId) -> ForgeResult<Vec<Comment>> {
        self.inner.list_pull_request_comments(id).await
    }

    async fn add_pull_request_comment(
        &self,
        id: &PullRequestId,
        input: CreateComment,
    ) -> ForgeResult<Comment> {
        self.inner.add_pull_request_comment(id, input).await
    }

    async fn merge_pull_request(
        &self,
        id: &PullRequestId,
        input: MergePullRequest,
    ) -> ForgeResult<MergeRecord> {
        self.tick(CountedForgeOp::MergePullRequest);
        if let Some(message) = self
            .merge_conflicts
            .lock()
            .expect("merge conflicts mutex")
            .get(id)
            .cloned()
        {
            return Err(ForgeError::Conflict(message));
        }
        self.inner.merge_pull_request(id, input).await
    }

    async fn list_ci_jobs(
        &self,
        repo_id: &RepositoryId,
        query: CiJobQuery,
    ) -> ForgeResult<Vec<CiJob>> {
        self.tick(CountedForgeOp::ListCiJobs);
        self.inner.list_ci_jobs(repo_id, query).await
    }

    async fn get_ci_job(&self, id: &CiJobId) -> ForgeResult<Option<CiJob>> {
        self.inner.get_ci_job(id).await
    }
}
