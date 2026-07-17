//! Reusable, non-production testing machinery shared across Temper crates.
//!
//! This crate is the home for the deterministic reference-delivery fakes that
//! were originally grown inside `temper-runner`'s integration-test support
//! module: behavior-only fake agents, the fake CI producer policies and sinks,
//! the backend-neutral `Scenario` seed/assert definitions, and the
//! `RunnerConfig`/repo/user helpers plus the bundled reference-delivery fixture
//! loader.
//!
//! It is kept out of the default production dependency graph; it is a
//! dev-dependency of `temper-runner`, a dependency of other test crates, and an
//! optional root-package dependency only behind test-only features.
//! Crate-specific helpers that only one crate uses stay local to that crate
//! (for example `CrashForge` in `temper-workflow` tests and the Forgejo mock
//! HTTP seam in `temper-forge-forgejo`).

pub mod agents;
pub mod ci;
pub mod counting_forge;
pub mod counting_http;
pub mod daemon_worker;
#[cfg(target_os = "linux")]
pub mod descendant_fixture;
pub mod forgejo_runtime;
pub mod forgejo_server;
pub mod live_basic_delivery;
pub mod provision_bin;
pub mod real_stack;
pub mod scenarios;
pub mod worker_bin;
pub mod world;

use chrono::{DateTime, Utc};
use std::future::Future;
use std::path::Path;
use std::sync::{Arc, Condvar, Mutex};
use std::task::{Context, Poll, Wake, Waker};
use std::time::Duration;
use temper_forge_memory::MemoryForge;
use temper_forge_model::{
    BranchRef, CreateIssue, CreatePullRequest, CreateRepository, Forge, ItemNumber, RepositoryId,
    User, UserId,
};
use temper_runner::RunnerConfig;
use temper_workflow::ValidatedWorkflow;

// The runtime workflow loader and the workflow-derived runner config are shared
// with the deployable reference-delivery binaries; reuse them rather than
// re-bundling the fixture or re-deriving the role bindings here. The
// re-export keeps the historic `temper-testing` symbols (`WorkflowLoadError`)
// importable from this crate.
pub use temper_reference_delivery::WorkflowLoadError;

const BLOCK_ON_WAKE_TIMEOUT: Duration = Duration::from_secs(30);

struct BlockingWake {
    notified: Mutex<bool>,
    condvar: Condvar,
}

impl BlockingWake {
    fn new() -> Self {
        Self {
            notified: Mutex::new(false),
            condvar: Condvar::new(),
        }
    }

    fn wait(&self) {
        let mut notified = self
            .notified
            .lock()
            .expect("test future wake mutex poisoned");
        while !*notified {
            let (guard, timeout) = self
                .condvar
                .wait_timeout(notified, BLOCK_ON_WAKE_TIMEOUT)
                .expect("test future wake mutex poisoned");
            notified = guard;
            if timeout.timed_out() && !*notified {
                panic!("test future parked without waking");
            }
        }
        *notified = false;
    }

    fn notify(&self) {
        let mut notified = self
            .notified
            .lock()
            .expect("test future wake mutex poisoned");
        *notified = true;
        self.condvar.notify_one();
    }
}

impl Wake for BlockingWake {
    fn wake(self: Arc<Self>) {
        self.notify();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.notify();
    }
}

pub fn block_on<F: Future>(future: F) -> F::Output {
    let wake = Arc::new(BlockingWake::new());
    let waker = Waker::from(Arc::clone(&wake));
    let mut context = Context::from_waker(&waker);
    let mut future = Box::pin(future);
    loop {
        match Future::poll(future.as_mut(), &mut context) {
            Poll::Ready(value) => return value,
            Poll::Pending => wake.wait(),
        }
    }
}

pub fn ts(value: &str) -> DateTime<Utc> {
    value.parse().expect("valid RFC 3339 timestamp")
}

/// The bundled reference-delivery workflow (the worker's default when no
/// `--workflow` is given). Delegates to the shared reference-delivery loader so
/// the testing fakes and the deployable binaries agree on the default document.
pub fn workflow() -> ValidatedWorkflow {
    temper_reference_delivery::workflow()
}

/// The bundled **basic-delivery** workflow — the minimal, no-human-in-the-loop
/// shape exercised by the basic-delivery fakes ([`agents::BasicArchitect`] /
/// [`agents::BasicEngineer`]).
///
/// Delegates to the shared reference-delivery loader so the testing fakes, the
/// fixture-shape tests, and the deployable binaries all agree on one document.
pub fn basic_delivery_workflow() -> ValidatedWorkflow {
    temper_reference_delivery::basic_delivery_workflow()
}

/// Runner config derived from [`basic_delivery_workflow`].
///
/// basic-delivery binds only the queue-subscribing roles `architect`/`engineer`;
/// `mechanical` is queue-less and stays unbound (see [`runner_config_for_workflow`]).
pub fn basic_delivery_runner_config() -> RunnerConfig {
    runner_config_for_workflow(&basic_delivery_workflow())
}

/// Resolves the workflow the worker operates against: the file at `path` when
/// supplied (the runtime `--workflow` selection), otherwise the bundled
/// reference-delivery default. Errors are reported against `path` so an operator
/// can see which file failed and why (read, parse, or validation), without
/// leaking any secret.
pub fn resolve_workflow(
    path: Option<impl AsRef<Path>>,
) -> Result<ValidatedWorkflow, WorkflowLoadError> {
    temper_reference_delivery::resolve_workflow(path)
}

pub fn repo_input() -> CreateRepository {
    temper_reference_delivery::repo_input()
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

/// Runner config for the bundled reference-delivery default.
///
/// Equivalent to `runner_config_for_workflow(&workflow())`; it derives role→user
/// bindings from the workflow's queue-subscribing roles, so the default binds
/// exactly architect/engineer/reviewer/owner/human (mechanical is queue-less and
/// has no role worker).
pub fn runner_config() -> RunnerConfig {
    runner_config_for_workflow(&workflow())
}

/// Derives a runner config from any validated workflow, against [`repo_input`].
///
/// Role bindings come from the workflow's queue-subscribing roles (Forge user id
/// == role id, the demo provisioning convention), so a runtime-selected workflow
/// binds its own roles. A basic-delivery spec, for example, binds only
/// `architect`/`engineer`; `mechanical` stays queue-less and unbound.
pub fn runner_config_for_workflow(workflow: &ValidatedWorkflow) -> RunnerConfig {
    temper_reference_delivery::runner_config_for(workflow, repo_input())
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
