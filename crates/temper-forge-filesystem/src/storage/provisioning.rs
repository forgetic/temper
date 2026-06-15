//! On-disk provisioning records and their read/write helpers.
//!
//! These back the [`ForgeContent`](temper_forge::ForgeContent) and
//! [`ForgeAdmin`](temper_forge::ForgeAdmin) capabilities. Store-global records
//! (users, tokens, org Owners-team membership) live at the store root; the
//! repository-scoped records (webhooks, collaborator grants, branches, the
//! CI-enabled marker, and committed content) live under
//! `repositories/<repo-id>/`, mirroring how the in-memory backend keys the same
//! state. Committed file contents are stored verbatim under a
//! `content/<branch>/` subtree so a branch's content is recoverable as a unit.

use crate::FilesystemForge;
use crate::errors::backend_error;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use temper_forge::{ForgeError, ForgeResult, RepoPermission, RepositoryId, WebhookEvents};

/// On-disk provisioned-user record.
///
/// Mirrors [`NewUser`](temper_forge::NewUser) and the in-memory `MemUser`.
/// `password` is retained verbatim so provisioning tests can assert it; this is
/// a development/test backend only.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct UserRecord {
    /// Login/username for the account.
    pub login: String,
    /// Email address for the account.
    pub email: String,
    /// Initial account password recorded at creation.
    pub password: String,
}

/// On-disk webhook record.
///
/// Mirrors [`WebhookSpec`](temper_forge::WebhookSpec) (which is not itself
/// serializable, to keep its secret out of `Debug`). `secret` is retained
/// verbatim; this is a development/test backend only.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct WebhookRecord {
    pub(crate) url: String,
    pub(crate) secret: String,
    pub(crate) events: WebhookEvents,
}

impl FilesystemForge {
    fn read_json_or_default<T>(&self, path: &std::path::Path) -> ForgeResult<T>
    where
        T: serde::de::DeserializeOwned + Default,
    {
        if path.exists() {
            self.read_json(path)
        } else {
            Ok(T::default())
        }
    }

    // --- Store-global records ---

    pub(crate) fn read_users(&self) -> ForgeResult<BTreeMap<String, UserRecord>> {
        self.ensure_layout()?;
        self.read_json_or_default(&self.users_file())
    }

    pub(crate) fn write_users(&self, users: &BTreeMap<String, UserRecord>) -> ForgeResult<()> {
        self.write_json(&self.users_file(), users)
    }

    pub(crate) fn read_tokens(&self) -> ForgeResult<BTreeMap<String, Vec<String>>> {
        self.ensure_layout()?;
        self.read_json_or_default(&self.tokens_file())
    }

    pub(crate) fn write_tokens(&self, tokens: &BTreeMap<String, Vec<String>>) -> ForgeResult<()> {
        self.write_json(&self.tokens_file(), tokens)
    }

    pub(crate) fn read_owners(&self) -> ForgeResult<BTreeMap<String, BTreeSet<String>>> {
        self.ensure_layout()?;
        self.read_json_or_default(&self.owners_file())
    }

    pub(crate) fn write_owners(
        &self,
        owners: &BTreeMap<String, BTreeSet<String>>,
    ) -> ForgeResult<()> {
        self.write_json(&self.owners_file(), owners)
    }

    // --- Repository-scoped records ---

    pub(crate) fn read_webhooks(&self, repo_id: &RepositoryId) -> ForgeResult<Vec<WebhookRecord>> {
        let Some(path) = self.webhooks_file(repo_id) else {
            return Err(ForgeError::Backend(format!(
                "invalid repository id {repo_id} for webhooks path"
            )));
        };
        self.read_json_or_default(&path)
    }

    pub(crate) fn write_webhooks(
        &self,
        repo_id: &RepositoryId,
        webhooks: &[WebhookRecord],
    ) -> ForgeResult<()> {
        let Some(path) = self.webhooks_file(repo_id) else {
            return Err(ForgeError::Backend(format!(
                "invalid repository id {repo_id} for webhooks path"
            )));
        };
        self.write_json(&path, &webhooks)
    }

    pub(crate) fn read_grants(
        &self,
        repo_id: &RepositoryId,
    ) -> ForgeResult<BTreeMap<String, RepoPermission>> {
        let Some(path) = self.grants_file(repo_id) else {
            return Err(ForgeError::Backend(format!(
                "invalid repository id {repo_id} for grants path"
            )));
        };
        self.read_json_or_default(&path)
    }

    pub(crate) fn write_grants(
        &self,
        repo_id: &RepositoryId,
        grants: &BTreeMap<String, RepoPermission>,
    ) -> ForgeResult<()> {
        let Some(path) = self.grants_file(repo_id) else {
            return Err(ForgeError::Backend(format!(
                "invalid repository id {repo_id} for grants path"
            )));
        };
        self.write_json(&path, grants)
    }

    pub(crate) fn read_branches(&self, repo_id: &RepositoryId) -> ForgeResult<BTreeSet<String>> {
        let Some(path) = self.branches_file(repo_id) else {
            return Err(ForgeError::Backend(format!(
                "invalid repository id {repo_id} for branches path"
            )));
        };
        self.read_json_or_default(&path)
    }

    pub(crate) fn write_branches(
        &self,
        repo_id: &RepositoryId,
        branches: &BTreeSet<String>,
    ) -> ForgeResult<()> {
        let Some(path) = self.branches_file(repo_id) else {
            return Err(ForgeError::Backend(format!(
                "invalid repository id {repo_id} for branches path"
            )));
        };
        self.write_json(&path, branches)
    }

    pub(crate) fn read_ci_enabled(&self, repo_id: &RepositoryId) -> ForgeResult<bool> {
        let Some(path) = self.ci_enabled_file(repo_id) else {
            return Err(ForgeError::Backend(format!(
                "invalid repository id {repo_id} for CI-enabled path"
            )));
        };
        Ok(path.exists())
    }

    pub(crate) fn write_ci_enabled(&self, repo_id: &RepositoryId) -> ForgeResult<()> {
        let Some(path) = self.ci_enabled_file(repo_id) else {
            return Err(ForgeError::Backend(format!(
                "invalid repository id {repo_id} for CI-enabled path"
            )));
        };
        self.write_json(&path, &true)
    }

    pub(crate) fn write_content_file(
        &self,
        repo_id: &RepositoryId,
        branch: &str,
        path: &str,
        contents: &[u8],
    ) -> ForgeResult<()> {
        let Some(file) = self.content_file(repo_id, branch, path) else {
            return Err(ForgeError::Backend(format!(
                "invalid repository id {repo_id} for content path"
            )));
        };
        let parent = file.parent().ok_or_else(|| {
            ForgeError::Backend(format!("content path {} has no parent", file.display()))
        })?;
        fs::create_dir_all(parent)
            .map_err(|error| backend_error(format!("create {}", parent.display()), error))?;
        fs::write(&file, contents)
            .map_err(|error| backend_error(format!("write {}", file.display()), error))
    }

    pub(crate) fn read_content_file(
        &self,
        repo_id: &RepositoryId,
        branch: &str,
        path: &str,
    ) -> ForgeResult<Option<Vec<u8>>> {
        let Some(file) = self.content_file(repo_id, branch, path) else {
            return Ok(None);
        };
        if !file.exists() {
            return Ok(None);
        }
        fs::read(&file)
            .map(Some)
            .map_err(|error| backend_error(format!("read {}", file.display()), error))
    }
}
