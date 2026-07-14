use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Mutex;
use temper_forge_model::{
    CiJob, CiJobId, CiJobQuery, Comment, CreateComment, CreateIssue, CreatePullRequest,
    CreatePullRequestReview, CreateRepository, Forge, ForgeError, ForgeResult, Issue, IssueId,
    IssueQuery, ItemListDetails, ItemNumber, Label, MergePullRequest, MergeRecord, PullRequest,
    PullRequestId, PullRequestQuery, PullRequestReview, Repository, RepositoryId, RepositoryPath,
    RepositoryQuery, RequestReviewers, UpdateIssue, UpdatePullRequest, UpsertLabel, User, UserId,
};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum CountedForgeOp {
    CurrentUser,
    GetUser,
    ListRepositories,
    CreateRepository,
    GetRepository,
    GetRepositoryByPath,
    ListLabels,
    UpsertLabel,
    ListIssues,
    CreateIssue,
    GetIssue,
    GetIssueByNumber,
    UpdateIssue,
    AddIssueDependency,
    RemoveIssueDependency,
    ListIssueComments,
    AddIssueComment,
    ListPullRequests,
    CreatePullRequest,
    GetPullRequest,
    GetPullRequestByNumber,
    UpdatePullRequest,
    AddPullRequestDependency,
    RemovePullRequestDependency,
    RequestPullRequestReviewers,
    ListPullRequestReviews,
    SubmitPullRequestReview,
    ListPullRequestComments,
    AddPullRequestComment,
    MergePullRequest,
    ListCiJobs,
    GetCiJob,
}

impl CountedForgeOp {
    pub const fn is_write(self) -> bool {
        matches!(
            self,
            Self::CreateRepository
                | Self::UpsertLabel
                | Self::CreateIssue
                | Self::UpdateIssue
                | Self::AddIssueDependency
                | Self::RemoveIssueDependency
                | Self::AddIssueComment
                | Self::CreatePullRequest
                | Self::UpdatePullRequest
                | Self::AddPullRequestDependency
                | Self::RemovePullRequestDependency
                | Self::RequestPullRequestReviewers
                | Self::SubmitPullRequestReview
                | Self::AddPullRequestComment
                | Self::MergePullRequest
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExactIssueRead {
    pub by_number: bool,
    pub details: ItemListDetails,
}

pub struct CountingForge<F: Forge> {
    inner: F,
    counts: Mutex<HashMap<CountedForgeOp, usize>>,
    merge_conflicts: Mutex<HashMap<PullRequestId, String>>,
    synthetic_heads: Mutex<bool>,
    head_overrides: Mutex<HashMap<PullRequestId, String>>,
    head_generations: Mutex<HashMap<PullRequestId, u64>>,
    advance_heads_on_conflict_resolution: Mutex<bool>,
    issue_queries: Mutex<Vec<IssueQuery>>,
    pull_request_queries: Mutex<Vec<PullRequestQuery>>,
    exact_issue_reads: Mutex<Vec<ExactIssueRead>>,
}

impl<F: Forge> CountingForge<F> {
    pub fn new(inner: F) -> Self {
        Self {
            inner,
            counts: Mutex::new(HashMap::new()),
            merge_conflicts: Mutex::new(HashMap::new()),
            synthetic_heads: Mutex::new(false),
            head_overrides: Mutex::new(HashMap::new()),
            head_generations: Mutex::new(HashMap::new()),
            advance_heads_on_conflict_resolution: Mutex::new(false),
            issue_queries: Mutex::new(Vec::new()),
            pull_request_queries: Mutex::new(Vec::new()),
            exact_issue_reads: Mutex::new(Vec::new()),
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

    pub fn read_count(&self) -> usize {
        self.counts
            .lock()
            .expect("counts mutex")
            .iter()
            .filter(|(op, _)| !op.is_write())
            .map(|(_, count)| count)
            .sum()
    }

    pub fn write_count(&self) -> usize {
        self.counts
            .lock()
            .expect("counts mutex")
            .iter()
            .filter(|(op, _)| op.is_write())
            .map(|(_, count)| count)
            .sum()
    }

    #[allow(dead_code)]
    pub fn reject_merge_for(&self, id: PullRequestId, message: impl Into<String>) {
        self.merge_conflicts
            .lock()
            .expect("merge conflicts mutex")
            .insert(id, message.into());
    }

    #[allow(dead_code)]
    pub fn allow_merge_for(&self, id: &PullRequestId) {
        self.merge_conflicts
            .lock()
            .expect("merge conflicts mutex")
            .remove(id);
    }

    #[allow(dead_code)]
    pub fn enable_synthetic_pull_request_heads(&self) {
        *self.synthetic_heads.lock().expect("synthetic heads mutex") = true;
    }

    #[allow(dead_code)]
    pub fn advance_heads_on_conflict_resolution(&self) {
        *self
            .advance_heads_on_conflict_resolution
            .lock()
            .expect("advance heads mutex") = true;
    }

    #[allow(dead_code)]
    pub fn override_head_for(&self, id: PullRequestId, head: impl Into<String>) {
        self.head_overrides
            .lock()
            .expect("head overrides mutex")
            .insert(id, head.into());
    }

    #[allow(dead_code)]
    pub fn projected_head(&self, pull_request: &PullRequest) -> Option<String> {
        self.project_pull_request(pull_request.clone()).head_sha
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

    pub fn exact_issue_reads(&self) -> Vec<ExactIssueRead> {
        self.exact_issue_reads
            .lock()
            .expect("exact issue reads mutex")
            .clone()
    }

    fn record_exact_issue_read(&self, by_number: bool, details: ItemListDetails) {
        self.exact_issue_reads
            .lock()
            .expect("exact issue reads mutex")
            .push(ExactIssueRead { by_number, details });
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

    fn project_pull_request(&self, mut pull_request: PullRequest) -> PullRequest {
        if let Some(head) = self
            .head_overrides
            .lock()
            .expect("head overrides mutex")
            .get(&pull_request.id)
            .cloned()
        {
            pull_request.head_sha = Some(head);
        } else if *self.synthetic_heads.lock().expect("synthetic heads mutex")
            && pull_request.head_sha.is_none()
        {
            pull_request.head_sha = Some(default_synthetic_head(pull_request.number));
        }
        pull_request
    }

    fn maybe_advance_head_after_update(
        &self,
        input: &UpdatePullRequest,
        updated: &PullRequest,
    ) -> Option<String> {
        let enabled = *self
            .advance_heads_on_conflict_resolution
            .lock()
            .expect("advance heads mutex");
        if !enabled
            || !input
                .remove_labels
                .iter()
                .any(|label| label == "merge-conflict")
        {
            return None;
        }
        let mut generations = self
            .head_generations
            .lock()
            .expect("head generations mutex");
        let generation = generations.entry(updated.id.clone()).or_insert(0);
        *generation = generation.saturating_add(1);
        let head = format!("pr-{}-resolved-head-{}", updated.number.get(), *generation);
        self.head_overrides
            .lock()
            .expect("head overrides mutex")
            .insert(updated.id.clone(), head.clone());
        Some(head)
    }
}

fn default_synthetic_head(number: ItemNumber) -> String {
    format!("pr-{}-head", number.get())
}

#[async_trait]
impl<F: Forge> Forge for CountingForge<F> {
    async fn current_user(&self) -> ForgeResult<User> {
        self.tick(CountedForgeOp::CurrentUser);
        self.inner.current_user().await
    }

    async fn get_user(&self, id: &UserId) -> ForgeResult<Option<User>> {
        self.tick(CountedForgeOp::GetUser);
        self.inner.get_user(id).await
    }

    async fn list_repositories(&self, query: RepositoryQuery) -> ForgeResult<Vec<Repository>> {
        self.tick(CountedForgeOp::ListRepositories);
        self.inner.list_repositories(query).await
    }

    async fn create_repository(&self, input: CreateRepository) -> ForgeResult<Repository> {
        self.tick(CountedForgeOp::CreateRepository);
        self.inner.create_repository(input).await
    }

    async fn get_repository(&self, id: &RepositoryId) -> ForgeResult<Option<Repository>> {
        self.tick(CountedForgeOp::GetRepository);
        self.inner.get_repository(id).await
    }

    async fn get_repository_by_path(
        &self,
        path: &RepositoryPath,
    ) -> ForgeResult<Option<Repository>> {
        self.tick(CountedForgeOp::GetRepositoryByPath);
        self.inner.get_repository_by_path(path).await
    }

    async fn list_labels(&self, repo_id: &RepositoryId) -> ForgeResult<Vec<Label>> {
        self.tick(CountedForgeOp::ListLabels);
        self.inner.list_labels(repo_id).await
    }

    async fn upsert_label(&self, repo_id: &RepositoryId, input: UpsertLabel) -> ForgeResult<Label> {
        self.tick(CountedForgeOp::UpsertLabel);
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
        self.tick(CountedForgeOp::CreateIssue);
        self.inner.create_issue(repo_id, input).await
    }

    async fn get_issue(&self, id: &IssueId) -> ForgeResult<Option<Issue>> {
        self.tick(CountedForgeOp::GetIssue);
        self.record_exact_issue_read(false, ItemListDetails::full());
        self.inner.get_issue(id).await
    }

    async fn get_issue_with_details(
        &self,
        id: &IssueId,
        details: ItemListDetails,
    ) -> ForgeResult<Option<Issue>> {
        self.tick(CountedForgeOp::GetIssue);
        self.record_exact_issue_read(false, details);
        self.inner.get_issue_with_details(id, details).await
    }

    async fn get_issue_by_number(
        &self,
        repo_id: &RepositoryId,
        number: ItemNumber,
    ) -> ForgeResult<Option<Issue>> {
        self.tick(CountedForgeOp::GetIssueByNumber);
        self.record_exact_issue_read(true, ItemListDetails::full());
        self.inner.get_issue_by_number(repo_id, number).await
    }

    async fn get_issue_by_number_with_details(
        &self,
        repo_id: &RepositoryId,
        number: ItemNumber,
        details: ItemListDetails,
    ) -> ForgeResult<Option<Issue>> {
        self.tick(CountedForgeOp::GetIssueByNumber);
        self.record_exact_issue_read(true, details);
        self.inner
            .get_issue_by_number_with_details(repo_id, number, details)
            .await
    }

    async fn update_issue(&self, id: &IssueId, input: UpdateIssue) -> ForgeResult<Issue> {
        self.tick(CountedForgeOp::UpdateIssue);
        self.inner.update_issue(id, input).await
    }

    async fn update_issue_from_snapshot(
        &self,
        current: &Issue,
        input: UpdateIssue,
    ) -> ForgeResult<Issue> {
        self.tick(CountedForgeOp::UpdateIssue);
        self.inner.update_issue_from_snapshot(current, input).await
    }

    async fn add_issue_dependency(&self, id: &IssueId, target: ItemNumber) -> ForgeResult<Issue> {
        self.tick(CountedForgeOp::AddIssueDependency);
        self.inner.add_issue_dependency(id, target).await
    }

    async fn remove_issue_dependency(
        &self,
        id: &IssueId,
        target: ItemNumber,
    ) -> ForgeResult<Issue> {
        self.tick(CountedForgeOp::RemoveIssueDependency);
        self.inner.remove_issue_dependency(id, target).await
    }

    async fn list_issue_comments(&self, id: &IssueId) -> ForgeResult<Vec<Comment>> {
        self.tick(CountedForgeOp::ListIssueComments);
        self.inner.list_issue_comments(id).await
    }

    async fn add_issue_comment(&self, id: &IssueId, input: CreateComment) -> ForgeResult<Comment> {
        self.tick(CountedForgeOp::AddIssueComment);
        self.inner.add_issue_comment(id, input).await
    }

    async fn list_pull_requests(
        &self,
        repo_id: &RepositoryId,
        query: PullRequestQuery,
    ) -> ForgeResult<Vec<PullRequest>> {
        self.tick(CountedForgeOp::ListPullRequests);
        self.record_pull_request_query(&query);
        Ok(self
            .inner
            .list_pull_requests(repo_id, query)
            .await?
            .into_iter()
            .map(|pull_request| self.project_pull_request(pull_request))
            .collect())
    }

    async fn create_pull_request(
        &self,
        repo_id: &RepositoryId,
        input: CreatePullRequest,
    ) -> ForgeResult<PullRequest> {
        self.tick(CountedForgeOp::CreatePullRequest);
        self.inner
            .create_pull_request(repo_id, input)
            .await
            .map(|pull_request| self.project_pull_request(pull_request))
    }

    async fn get_pull_request(&self, id: &PullRequestId) -> ForgeResult<Option<PullRequest>> {
        self.tick(CountedForgeOp::GetPullRequest);
        Ok(self
            .inner
            .get_pull_request(id)
            .await?
            .map(|pull_request| self.project_pull_request(pull_request)))
    }

    async fn get_pull_request_by_number(
        &self,
        repo_id: &RepositoryId,
        number: ItemNumber,
    ) -> ForgeResult<Option<PullRequest>> {
        self.tick(CountedForgeOp::GetPullRequestByNumber);
        Ok(self
            .inner
            .get_pull_request_by_number(repo_id, number)
            .await?
            .map(|pull_request| self.project_pull_request(pull_request)))
    }

    async fn update_pull_request(
        &self,
        id: &PullRequestId,
        input: UpdatePullRequest,
    ) -> ForgeResult<PullRequest> {
        self.tick(CountedForgeOp::UpdatePullRequest);
        let updated = self.inner.update_pull_request(id, input.clone()).await?;
        let mut projected = self.project_pull_request(updated.clone());
        if let Some(head) = self.maybe_advance_head_after_update(&input, &updated) {
            projected.head_sha = Some(head);
        }
        Ok(projected)
    }

    async fn add_pull_request_dependency(
        &self,
        id: &PullRequestId,
        target: ItemNumber,
    ) -> ForgeResult<PullRequest> {
        self.tick(CountedForgeOp::AddPullRequestDependency);
        self.inner.add_pull_request_dependency(id, target).await
    }

    async fn remove_pull_request_dependency(
        &self,
        id: &PullRequestId,
        target: ItemNumber,
    ) -> ForgeResult<PullRequest> {
        self.tick(CountedForgeOp::RemovePullRequestDependency);
        self.inner.remove_pull_request_dependency(id, target).await
    }

    async fn request_pull_request_reviewers(
        &self,
        id: &PullRequestId,
        input: RequestReviewers,
    ) -> ForgeResult<PullRequest> {
        self.tick(CountedForgeOp::RequestPullRequestReviewers);
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
        self.tick(CountedForgeOp::SubmitPullRequestReview);
        self.inner.submit_pull_request_review(id, input).await
    }

    async fn list_pull_request_comments(&self, id: &PullRequestId) -> ForgeResult<Vec<Comment>> {
        self.tick(CountedForgeOp::ListPullRequestComments);
        self.inner.list_pull_request_comments(id).await
    }

    async fn add_pull_request_comment(
        &self,
        id: &PullRequestId,
        input: CreateComment,
    ) -> ForgeResult<Comment> {
        self.tick(CountedForgeOp::AddPullRequestComment);
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
        self.tick(CountedForgeOp::GetCiJob);
        self.inner.get_ci_job(id).await
    }
}
