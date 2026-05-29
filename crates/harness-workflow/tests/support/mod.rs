//! Shared test support for the Phase 7 runtime tests.
//!
//! These helpers drive the deterministic in-memory `harness-forge-memory`
//! backend without an async runtime: the in-memory forge never parks, so a
//! hand-rolled `block_on` is enough. Each test binary that needs a backend
//! includes this module with `mod support;`.
//!
//! The [`crash`] submodule adds a fault-injecting [`harness_forge::Forge`]
//! wrapper used by the Phase 8 robustness tests.
#![allow(dead_code)]

pub mod crash;

use chrono::{DateTime, Utc};
use harness_forge::{
    BranchRef, CreateIssue, CreatePullRequest, Forge, ItemNumber, RepositoryId, UserId,
};
use harness_forge_memory::MemoryForge;
use harness_workflow::{RawWorkflowSpec, ValidatedWorkflow};
use std::future::Future;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};

const FIXTURE: &str = include_str!("../../fixtures/five-role-delivery.json");

/// Owns one in-memory backend store for a test.
///
/// Kept as a thin wrapper so existing tests can continue to write
/// `let root = TestRoot::new(); let forge = root.forge();`. Each `forge()`
/// returns a clone that shares the same underlying store.
pub struct TestRoot {
    forge: MemoryForge,
}

impl TestRoot {
    pub fn new() -> Self {
        Self {
            forge: MemoryForge::new(),
        }
    }

    pub fn forge(&self) -> MemoryForge {
        self.forge.clone()
    }
}

impl Default for TestRoot {
    fn default() -> Self {
        Self::new()
    }
}

struct NoopWake;

impl Wake for NoopWake {
    fn wake(self: Arc<Self>) {}
}

/// Drives a Forge future to completion; the in-memory backend never parks.
pub fn block_on<F: Future>(future: F) -> F::Output {
    let waker = Waker::from(Arc::new(NoopWake));
    let mut context = Context::from_waker(&waker);
    let mut future = Box::pin(future);
    match Future::poll(future.as_mut(), &mut context) {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("in-memory forge futures should not park in tests"),
    }
}

/// Loads and validates the checked-in five-role fixture.
pub fn workflow() -> ValidatedWorkflow {
    let spec: RawWorkflowSpec =
        serde_json::from_str(FIXTURE).expect("fixture is valid RawWorkflowSpec JSON");
    spec.validate().expect("five-role fixture validates")
}

/// Parses an RFC 3339 timestamp for deterministic time control.
pub fn ts(value: &str) -> DateTime<Utc> {
    value.parse().expect("valid RFC 3339 timestamp")
}

/// Creates a fresh repository in the backend.
pub fn new_repo(forge: &MemoryForge) -> RepositoryId {
    block_on(forge.create_repository(harness_forge::CreateRepository {
        owner: "acme".into(),
        name: "service".into(),
        default_branch: "main".into(),
        description: None,
    }))
    .expect("repository is created")
    .id
}

/// Creates an issue with the given labels and body.
pub fn create_issue(
    forge: &MemoryForge,
    repo: &RepositoryId,
    labels: &[&str],
    body: &str,
) -> ItemNumber {
    block_on(forge.create_issue(
        repo,
        CreateIssue {
            title: "code work".into(),
            body: body.into(),
            labels: labels.iter().map(|l| (*l).to_string()).collect(),
            assignees: Vec::new(),
        },
    ))
    .expect("issue is created")
    .number
}

/// Creates a pull request with the given labels and body.
pub fn create_pr(
    forge: &MemoryForge,
    repo: &RepositoryId,
    labels: &[&str],
    body: &str,
) -> ItemNumber {
    block_on(forge.create_pull_request(
        repo,
        CreatePullRequest {
            title: "implementation".into(),
            body: body.into(),
            source: BranchRef {
                repository_id: repo.clone(),
                branch: "feature".into(),
            },
            target: BranchRef {
                repository_id: repo.clone(),
                branch: "main".into(),
            },
            labels: labels.iter().map(|l| (*l).to_string()).collect(),
            assignees: Vec::<UserId>::new(),
        },
    ))
    .expect("pull request is created")
    .number
}

/// Reads an issue's current body from the backend.
pub fn issue_body(forge: &MemoryForge, repo: &RepositoryId, number: ItemNumber) -> String {
    block_on(forge.get_issue_by_number(repo, number))
        .expect("lookup succeeds")
        .expect("issue exists")
        .body
}

/// Reads an issue's sorted labels from the backend.
pub fn issue_labels(forge: &MemoryForge, repo: &RepositoryId, number: ItemNumber) -> Vec<String> {
    let mut labels = block_on(forge.get_issue_by_number(repo, number))
        .expect("lookup succeeds")
        .expect("issue exists")
        .labels;
    labels.sort();
    labels
}

/// Reads a pull request's sorted labels from the backend.
pub fn pr_labels(forge: &MemoryForge, repo: &RepositoryId, number: ItemNumber) -> Vec<String> {
    let mut labels = block_on(forge.get_pull_request_by_number(repo, number))
        .expect("lookup succeeds")
        .expect("pull request exists")
        .labels;
    labels.sort();
    labels
}
