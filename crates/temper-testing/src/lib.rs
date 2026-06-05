//! Reusable, non-production testing machinery shared across Temper crates.
//!
//! This crate is the home for the deterministic reference-delivery fakes that
//! were originally grown inside `temper-runner`'s integration-test support
//! module: behavior-only fake agents, the fake CI producer policies and sinks,
//! the backend-neutral `Scenario` seed/assert definitions, and the
//! `RunnerConfig`/repo/user helpers plus the bundled reference-delivery fixture
//! loader.
//!
//! It is **never** a normal (non-dev) dependency of a production crate; it is a
//! dev-dependency of `temper-runner` (and a dependency of other test crates).
//! Crate-specific helpers that only one crate uses stay local to that crate
//! (for example `CrashForge` in `temper-workflow` tests and the Forgejo mock
//! HTTP seam in `temper-forge-forgejo`).

pub mod agents;
pub mod ci;
pub mod forgejo_runtime;
pub mod forgejo_server;
pub mod provision_bin;
pub mod scenarios;
pub mod worker_bin;
pub mod world;

use chrono::{DateTime, Duration, Utc};
use std::future::Future;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use temper_forge::{
    BranchRef, CreateIssue, CreatePullRequest, CreateRepository, Forge, ItemNumber, RepositoryId,
    User, UserId,
};
use temper_forge_memory::MemoryForge;
use temper_runner::RunnerConfig;
use temper_workflow::{RawWorkflowSpec, RoleId};

const FIXTURE: &str = include_str!("../../temper-workflow/fixtures/reference-delivery.json");

struct NoopWake;

impl Wake for NoopWake {
    fn wake(self: Arc<Self>) {}
}

pub fn block_on<F: Future>(future: F) -> F::Output {
    let waker = Waker::from(Arc::new(NoopWake));
    let mut context = Context::from_waker(&waker);
    let mut future = Box::pin(future);
    match Future::poll(future.as_mut(), &mut context) {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("reference forge futures should not park in tests"),
    }
}

pub fn ts(value: &str) -> DateTime<Utc> {
    value.parse().expect("valid RFC 3339 timestamp")
}

pub fn workflow() -> temper_workflow::ValidatedWorkflow {
    let spec: RawWorkflowSpec = serde_json::from_str(FIXTURE).expect("fixture parses");
    spec.validate().expect("reference fixture validates")
}

pub fn repo_input() -> CreateRepository {
    CreateRepository {
        owner: "acme".into(),
        name: "service".into(),
        default_branch: "main".into(),
        description: None,
    }
}

pub fn new_repo(forge: &MemoryForge) -> RepositoryId {
    block_on(forge.create_repository(repo_input()))
        .expect("repository is created")
        .id
}

pub fn user(id: &str, handle: &str) -> User {
    User {
        id: UserId::new(id),
        handle: handle.into(),
        display_name: None,
        email: None,
    }
}

pub fn actor_user(role: &str) -> User {
    // The user id and handle are intentionally the **same** string. On real
    // Forgejo the role's single login must serve three things at once: the access
    // token's `current_user`, the assignee the workflow sends (`set_assignee`
    // binds `user.id`), and the web-UI CI-read login (which authenticates by
    // handle). Keeping id == handle lets one provisioned account satisfy all
    // three; the filesystem/memory backends are unaffected (identity there is a
    // free relabel).
    user(role, role)
}

pub fn runner_config() -> RunnerConfig {
    RunnerConfig::new(repo_input())
        .with_role_binding(RoleId::new("architect"), actor_user("architect"))
        .with_role_binding(RoleId::new("engineer"), actor_user("engineer"))
        .with_role_binding(RoleId::new("reviewer"), actor_user("reviewer"))
        .with_role_binding(RoleId::new("owner"), actor_user("owner"))
        .with_role_binding(RoleId::new("human"), actor_user("human"))
        .with_lease_ttl(Duration::minutes(30))
        .with_poll_interval(Duration::seconds(1))
}

pub fn create_issue(
    forge: &MemoryForge,
    repo: &RepositoryId,
    labels: &[&str],
    title: &str,
    body: &str,
) -> ItemNumber {
    block_on(forge.create_issue(
        repo,
        CreateIssue {
            title: title.into(),
            body: body.into(),
            labels: labels.iter().map(|label| (*label).to_string()).collect(),
            assignees: Vec::new(),
        },
    ))
    .expect("issue is created")
    .number
}

pub fn create_pr(
    forge: &MemoryForge,
    repo: &RepositoryId,
    labels: &[&str],
    title: &str,
    body: &str,
) -> ItemNumber {
    block_on(forge.create_pull_request(
        repo,
        CreatePullRequest {
            title: title.into(),
            body: body.into(),
            source: BranchRef {
                repository_id: repo.clone(),
                branch: "feature".into(),
            },
            target: BranchRef {
                repository_id: repo.clone(),
                branch: "main".into(),
            },
            labels: labels.iter().map(|label| (*label).to_string()).collect(),
            assignees: Vec::<UserId>::new(),
        },
    ))
    .expect("pull request is created")
    .number
}

pub fn pull_request_input(
    repo: &RepositoryId,
    title: impl Into<String>,
    body: impl Into<String>,
    source_branch: impl Into<String>,
    labels: Vec<String>,
) -> CreatePullRequest {
    CreatePullRequest {
        title: title.into(),
        body: body.into(),
        source: BranchRef {
            repository_id: repo.clone(),
            branch: source_branch.into(),
        },
        target: BranchRef {
            repository_id: repo.clone(),
            branch: "main".into(),
        },
        labels,
        assignees: Vec::<UserId>::new(),
    }
}

pub fn labels(mut labels: Vec<String>) -> Vec<String> {
    labels.sort();
    labels
}
