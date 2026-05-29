//! In-memory Forge backend for Harness development and tests.
//!
//! [`MemoryForge`] implements [`harness_forge::Forge`] entirely in process: all
//! records live in ordinary collections behind a single mutex, with no
//! filesystem, network, or async runtime involved. It is a sibling reference
//! backend to `harness-forge-filesystem` and intentionally reproduces the same
//! deterministic identifier scheme, logical clock, ordering, and query
//! semantics, including native dependency links and pull-request reviews (see
//! ADR 0008), so workflow tests can swap between the two without changing
//! expectations.
//!
//! Because there is no durable store to corrupt, a small one-shot
//! [`fault hook`](MemoryForge::fail_next) lets tests force a chosen operation to
//! return [`ForgeError::Backend`](harness_forge::ForgeError::Backend) so backend
//! error paths stay exercisable. CI jobs have no create operation in the Forge
//! interface, so [`MemoryForge::seed_ci_jobs`] seeds them directly, mirroring how
//! the filesystem backend seeds its `ci_jobs.json` fixture. [`MemoryForge::as_user`]
//! creates another handle over the same store with a different current-user
//! identity, matching the per-process identity seam used by runner tests.

mod dependencies;
mod fault;
mod ids;
mod lists;
mod operations;
mod reviews;
mod state;
mod util;

use crate::fault::FaultStore;
use crate::state::State;
use harness_forge::{CiJob, RepositoryId, User};
use std::sync::{Arc, Mutex, MutexGuard};

pub use crate::fault::FaultOp;

/// The mutex-guarded interior: record store plus armed faults.
pub(crate) struct Inner {
    pub(crate) state: State,
    pub(crate) faults: FaultStore,
}

/// In-memory [`Forge`](harness_forge::Forge) backend.
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
    /// [`ForgeError::Backend`](harness_forge::ForgeError::Backend) with `message`
    /// before touching any state; later calls proceed normally. Arming the same
    /// op again queues another fault.
    pub fn fail_next(&self, op: FaultOp, message: impl Into<String>) {
        self.lock().faults.arm(op, message.into());
    }

    /// Clears every armed fault.
    pub fn clear_faults(&self) {
        self.lock().faults.clear();
    }

    /// Seeds CI jobs for a repository, replacing any previously seeded jobs.
    ///
    /// The Forge interface has no CI-job creation operation, so tests inject
    /// deterministic fixture jobs through this hook, matching the filesystem
    /// backend's `ci_jobs.json` seeding.
    pub fn seed_ci_jobs(&self, repo_id: &RepositoryId, jobs: Vec<CiJob>) {
        self.lock().state.set_ci_jobs(repo_id, jobs);
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
