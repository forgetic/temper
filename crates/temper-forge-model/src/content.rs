//! Portable content-provisioning capability.
//!
//! [`ForgeContent`] abstracts the *unprivileged* half of provisioning: creating
//! and locating repositories and writing content (commits and branches) into
//! them. It is the backend-agnostic counterpart to the raw Forgejo REST calls
//! that today live as free functions in `temper-forgejo-ops`. It is an additive
//! capability trait; it does not widen the existing [`Forge`](crate::Forge)
//! trait.
//!
//! Every method is idempotent in the ensure/upsert sense documented per method:
//! re-running provisioning against a partially-provisioned backend converges to
//! the same end state without erroring on already-existing resources.

use crate::ForgeResult;
use crate::ids::RepositoryId;
use crate::model::{CreateRepository, RepositoryPath};
use async_trait::async_trait;

/// Input used to commit a single file into a repository on a branch.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommitFile {
    /// Repository-relative path of the file to write (e.g.
    /// `".forgejo/workflows/ci.yml"`).
    pub path: String,
    /// Raw file contents. Backends encode this as required by their content API.
    pub contents: Vec<u8>,
    /// Commit message recorded for the write.
    pub message: String,
    /// Branch the commit is applied to.
    pub branch: String,
}

/// Input used to create a branch from an existing branch.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreateBranch {
    /// Name of the branch to create.
    pub new_branch: String,
    /// Name of the existing branch the new branch is forked from.
    pub from_branch: String,
}

/// Backend-agnostic content-provisioning capability.
///
/// Implementations adapt these operations to a concrete backend (e.g. Forgejo).
/// All methods are idempotent so provisioning can be safely re-run.
#[async_trait]
pub trait ForgeContent: Send + Sync {
    /// Ensures a repository exists, returning its stable identifier.
    ///
    /// Idempotent: if a repository already exists at the requested owner/name it
    /// is left unchanged and its identifier is returned; only an absent
    /// repository is created. Re-running provisioning is benign.
    async fn ensure_repository(&self, input: CreateRepository) -> ForgeResult<RepositoryId>;

    /// Requires that a repository already exists, returning its identifier.
    ///
    /// Unlike [`ensure_repository`](Self::ensure_repository) this never creates
    /// anything: an absent repository is a hard error
    /// ([`ForgeError::NotFound`](crate::ForgeError::NotFound)). Used when
    /// provisioning onto a repository an operator named explicitly, so a typo
    /// fails loudly instead of silently creating a bare repo.
    async fn require_repository(&self, path: &RepositoryPath) -> ForgeResult<RepositoryId>;

    /// Commits a single file onto a branch of `repo`.
    ///
    /// Idempotent in the upsert sense: writing a file that already exists with
    /// the same path is benign (the backend treats an already-present create as
    /// a no-op conflict) so re-running provisioning does not error.
    async fn commit_file(&self, repo: &RepositoryId, input: CommitFile) -> ForgeResult<()>;

    /// Creates a branch in `repo` from an existing branch.
    ///
    /// Idempotent: creating a branch that already exists is benign so re-running
    /// provisioning does not error.
    async fn create_branch(&self, repo: &RepositoryId, input: CreateBranch) -> ForgeResult<()>;
}
