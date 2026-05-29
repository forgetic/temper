//! Shared test support for the Phase 7 runtime tests.
//!
//! These helpers drive the deterministic `harness-fs` backend without an async
//! runtime: the filesystem forge never parks, so a hand-rolled `block_on` is
//! enough. Each test binary that needs a backend includes this module with
//! `mod support;`.
//!
//! The [`crash`] submodule adds a fault-injecting [`harness_forge::Forge`]
//! wrapper used by the Phase 8 robustness tests.
#![allow(dead_code)]

pub mod crash;

use chrono::{DateTime, Utc};
use harness_forge::{
    BranchRef, CreateIssue, CreatePullRequest, Forge, ItemNumber, RepositoryId, UserId,
};
use harness_fs::FilesystemForge;
use harness_workflow::{RawWorkflowSpec, ValidatedWorkflow};
use std::future::Future;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};

const FIXTURE: &str = include_str!("../../fixtures/five-role-delivery.json");

static NEXT_TEMP_ROOT: AtomicU64 = AtomicU64::new(0);

/// A temporary backend root cleaned up on drop.
pub struct TestRoot {
    path: PathBuf,
}

impl TestRoot {
    pub fn new() -> Self {
        let id = NEXT_TEMP_ROOT.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "harness-workflow-phase7-{}-{id}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        Self { path }
    }

    pub fn forge(&self) -> FilesystemForge {
        FilesystemForge::new(&self.path)
    }
}

impl Default for TestRoot {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for TestRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

struct NoopWake;

impl Wake for NoopWake {
    fn wake(self: Arc<Self>) {}
}

/// Drives a Forge future to completion; the filesystem backend never parks.
pub fn block_on<F: Future>(future: F) -> F::Output {
    let waker = Waker::from(Arc::new(NoopWake));
    let mut context = Context::from_waker(&waker);
    let mut future = Box::pin(future);
    match Future::poll(future.as_mut(), &mut context) {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("filesystem forge futures should not park in tests"),
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
pub fn new_repo(forge: &FilesystemForge) -> RepositoryId {
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
    forge: &FilesystemForge,
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
    forge: &FilesystemForge,
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
pub fn issue_body(forge: &FilesystemForge, repo: &RepositoryId, number: ItemNumber) -> String {
    block_on(forge.get_issue_by_number(repo, number))
        .expect("lookup succeeds")
        .expect("issue exists")
        .body
}

/// Reads an issue's sorted labels from the backend.
pub fn issue_labels(
    forge: &FilesystemForge,
    repo: &RepositoryId,
    number: ItemNumber,
) -> Vec<String> {
    let mut labels = block_on(forge.get_issue_by_number(repo, number))
        .expect("lookup succeeds")
        .expect("issue exists")
        .labels;
    labels.sort();
    labels
}

/// Reads a pull request's sorted labels from the backend.
pub fn pr_labels(forge: &FilesystemForge, repo: &RepositoryId, number: ItemNumber) -> Vec<String> {
    let mut labels = block_on(forge.get_pull_request_by_number(repo, number))
        .expect("lookup succeeds")
        .expect("pull request exists")
        .labels;
    labels.sort();
    labels
}
