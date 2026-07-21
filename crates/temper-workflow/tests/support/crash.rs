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
    CreatePullRequestReview, CreateRepository, Forge, ForgeError, ForgeResult, Issue,
    IssueCandidateQuery, IssueId, IssueQuery, ItemListDetails, ItemNumber, ItemNumberNamespace,
    Label, MergePullRequest, MergeRecord, PullRequest, PullRequestCandidateQuery, PullRequestId,
    PullRequestQuery, PullRequestReview, Repository, RepositoryId, RepositoryPath, RepositoryQuery,
    RequestReviewers, UpdateIssue, UpdatePullRequest, UpsertLabel, User, UserId,
};

mod forge_impl;

/// The fault-aware Forge operations.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ForgeOp {
    CreateIssue,
    UpdateIssue,
    GetIssue,
    GetIssueByNumber,
    ListIssues,
    ListIssueCandidates,
    ListIssuesDefault,
    ListIssueComments,
    AddIssueComment,
    CreatePullRequest,
    UpdatePullRequest,
    GetPullRequest,
    GetPullRequestByNumber,
    ListPullRequests,
    ListPullRequestCandidates,
    ListPullRequestsDefault,
    ListPullRequestComments,
    AddPullRequestComment,
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
    issue_candidate_queries: Mutex<Vec<IssueCandidateQuery>>,
    issue_exact_details: Mutex<Vec<ItemListDetails>>,
    issue_updates: Mutex<Vec<UpdateIssue>>,
    pull_request_queries: Mutex<Vec<PullRequestQuery>>,
    pull_request_candidate_queries: Mutex<Vec<PullRequestCandidateQuery>>,
    pull_request_exact_details: Mutex<Vec<ItemListDetails>>,
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
            issue_candidate_queries: Mutex::new(Vec::new()),
            issue_exact_details: Mutex::new(Vec::new()),
            issue_updates: Mutex::new(Vec::new()),
            pull_request_queries: Mutex::new(Vec::new()),
            pull_request_candidate_queries: Mutex::new(Vec::new()),
            pull_request_exact_details: Mutex::new(Vec::new()),
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

    /// Returns the issue candidate queries this wrapper observed.
    pub fn issue_candidate_queries(&self) -> Vec<IssueCandidateQuery> {
        self.issue_candidate_queries
            .lock()
            .expect("issue candidate queries mutex")
            .clone()
    }

    /// Returns the exact-issue detail budgets this wrapper observed.
    pub fn issue_exact_details(&self) -> Vec<ItemListDetails> {
        self.issue_exact_details
            .lock()
            .expect("issue exact details mutex")
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

    /// Returns the pull-request candidate queries this wrapper observed.
    pub fn pull_request_candidate_queries(&self) -> Vec<PullRequestCandidateQuery> {
        self.pull_request_candidate_queries
            .lock()
            .expect("pull request candidate queries mutex")
            .clone()
    }

    /// Returns the exact pull-request detail budgets this wrapper observed.
    pub fn pull_request_exact_details(&self) -> Vec<ItemListDetails> {
        self.pull_request_exact_details
            .lock()
            .expect("pull request exact details mutex")
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
