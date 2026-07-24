//! In-memory Forge backend for Temper development and tests.
//!
//! [`MemoryForge`] implements [`temper_forge_model::Forge`] entirely in process: all
//! records live in ordinary collections behind a single mutex, with no
//! filesystem, network, or async runtime involved. It is a sibling reference
//! backend to `temper-forge-filesystem` and intentionally reproduces the same
//! deterministic identifier scheme, logical clock, ordering, and query
//! semantics, including native dependency links and pull-request reviews (see
//! ADR 0008), so workflow tests can swap between the two without changing
//! expectations.
//!
//! Because there is no durable store to corrupt, a small one-shot
//! [`fault hooks`](MemoryForge::fail_next) let tests force a chosen operation to
//! return a backend error or optimistic-concurrency conflict so failure paths
//! stay exercisable. CI jobs have no create operation in the Forge
//! interface, so [`MemoryForge::seed_ci_jobs`] seeds them directly, mirroring how
//! the filesystem backend seeds its `ci_jobs.json` fixture. Tests that need the
//! earlier run-registered/job-unassigned state use [`MemoryForge::seed_ci_run`].
//! [`MemoryForge::as_user`]
//! creates another handle over the same store with a different current-user
//! identity, matching the per-process identity seam used by runner tests.

mod dependencies;
mod fault;
mod hint;
mod ids;
mod lists;
mod operations;
mod reviews;
mod state;
mod util;

use crate::fault::FaultStore;
use crate::hint::HintBus;
use crate::state::State;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};
use temper_forge_model::{
    CiJob, CiRetryOutcome, CiRetryRequest, ForgeError, ForgeResult, PullRequest, PullRequestId,
    RepoPermission, RepositoryId, User, WebhookSpec,
};

pub use crate::fault::FaultOp;
pub use crate::hint::MemoryHintReceiver;
pub use crate::state::MemUser;

/// The mutex-guarded interior: record store plus armed faults.
pub(crate) struct Inner {
    pub(crate) state: State,
    pub(crate) faults: FaultStore,
    pub(crate) hints: HintBus,
    pub(crate) ci_retry_outcome: CiRetryOutcome,
    pub(crate) ci_retry_requests: Vec<CiRetryRequest>,
    pub(crate) accepted_ci_retries: Vec<CiRetryRequest>,
}

/// In-memory [`Forge`](temper_forge_model::Forge) backend.
///
/// Cloning a `MemoryForge` shares the same underlying store, so a clone observes
/// and mutates the same records — useful for handing a backend to several
/// helpers in a test while keeping one logical store.
#[derive(Clone)]
pub struct MemoryForge {
    inner: Arc<Mutex<Inner>>,
    current_user: Option<User>,
}

impl MemoryForge {
    /// Creates an empty backend with the default bootstrapped current user.
    pub fn new() -> Self {
        Self::with_current_user(State::default_user())
    }

    /// Creates an empty backend with an explicit current user.
    pub fn with_current_user(user: User) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                state: State::new(user),
                faults: FaultStore::default(),
                hints: HintBus::default(),
                ci_retry_outcome: CiRetryOutcome::Unsupported,
                ci_retry_requests: Vec::new(),
                accepted_ci_retries: Vec::new(),
            })),
            current_user: None,
        }
    }

    /// Returns a handle over the same store that acts as `user`.
    ///
    /// The identity override lives on the handle, outside the shared store, so
    /// clones of the returned handle preserve the override while other handles
    /// can act as different users. This is a memory-only testing hook for the
    /// runner identity seam; it does not create or persist users.
    pub fn as_user(&self, user: User) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            current_user: Some(user),
        }
    }

    /// Arms a one-shot backend fault for the next call to `op`.
    ///
    /// The next invocation of `op` returns
    /// [`ForgeError::Backend`](temper_forge_model::ForgeError::Backend) before
    /// touching state; CI retry instead maps the injected delivery fault to the
    /// portable [`CiRetryOutcome::Uncertain`]. Later calls proceed normally.
    /// Arming the same op again queues another fault.
    pub fn fail_next(&self, op: FaultOp, message: impl Into<String>) {
        self.lock().faults.arm(op, message.into());
    }

    /// Arms a one-shot optimistic-concurrency conflict for the next call.
    ///
    /// This test seam models a provider rejecting a stale conditional write;
    /// no in-memory state is changed by the rejected operation.
    pub fn conflict_next(&self, op: FaultOp, message: impl Into<String>) {
        self.lock().faults.arm_conflict(op, message.into());
    }

    /// Clears every armed fault.
    pub fn clear_faults(&self) {
        self.lock().faults.clear();
    }

    /// Subscribes to in-process change hints from this shared store.
    ///
    /// This is an optional memory-backend companion surface, not part of the
    /// Forge trait. Every successful mutation publishes a best-effort hint to
    /// current subscribers; callers must still re-read Forge state after waking.
    pub fn subscribe_hints(&self) -> MemoryHintReceiver {
        self.lock().subscribe_hints()
    }

    /// Seeds CI jobs for a repository, replacing any previously seeded jobs.
    ///
    /// The Forge interface has no CI-job creation operation, so tests inject
    /// deterministic fixture jobs through this hook, matching the filesystem
    /// backend's `ci_jobs.json` seeding.
    pub fn seed_ci_jobs(&self, repo_id: &RepositoryId, jobs: Vec<CiJob>) {
        let mut inner = self.lock();
        inner.state.set_ci_jobs(repo_id, jobs);
        inner.publish_repo_hint(repo_id, temper_forge_model::ChangeKind::Ci);
    }

    /// Selects the deterministic outcome returned by exact-attempt CI retry.
    /// Accepted requests are remembered so an exact duplicate returns
    /// [`CiRetryOutcome::AlreadyObserved`].
    pub fn set_ci_retry_outcome(&self, outcome: CiRetryOutcome) {
        self.lock().ci_retry_outcome = outcome;
    }

    /// Returns exact-attempt retry requests in call order.
    pub fn ci_retry_requests(&self) -> Vec<CiRetryRequest> {
        self.lock().ci_retry_requests.clone()
    }

    /// Seeds provider evidence for a CI run before any job is assigned.
    ///
    /// This models hosted CI systems that register a PR/head run immediately but
    /// materialize jobs only when runner capacity becomes available.
    pub fn seed_ci_run(
        &self,
        repo_id: &RepositoryId,
        pull_request_id: Option<&PullRequestId>,
        commit_sha: &str,
    ) {
        let mut inner = self.lock();
        inner.state.add_ci_run(repo_id, pull_request_id, commit_sha);
        inner.publish_repo_hint(repo_id, temper_forge_model::ChangeKind::Ci);
    }

    /// Sets a pull request's current head SHA in the memory backend.
    ///
    /// Real hosted backends learn this from the provider's branch/PR metadata;
    /// the portable Forge trait intentionally has no operation for arbitrary PR
    /// head mutation. Tests use this companion hook to simulate a PR branch being
    /// advanced by a push before exercising freshness predicates.
    pub fn set_pull_request_head(
        &self,
        id: &PullRequestId,
        head_sha: Option<String>,
    ) -> ForgeResult<PullRequest> {
        let mut inner = self.lock();
        let (repo_id, _) = inner
            .state
            .find_pull_request(id)
            .ok_or_else(|| ForgeError::NotFound(format!("pull request {id}")))?;
        let now = inner.state.next_timestamp()?;
        let pull_requests = inner.state.pull_requests_mut(&repo_id);
        let pull_request = pull_requests
            .iter_mut()
            .find(|pull_request| &pull_request.id == id)
            .ok_or_else(|| ForgeError::NotFound(format!("pull request {id}")))?;
        pull_request.head_sha = head_sha;
        pull_request.version = pull_request.version.next();
        pull_request.updated_at = now;
        let updated = pull_request.clone();
        inner.publish_pull_request_hint(&updated, temper_forge_model::ChangeKind::Edited);
        Ok(updated)
    }

    /// Returns every user provisioned via
    /// [`ForgeAdmin::ensure_user`](temper_forge_model::ForgeAdmin::ensure_user),
    /// ordered by login.
    ///
    /// Test-only read-back companion surface, not part of any Forge trait,
    /// mirroring the filesystem backend's accessor of the same name.
    pub fn provisioned_users(&self) -> Vec<MemUser> {
        self.lock().state.provisioned_users()
    }

    /// Returns the webhooks registered on `repo` via
    /// [`ForgeAdmin::ensure_webhook`](temper_forge_model::ForgeAdmin::ensure_webhook).
    ///
    /// Test-only read-back companion surface, not part of any Forge trait.
    pub fn webhooks(&self, repo: &RepositoryId) -> Vec<WebhookSpec> {
        self.lock().state.webhooks(repo)
    }

    /// Returns the contents committed at `(repo, branch, path)` via
    /// [`ForgeContent::commit_file`](temper_forge_model::ForgeContent::commit_file),
    /// or `None` if no such file was committed.
    ///
    /// Test-only read-back companion surface, not part of any Forge trait.
    pub fn committed_file(&self, repo: &RepositoryId, branch: &str, path: &str) -> Option<Vec<u8>> {
        self.lock().state.committed_file(repo, branch, path)
    }

    /// Returns the repo-scoped collaborator grants recorded on `repo` via
    /// [`ForgeAdmin::grant_access`](temper_forge_model::ForgeAdmin::grant_access) with
    /// [`AccessScope::RepoCollaborator`](temper_forge_model::AccessScope::RepoCollaborator),
    /// keyed by login.
    ///
    /// Test-only read-back companion surface, not part of any Forge trait.
    pub fn grants(&self, repo: &RepositoryId) -> BTreeMap<String, RepoPermission> {
        self.lock().state.grants(repo)
    }

    /// Returns the tokens minted for `login` via
    /// [`ForgeAdmin::mint_token`](temper_forge_model::ForgeAdmin::mint_token), in mint
    /// order.
    ///
    /// Test-only read-back companion surface, not part of any Forge trait.
    pub fn minted_tokens(&self, login: &str) -> Vec<String> {
        self.lock().state.minted_tokens(login)
    }

    /// Reports whether CI was enabled on `repo` via
    /// [`ForgeAdmin::enable_ci`](temper_forge_model::ForgeAdmin::enable_ci).
    ///
    /// Test-only read-back companion surface, not part of any Forge trait.
    pub fn ci_enabled(&self, repo: &RepositoryId) -> bool {
        self.lock().state.ci_enabled(repo)
    }

    /// Reports whether `branch` was recorded on `repo` via
    /// [`ForgeContent::create_branch`](temper_forge_model::ForgeContent::create_branch)
    /// or as the target of a [`commit_file`](temper_forge_model::ForgeContent::commit_file).
    ///
    /// Test-only read-back companion surface, not part of any Forge trait.
    pub fn branch_exists(&self, repo: &RepositoryId, branch: &str) -> bool {
        self.lock().state.branch_exists(repo, branch)
    }

    pub(crate) fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().expect("memory forge mutex is poisoned")
    }

    pub(crate) fn effective_user(&self, inner: &Inner) -> User {
        self.current_user
            .clone()
            .unwrap_or_else(|| inner.state.current_user.clone())
    }
}

impl Default for MemoryForge {
    fn default() -> Self {
        Self::new()
    }
}
