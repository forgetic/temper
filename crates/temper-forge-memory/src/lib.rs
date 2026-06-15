//! In-memory Forge backend for Temper development and tests.
//!
//! [`MemoryForge`] implements [`temper_forge::Forge`] entirely in process: all
//! records live in ordinary collections behind a single mutex, with no
//! filesystem, network, or async runtime involved. It is a sibling reference
//! backend to `temper-forge-filesystem` and intentionally reproduces the same
//! deterministic identifier scheme, logical clock, ordering, and query
//! semantics, including native dependency links and pull-request reviews (see
//! ADR 0008), so workflow tests can swap between the two without changing
//! expectations.
//!
//! Because there is no durable store to corrupt, a small one-shot
//! [`fault hook`](MemoryForge::fail_next) lets tests force a chosen operation to
//! return [`ForgeError::Backend`](temper_forge::ForgeError::Backend) so backend
//! error paths stay exercisable. CI jobs have no create operation in the Forge
//! interface, so [`MemoryForge::seed_ci_jobs`] seeds them directly, mirroring how
//! the filesystem backend seeds its `ci_jobs.json` fixture. [`MemoryForge::as_user`]
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
use temper_forge::{CiJob, RepoPermission, RepositoryId, User, WebhookSpec};

pub use crate::fault::FaultOp;
pub use crate::hint::MemoryHintReceiver;
pub use crate::state::MemUser;

/// The mutex-guarded interior: record store plus armed faults.
pub(crate) struct Inner {
    pub(crate) state: State,
    pub(crate) faults: FaultStore,
    pub(crate) hints: HintBus,
}

/// In-memory [`Forge`](temper_forge::Forge) backend.
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
    /// [`ForgeError::Backend`](temper_forge::ForgeError::Backend) with `message`
    /// before touching any state; later calls proceed normally. Arming the same
    /// op again queues another fault.
    pub fn fail_next(&self, op: FaultOp, message: impl Into<String>) {
        self.lock().faults.arm(op, message.into());
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
        inner.publish_repo_hint(repo_id, temper_forge::ChangeKind::Ci);
    }

    /// Returns every user provisioned via
    /// [`ForgeAdmin::ensure_user`](temper_forge::ForgeAdmin::ensure_user),
    /// ordered by login.
    ///
    /// Test-only read-back companion surface, not part of any Forge trait,
    /// mirroring the filesystem backend's accessor of the same name.
    pub fn provisioned_users(&self) -> Vec<MemUser> {
        self.lock().state.provisioned_users()
    }

    /// Returns the webhooks registered on `repo` via
    /// [`ForgeAdmin::ensure_webhook`](temper_forge::ForgeAdmin::ensure_webhook).
    ///
    /// Test-only read-back companion surface, not part of any Forge trait.
    pub fn webhooks(&self, repo: &RepositoryId) -> Vec<WebhookSpec> {
        self.lock().state.webhooks(repo)
    }

    /// Returns the contents committed at `(repo, branch, path)` via
    /// [`ForgeContent::commit_file`](temper_forge::ForgeContent::commit_file),
    /// or `None` if no such file was committed.
    ///
    /// Test-only read-back companion surface, not part of any Forge trait.
    pub fn committed_file(&self, repo: &RepositoryId, branch: &str, path: &str) -> Option<Vec<u8>> {
        self.lock().state.committed_file(repo, branch, path)
    }

    /// Returns the repo-scoped collaborator grants recorded on `repo` via
    /// [`ForgeAdmin::grant_access`](temper_forge::ForgeAdmin::grant_access) with
    /// [`AccessScope::RepoCollaborator`](temper_forge::AccessScope::RepoCollaborator),
    /// keyed by login.
    ///
    /// Test-only read-back companion surface, not part of any Forge trait.
    pub fn grants(&self, repo: &RepositoryId) -> BTreeMap<String, RepoPermission> {
        self.lock().state.grants(repo)
    }

    /// Returns the tokens minted for `login` via
    /// [`ForgeAdmin::mint_token`](temper_forge::ForgeAdmin::mint_token), in mint
    /// order.
    ///
    /// Test-only read-back companion surface, not part of any Forge trait.
    pub fn minted_tokens(&self, login: &str) -> Vec<String> {
        self.lock().state.minted_tokens(login)
    }

    /// Reports whether CI was enabled on `repo` via
    /// [`ForgeAdmin::enable_ci`](temper_forge::ForgeAdmin::enable_ci).
    ///
    /// Test-only read-back companion surface, not part of any Forge trait.
    pub fn ci_enabled(&self, repo: &RepositoryId) -> bool {
        self.lock().state.ci_enabled(repo)
    }

    /// Reports whether `branch` was recorded on `repo` via
    /// [`ForgeContent::create_branch`](temper_forge::ForgeContent::create_branch)
    /// or as the target of a [`commit_file`](temper_forge::ForgeContent::commit_file).
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
