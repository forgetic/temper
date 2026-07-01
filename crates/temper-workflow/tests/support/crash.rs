//! A crash-injecting [`Forge`] wrapper for deterministic robustness tests.
//!
//! `CrashForge` wraps any [`Forge`] backend and can fail a chosen operation on a
//! chosen call. Faults are deterministic — they fire on a specific 1-based
//! occurrence of an operation — so a test never depends on timing or sleeps.
//!
//! Each fault has a [`FaultPoint`]:
//!
//! - [`FaultPoint::Before`] fails *before* delegating, so the backend is never
//!   touched. This models a crash on the way into a side effect: state is intact
//!   and a retry can complete cleanly.
//! - [`FaultPoint::After`] delegates first and *then* returns an error, so the
//!   backend mutation lands but the caller observes a failure. This is the
//!   dangerous "crashed right after the side effect" case the runtime must
//!   tolerate without double-applying on retry.
//!
//! Only the mutating and load operations the runtime exercises are fault-aware;
//! every other [`Forge`] method delegates straight through.
#![allow(dead_code)]

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

/// The fault-aware Forge operations.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ForgeOp {
    CreateIssue,
    UpdateIssue,
    GetIssueByNumber,
    ListIssues,
    ListIssuesDefault,
    CreatePullRequest,
    UpdatePullRequest,
    GetPullRequestByNumber,
    ListPullRequests,
    ListPullRequestsDefault,
    ListPullRequestReviews,
    ListCiJobs,
    MergePullRequest,
}

/// Where in an operation a fault fires.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FaultPoint {
    /// Fail before delegating: the backend is never touched.
    Before,
    /// Fail after delegating: the mutation lands, then the call returns an error.
    After,
}

/// Error category emitted by a deterministic fault.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FaultError {
    /// Emit a generic backend failure, modelling a crash or unavailable store.
    Backend,
    /// Emit a portable conflict, used by merge-conflict/rejection tests.
    Conflict,
}

/// A single deterministic fault: fail `op` on its `occurrence`-th call.
#[derive(Clone, Copy, Debug)]
pub struct Fault {
    pub op: ForgeOp,
    /// 1-based call index at which to fire.
    pub occurrence: usize,
    pub point: FaultPoint,
    pub error: FaultError,
}

impl Fault {
    /// A fault that fails before the backend is touched.
    pub fn before(op: ForgeOp, occurrence: usize) -> Self {
        Self {
            op,
            occurrence,
            point: FaultPoint::Before,
            error: FaultError::Backend,
        }
    }

    /// A fault that fails after the backend mutation has landed.
    pub fn after(op: ForgeOp, occurrence: usize) -> Self {
        Self {
            op,
            occurrence,
            point: FaultPoint::After,
            error: FaultError::Backend,
        }
    }

    /// A conflict fault that fails before the backend is touched.
    pub fn conflict_before(op: ForgeOp, occurrence: usize) -> Self {
        Self {
            op,
            occurrence,
            point: FaultPoint::Before,
            error: FaultError::Conflict,
        }
    }

    /// A conflict fault that fails after the backend mutation has landed.
    pub fn conflict_after(op: ForgeOp, occurrence: usize) -> Self {
        Self {
            op,
            occurrence,
            point: FaultPoint::After,
            error: FaultError::Conflict,
        }
    }
}

/// A [`Forge`] wrapper that injects deterministic faults.
pub struct CrashForge<F: Forge> {
    inner: F,
    faults: Vec<Fault>,
    counts: Mutex<HashMap<ForgeOp, usize>>,
    issue_queries: Mutex<Vec<IssueQuery>>,
    issue_updates: Mutex<Vec<UpdateIssue>>,
    pull_request_queries: Mutex<Vec<PullRequestQuery>>,
    merge_inputs: Mutex<Vec<MergePullRequest>>,
}

impl<F: Forge> CrashForge<F> {
    /// Wraps `inner`, arming the given faults.
    pub fn new(inner: F, faults: Vec<Fault>) -> Self {
        Self {
            inner,
            faults,
            counts: Mutex::new(HashMap::new()),
            issue_queries: Mutex::new(Vec::new()),
            issue_updates: Mutex::new(Vec::new()),
            pull_request_queries: Mutex::new(Vec::new()),
            merge_inputs: Mutex::new(Vec::new()),
        }
    }

    /// Borrows the wrapped backend for fault-free state inspection.
    pub fn inner(&self) -> &F {
        &self.inner
    }

    /// Returns how many times `op` has been called.
    pub fn count(&self, op: ForgeOp) -> usize {
        *self
            .counts
            .lock()
            .expect("counts mutex")
            .get(&op)
            .unwrap_or(&0)
    }

    /// Returns the issue list queries this wrapper observed.
    pub fn issue_queries(&self) -> Vec<IssueQuery> {
        self.issue_queries
            .lock()
            .expect("issue queries mutex")
            .clone()
    }

    /// Returns the issue updates this wrapper observed.
    pub fn issue_updates(&self) -> Vec<UpdateIssue> {
        self.issue_updates
            .lock()
            .expect("issue updates mutex")
            .clone()
    }

    /// Returns the pull-request list queries this wrapper observed.
    pub fn pull_request_queries(&self) -> Vec<PullRequestQuery> {
        self.pull_request_queries
            .lock()
            .expect("pull request queries mutex")
            .clone()
    }

    /// Returns the pull-request merge inputs this wrapper observed.
    pub fn merge_inputs(&self) -> Vec<MergePullRequest> {
        self.merge_inputs
            .lock()
            .expect("merge inputs mutex")
            .clone()
    }

    /// Records a call to `op` and returns its 1-based occurrence.
    fn tick(&self, op: ForgeOp) -> usize {
        let mut counts = self.counts.lock().expect("counts mutex");
        let entry = counts.entry(op).or_insert(0);
        *entry += 1;
        *entry
    }

    /// Fails when a fault is armed for `op` at this `occurrence` and `point`.
    fn guard(&self, op: ForgeOp, occurrence: usize, point: FaultPoint) -> ForgeResult<()> {
        let fault = self
            .faults
            .iter()
            .find(|fault| fault.op == op && fault.occurrence == occurrence && fault.point == point);
        match fault.map(|fault| fault.error) {
            Some(FaultError::Backend) => Err(ForgeError::Backend(format!(
                "injected fault: {op:?} {point:?} on call #{occurrence}"
            ))),
            Some(FaultError::Conflict) => Err(ForgeError::Conflict(format!(
                "injected conflict: {op:?} {point:?} on call #{occurrence}"
            ))),
            None => Ok(()),
        }
    }
}

#[async_trait]
impl<F: Forge> Forge for CrashForge<F> {
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
        let n = self.tick(ForgeOp::ListIssues);
        if query == IssueQuery::default() {
            self.tick(ForgeOp::ListIssuesDefault);
        }
        self.issue_queries
            .lock()
            .expect("issue queries mutex")
            .push(query.clone());
        self.guard(ForgeOp::ListIssues, n, FaultPoint::Before)?;
        let result = self.inner.list_issues(repo_id, query).await?;
        self.guard(ForgeOp::ListIssues, n, FaultPoint::After)?;
        Ok(result)
    }

    async fn create_issue(&self, repo_id: &RepositoryId, input: CreateIssue) -> ForgeResult<Issue> {
        let n = self.tick(ForgeOp::CreateIssue);
        self.guard(ForgeOp::CreateIssue, n, FaultPoint::Before)?;
        let result = self.inner.create_issue(repo_id, input).await?;
        self.guard(ForgeOp::CreateIssue, n, FaultPoint::After)?;
        Ok(result)
    }

    async fn get_issue(&self, id: &IssueId) -> ForgeResult<Option<Issue>> {
        self.inner.get_issue(id).await
    }

    async fn get_issue_by_number(
        &self,
        repo_id: &RepositoryId,
        number: ItemNumber,
    ) -> ForgeResult<Option<Issue>> {
        let n = self.tick(ForgeOp::GetIssueByNumber);
        self.guard(ForgeOp::GetIssueByNumber, n, FaultPoint::Before)?;
        let result = self.inner.get_issue_by_number(repo_id, number).await?;
        self.guard(ForgeOp::GetIssueByNumber, n, FaultPoint::After)?;
        Ok(result)
    }

    async fn update_issue(&self, id: &IssueId, input: UpdateIssue) -> ForgeResult<Issue> {
        let n = self.tick(ForgeOp::UpdateIssue);
        self.guard(ForgeOp::UpdateIssue, n, FaultPoint::Before)?;
        self.issue_updates
            .lock()
            .expect("issue updates mutex")
            .push(input.clone());
        let result = self.inner.update_issue(id, input).await?;
        self.guard(ForgeOp::UpdateIssue, n, FaultPoint::After)?;
        Ok(result)
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
        let n = self.tick(ForgeOp::ListPullRequests);
        if query == PullRequestQuery::default() {
            self.tick(ForgeOp::ListPullRequestsDefault);
        }
        self.pull_request_queries
            .lock()
            .expect("pull request queries mutex")
            .push(query.clone());
        self.guard(ForgeOp::ListPullRequests, n, FaultPoint::Before)?;
        let result = self.inner.list_pull_requests(repo_id, query).await?;
        self.guard(ForgeOp::ListPullRequests, n, FaultPoint::After)?;
        Ok(result)
    }

    async fn create_pull_request(
        &self,
        repo_id: &RepositoryId,
        input: CreatePullRequest,
    ) -> ForgeResult<PullRequest> {
        let n = self.tick(ForgeOp::CreatePullRequest);
        self.guard(ForgeOp::CreatePullRequest, n, FaultPoint::Before)?;
        let result = self.inner.create_pull_request(repo_id, input).await?;
        self.guard(ForgeOp::CreatePullRequest, n, FaultPoint::After)?;
        Ok(result)
    }

    async fn get_pull_request(&self, id: &PullRequestId) -> ForgeResult<Option<PullRequest>> {
        self.inner.get_pull_request(id).await
    }

    async fn get_pull_request_by_number(
        &self,
        repo_id: &RepositoryId,
        number: ItemNumber,
    ) -> ForgeResult<Option<PullRequest>> {
        let n = self.tick(ForgeOp::GetPullRequestByNumber);
        self.guard(ForgeOp::GetPullRequestByNumber, n, FaultPoint::Before)?;
        let result = self
            .inner
            .get_pull_request_by_number(repo_id, number)
            .await?;
        self.guard(ForgeOp::GetPullRequestByNumber, n, FaultPoint::After)?;
        Ok(result)
    }

    async fn update_pull_request(
        &self,
        id: &PullRequestId,
        input: UpdatePullRequest,
    ) -> ForgeResult<PullRequest> {
        let n = self.tick(ForgeOp::UpdatePullRequest);
        self.guard(ForgeOp::UpdatePullRequest, n, FaultPoint::Before)?;
        let result = self.inner.update_pull_request(id, input).await?;
        self.guard(ForgeOp::UpdatePullRequest, n, FaultPoint::After)?;
        Ok(result)
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
        let n = self.tick(ForgeOp::ListPullRequestReviews);
        self.guard(ForgeOp::ListPullRequestReviews, n, FaultPoint::Before)?;
        let result = self.inner.list_pull_request_reviews(id).await?;
        self.guard(ForgeOp::ListPullRequestReviews, n, FaultPoint::After)?;
        Ok(result)
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
        let n = self.tick(ForgeOp::MergePullRequest);
        self.guard(ForgeOp::MergePullRequest, n, FaultPoint::Before)?;
        self.merge_inputs
            .lock()
            .expect("merge inputs mutex")
            .push(input.clone());
        let result = self.inner.merge_pull_request(id, input).await?;
        self.guard(ForgeOp::MergePullRequest, n, FaultPoint::After)?;
        Ok(result)
    }

    async fn list_ci_jobs(
        &self,
        repo_id: &RepositoryId,
        query: CiJobQuery,
    ) -> ForgeResult<Vec<CiJob>> {
        let n = self.tick(ForgeOp::ListCiJobs);
        self.guard(ForgeOp::ListCiJobs, n, FaultPoint::Before)?;
        let result = self.inner.list_ci_jobs(repo_id, query).await?;
        self.guard(ForgeOp::ListCiJobs, n, FaultPoint::After)?;
        Ok(result)
    }

    async fn get_ci_job(&self, id: &CiJobId) -> ForgeResult<Option<CiJob>> {
        self.inner.get_ci_job(id).await
    }
}
