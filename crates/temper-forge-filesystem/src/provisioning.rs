//! Test-only read-back accessors for provisioned state.
//!
//! These are a companion surface to the
//! [`ForgeContent`](temper_forge_model::ForgeContent) and
//! [`ForgeAdmin`](temper_forge_model::ForgeAdmin) trait impls, not part of any Forge
//! trait. They let provisioning tests assert what was persisted, and mirror the
//! identically named accessors on the in-memory backend so the same test passes
//! against both backends.

use crate::FilesystemForge;
use crate::storage::UserRecord;
use std::collections::BTreeMap;
use temper_forge_model::{ForgeResult, RepoPermission, RepositoryId, WebhookSpec};

/// A provisioned user account read back from the filesystem store.
///
/// Shares the field shape (`login`, `email`, `password`) of the in-memory
/// backend's `MemUser` so a single provisioning test can assert against either
/// backend.
pub type MemUser = UserRecord;

impl FilesystemForge {
    /// Returns every user provisioned via
    /// [`ForgeAdmin::ensure_user`](temper_forge_model::ForgeAdmin::ensure_user),
    /// ordered by login.
    ///
    /// Test-only read-back companion surface, not part of any Forge trait,
    /// mirroring the in-memory backend's accessor of the same name.
    pub fn provisioned_users(&self) -> ForgeResult<Vec<MemUser>> {
        Ok(self.read_users()?.into_values().collect())
    }

    /// Returns the webhooks registered on `repo` via
    /// [`ForgeAdmin::ensure_webhook`](temper_forge_model::ForgeAdmin::ensure_webhook).
    ///
    /// Test-only read-back companion surface, not part of any Forge trait.
    pub fn webhooks(&self, repo: &RepositoryId) -> ForgeResult<Vec<WebhookSpec>> {
        Ok(self
            .read_webhooks(repo)?
            .into_iter()
            .map(|record| WebhookSpec {
                url: record.url,
                secret: record.secret,
                events: record.events,
            })
            .collect())
    }

    /// Returns the contents committed at `(repo, branch, path)` via
    /// [`ForgeContent::commit_file`](temper_forge_model::ForgeContent::commit_file),
    /// or `None` if no such file was committed.
    ///
    /// Test-only read-back companion surface, not part of any Forge trait.
    pub fn committed_file(
        &self,
        repo: &RepositoryId,
        branch: &str,
        path: &str,
    ) -> ForgeResult<Option<Vec<u8>>> {
        self.read_content_file(repo, branch, path)
    }

    /// Returns the repo-scoped collaborator grants recorded on `repo` via
    /// [`ForgeAdmin::grant_access`](temper_forge_model::ForgeAdmin::grant_access) with
    /// [`AccessScope::RepoCollaborator`](temper_forge_model::AccessScope::RepoCollaborator),
    /// keyed by login.
    ///
    /// Test-only read-back companion surface, not part of any Forge trait.
    pub fn grants(&self, repo: &RepositoryId) -> ForgeResult<BTreeMap<String, RepoPermission>> {
        self.read_grants(repo)
    }

    /// Returns the tokens minted for `login` via
    /// [`ForgeAdmin::mint_token`](temper_forge_model::ForgeAdmin::mint_token), in mint
    /// order.
    ///
    /// Test-only read-back companion surface, not part of any Forge trait.
    pub fn minted_tokens(&self, login: &str) -> ForgeResult<Vec<String>> {
        Ok(self.read_tokens()?.get(login).cloned().unwrap_or_default())
    }

    /// Reports whether CI was enabled on `repo` via
    /// [`ForgeAdmin::enable_ci`](temper_forge_model::ForgeAdmin::enable_ci).
    ///
    /// Test-only read-back companion surface, not part of any Forge trait.
    pub fn ci_enabled(&self, repo: &RepositoryId) -> ForgeResult<bool> {
        self.read_ci_enabled(repo)
    }

    /// Reports whether `branch` was recorded on `repo` via
    /// [`ForgeContent::create_branch`](temper_forge_model::ForgeContent::create_branch)
    /// or as the target of a
    /// [`commit_file`](temper_forge_model::ForgeContent::commit_file).
    ///
    /// Test-only read-back companion surface, not part of any Forge trait.
    pub fn branch_exists(&self, repo: &RepositoryId, branch: &str) -> ForgeResult<bool> {
        Ok(self.read_branches(repo)?.contains(branch))
    }
}
