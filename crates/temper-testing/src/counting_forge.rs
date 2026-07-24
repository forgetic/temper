use async_trait::async_trait;
use std::collections::HashMap;
use std::future::Future;
use std::sync::Mutex;
use temper_forge_model::{
    CiJob, CiJobId, CiJobQuery, CiRetryOutcome, CiRetryRequest, Comment, CreateComment,
    CreateIssue, CreatePullRequest, CreatePullRequestReview, CreateRepository, Forge, ForgeError,
    ForgeResult, Issue, IssueCandidateQuery, IssueId, IssueQuery, ItemListDetails, ItemNumber,
    ItemNumberNamespace, Label, MergePullRequest, MergeRecord, PullRequest,
    PullRequestCandidateQuery, PullRequestId, PullRequestQuery, PullRequestReview, Repository,
    RepositoryId, RepositoryPath, RepositoryQuery, RequestReviewers, UpdateIssue,
    UpdatePullRequest, UpsertLabel, User, UserId,
};

use operation_log::ForgeOperationLog;
pub use operation_log::ForgeOperationPause;

mod forge_impl;
mod operation_log;

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
    ListIssueCandidates,
    CreateIssue,
    GetIssue,
    GetIssueByNumber,
    UpdateIssue,
    AddIssueDependency,
    RemoveIssueDependency,
    ListIssueComments,
    AddIssueComment,
    ListPullRequests,
    ListPullRequestCandidates,
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
    RetryCiAttempt,
    ListCiJobs,
    GetCiJob,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExactIssueRead {
    pub by_number: bool,
    pub details: ItemListDetails,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExactPullRequestRead {
    pub by_number: bool,
    pub details: ItemListDetails,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ForgeReadShape {
    /// Consolidated issue/PR candidate-list calls.
    pub candidate_list_calls: usize,
    /// Exact artifact reads that requested summary detail only.
    pub exact_summary_reads: usize,
    /// Exact artifact reads that requested dependency-enriched full detail.
    pub exact_full_reads: usize,
}

pub struct CountingForge<F: Forge> {
    inner: F,
    item_number_namespace: Option<ItemNumberNamespace>,
    operations: ForgeOperationLog,
    merge_conflicts: Mutex<HashMap<PullRequestId, String>>,
    synthetic_heads: Mutex<bool>,
    head_overrides: Mutex<HashMap<PullRequestId, String>>,
    head_generations: Mutex<HashMap<PullRequestId, u64>>,
    advance_heads_on_conflict_resolution: Mutex<bool>,
    issue_queries: Mutex<Vec<IssueQuery>>,
    issue_candidate_queries: Mutex<Vec<IssueCandidateQuery>>,
    issue_candidate_overrides: Mutex<HashMap<IssueId, Issue>>,
    pull_request_queries: Mutex<Vec<PullRequestQuery>>,
    pull_request_candidate_queries: Mutex<Vec<PullRequestCandidateQuery>>,
    ci_job_queries: Mutex<Vec<CiJobQuery>>,
    ci_retry_requests: Mutex<Vec<CiRetryRequest>>,
    exact_issue_reads: Mutex<Vec<ExactIssueRead>>,
    exact_pull_request_reads: Mutex<Vec<ExactPullRequestRead>>,
}

impl<F: Forge> CountingForge<F> {
    pub fn new(inner: F) -> Self {
        Self {
            inner,
            item_number_namespace: None,
            operations: ForgeOperationLog::default(),
            merge_conflicts: Mutex::new(HashMap::new()),
            synthetic_heads: Mutex::new(false),
            head_overrides: Mutex::new(HashMap::new()),
            head_generations: Mutex::new(HashMap::new()),
            advance_heads_on_conflict_resolution: Mutex::new(false),
            issue_queries: Mutex::new(Vec::new()),
            issue_candidate_queries: Mutex::new(Vec::new()),
            issue_candidate_overrides: Mutex::new(HashMap::new()),
            pull_request_queries: Mutex::new(Vec::new()),
            pull_request_candidate_queries: Mutex::new(Vec::new()),
            ci_job_queries: Mutex::new(Vec::new()),
            ci_retry_requests: Mutex::new(Vec::new()),
            exact_issue_reads: Mutex::new(Vec::new()),
            exact_pull_request_reads: Mutex::new(Vec::new()),
        }
    }

    /// Wraps a fixture while modelling a specific provider number namespace.
    ///
    /// Most tests should use [`Self::new`] and inherit the wrapped backend's
    /// capability. This constructor lets production-shaped tests run against a
    /// local fixture whose storage counters differ from the modelled provider.
    pub fn with_item_number_namespace(inner: F, namespace: ItemNumberNamespace) -> Self {
        Self {
            item_number_namespace: Some(namespace),
            ..Self::new(inner)
        }
    }

    /// Direct access to the wrapped fixture for deterministic state changes
    /// while a selected read is paused.
    pub fn inner(&self) -> &F {
        &self.inner
    }

    pub fn count(&self, op: CountedForgeOp) -> usize {
        self.operations.count(op)
    }

    pub fn operation_trace(&self) -> Vec<CountedForgeOp> {
        self.operations.trace()
    }

    /// Pauses once after `occurrence` of `op` has captured its inner result.
    /// Occurrences are one-indexed and include calls already in the trace.
    pub fn pause_after(&self, op: CountedForgeOp, occurrence: usize) -> ForgeOperationPause {
        self.operations.pause_after(op, occurrence)
    }

    pub fn read_count(&self) -> usize {
        self.operations.read_count()
    }

    pub fn write_count(&self) -> usize {
        self.operations.write_count()
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

    /// Overrides one issue only in candidate-list results. Exact reads still
    /// reach the wrapped forge, allowing tests to model providers whose list
    /// summaries lag dependency-link changes.
    pub fn override_issue_candidate_summary(&self, issue: Issue) {
        self.issue_candidate_overrides
            .lock()
            .expect("issue candidate overrides mutex")
            .insert(issue.id.clone(), issue);
    }

    pub fn issue_candidate_queries(&self) -> Vec<IssueCandidateQuery> {
        self.issue_candidate_queries
            .lock()
            .expect("issue candidate query mutex")
            .clone()
    }

    #[allow(dead_code)]
    pub fn pull_request_queries(&self) -> Vec<PullRequestQuery> {
        self.pull_request_queries
            .lock()
            .expect("pull request query mutex")
            .clone()
    }

    pub fn pull_request_candidate_queries(&self) -> Vec<PullRequestCandidateQuery> {
        self.pull_request_candidate_queries
            .lock()
            .expect("pull request candidate query mutex")
            .clone()
    }

    pub fn ci_job_queries(&self) -> Vec<CiJobQuery> {
        self.ci_job_queries
            .lock()
            .expect("CI job query mutex")
            .clone()
    }

    pub fn ci_retry_requests(&self) -> Vec<CiRetryRequest> {
        self.ci_retry_requests
            .lock()
            .expect("CI retry requests mutex")
            .clone()
    }

    pub fn exact_issue_reads(&self) -> Vec<ExactIssueRead> {
        self.exact_issue_reads
            .lock()
            .expect("exact issue reads mutex")
            .clone()
    }

    pub fn exact_pull_request_reads(&self) -> Vec<ExactPullRequestRead> {
        self.exact_pull_request_reads
            .lock()
            .expect("exact pull request reads mutex")
            .clone()
    }

    /// Returns the read shape recorded so far. Writes are deliberately absent:
    /// this helper is for request-budget assertions, not total operation volume.
    pub fn read_shape(&self) -> ForgeReadShape {
        let exact_issue_reads = self.exact_issue_reads();
        let exact_pull_request_reads = self.exact_pull_request_reads();
        let exact_summary_reads = exact_issue_reads
            .iter()
            .filter(|read| !read.details.dependencies)
            .count()
            .saturating_add(
                exact_pull_request_reads
                    .iter()
                    .filter(|read| !read.details.dependencies)
                    .count(),
            );
        let exact_full_reads = exact_issue_reads
            .iter()
            .filter(|read| read.details.dependencies)
            .count()
            .saturating_add(
                exact_pull_request_reads
                    .iter()
                    .filter(|read| read.details.dependencies)
                    .count(),
            );
        ForgeReadShape {
            candidate_list_calls: self
                .count(CountedForgeOp::ListIssueCandidates)
                .saturating_add(self.count(CountedForgeOp::ListPullRequestCandidates)),
            exact_summary_reads,
            exact_full_reads,
        }
    }

    fn record_exact_issue_read(&self, by_number: bool, details: ItemListDetails) {
        self.exact_issue_reads
            .lock()
            .expect("exact issue reads mutex")
            .push(ExactIssueRead { by_number, details });
    }

    fn record_exact_pull_request_read(&self, by_number: bool, details: ItemListDetails) {
        self.exact_pull_request_reads
            .lock()
            .expect("exact pull request reads mutex")
            .push(ExactPullRequestRead { by_number, details });
    }

    async fn perform<T>(&self, op: CountedForgeOp, operation: impl Future<Output = T>) -> T {
        let occurrence = self.operations.tick(op);
        let result = operation.await;
        self.operations.pause_after_result(op, occurrence).await;
        result
    }

    fn record_issue_query(&self, query: &IssueQuery) {
        self.issue_queries
            .lock()
            .expect("issue query mutex")
            .push(query.clone());
    }

    fn record_issue_candidate_query(&self, query: &IssueCandidateQuery) {
        self.issue_candidate_queries
            .lock()
            .expect("issue candidate query mutex")
            .push(query.clone());
    }

    fn record_pull_request_query(&self, query: &PullRequestQuery) {
        self.pull_request_queries
            .lock()
            .expect("pull request query mutex")
            .push(query.clone());
    }

    fn record_pull_request_candidate_query(&self, query: &PullRequestCandidateQuery) {
        self.pull_request_candidate_queries
            .lock()
            .expect("pull request candidate query mutex")
            .push(query.clone());
    }

    fn record_ci_job_query(&self, query: &CiJobQuery) {
        self.ci_job_queries
            .lock()
            .expect("CI job query mutex")
            .push(query.clone());
    }

    fn record_ci_retry_request(&self, request: &CiRetryRequest) {
        self.ci_retry_requests
            .lock()
            .expect("CI retry requests mutex")
            .push(request.clone());
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
