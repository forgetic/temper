#![allow(dead_code)]

use chrono::{DateTime, Utc};
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll, Wake, Waker};
use temper_forge::{
    BranchRef, CiJob, CreateComment, CreateIssue, CreatePullRequest, CreateRepository,
    RepositoryId, UserId,
};
use temper_forge_filesystem::FilesystemForge;

static NEXT_TEMP_ROOT: AtomicU64 = AtomicU64::new(0);

pub struct TestRoot {
    path: PathBuf,
}

impl TestRoot {
    pub fn new(suite: &str) -> Self {
        let id = NEXT_TEMP_ROOT.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "temper-forge-filesystem-{suite}-test-{}-{id}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        Self { path }
    }

    pub fn forge(&self) -> FilesystemForge {
        FilesystemForge::new(&self.path)
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

pub fn block_on<F>(future: F) -> F::Output
where
    F: Future,
{
    let waker = Waker::from(Arc::new(NoopWake));
    let mut context = Context::from_waker(&waker);
    let mut future = Box::pin(future);

    match Future::poll(future.as_mut(), &mut context) {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("filesystem forge futures should not park in tests"),
    }
}

pub fn repository(owner: &str, name: &str) -> CreateRepository {
    CreateRepository {
        owner: owner.into(),
        name: name.into(),
        default_branch: "main".into(),
        description: None,
    }
}

pub fn issue(title: &str) -> CreateIssue {
    issue_with(title, "", &[], &[])
}

pub fn issue_with(title: &str, body: &str, labels: &[&str], assignees: &[&str]) -> CreateIssue {
    CreateIssue {
        title: title.into(),
        body: body.into(),
        labels: labels.iter().map(|label| (*label).into()).collect(),
        assignees: assignees.iter().map(|id| UserId::new(*id)).collect(),
    }
}

pub fn branch(repo_id: &RepositoryId, name: &str) -> BranchRef {
    BranchRef {
        repository_id: repo_id.clone(),
        branch: name.into(),
    }
}

pub fn pull_request(repo_id: &RepositoryId, title: &str) -> CreatePullRequest {
    pull_request_with(repo_id, title, "", &[], &[])
}

pub fn pull_request_with(
    repo_id: &RepositoryId,
    title: &str,
    body: &str,
    labels: &[&str],
    assignees: &[&str],
) -> CreatePullRequest {
    CreatePullRequest {
        title: title.into(),
        body: body.into(),
        source: branch(repo_id, "feature"),
        target: branch(repo_id, "main"),
        labels: labels.iter().map(|label| (*label).into()).collect(),
        assignees: assignees.iter().map(|id| UserId::new(*id)).collect(),
    }
}

pub fn comment(body: &str) -> CreateComment {
    CreateComment { body: body.into() }
}

pub fn timestamp(seconds: i64) -> DateTime<Utc> {
    DateTime::<Utc>::from_timestamp(seconds, 0).expect("valid test timestamp")
}

pub fn write_ci_jobs(forge: &FilesystemForge, repo_id: &RepositoryId, ci_jobs: &[CiJob]) {
    let path = forge
        .root()
        .join("repositories")
        .join(repo_id.as_str())
        .join("ci_jobs.json");
    std::fs::create_dir_all(path.parent().expect("CI jobs path has parent"))
        .expect("create CI jobs fixture directory");

    let mut content = serde_json::to_string_pretty(ci_jobs).expect("serialize CI jobs fixture");
    content.push('\n');
    std::fs::write(path, content).expect("write CI jobs fixture");
}

pub fn user_ids(ids: &[&str]) -> Vec<UserId> {
    ids.iter().map(|id| UserId::new(*id)).collect()
}
