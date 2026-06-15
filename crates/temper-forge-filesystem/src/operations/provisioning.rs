//! [`ForgeContent`] and [`ForgeAdmin`] provisioning operations for
//! [`FilesystemForge`].
//!
//! These persist the same operations the in-memory backend keeps in maps as
//! JSON records under the store layout (see
//! [`storage::provisioning`](crate::storage)). Every mutation holds the
//! store-level write lock for its whole read-modify-write-persist critical
//! section, matching the rest of the backend. Semantics mirror the in-memory
//! backend exactly, including the **deterministic** token format
//! `mem-token-{login}-{n}` (a per-login counter derived from the persisted token
//! list; no clock, PID, or randomness) so the two doubles stay in parity.

use crate::FilesystemForge;
use crate::storage::{UserRecord, WebhookRecord};
use crate::validation::validate_create_repository;
use async_trait::async_trait;
use temper_forge_model::{
    AccessGrant, AccessScope, ChangeKind, CommitFile, CreateBranch, CreateRepository, ForgeContent,
    ForgeError, ForgeResult, NewUser, RepositoryId, RepositoryPath, TokenScope, WebhookSpec,
};

#[async_trait]
impl ForgeContent for FilesystemForge {
    async fn ensure_repository(&self, input: CreateRepository) -> ForgeResult<RepositoryId> {
        validate_create_repository(&input)?;

        let _guard = self.write_lock()?;
        let path = RepositoryPath::new(input.owner.clone(), input.name.clone());
        if let Some(existing) = self.find_repository_by_path(&path)? {
            return Ok(existing.id);
        }

        let mut metadata = self.read_metadata()?;
        let now = crate::metadata::next_timestamp(&mut metadata)?;
        let repository = temper_forge_model::Repository {
            id: self.next_repository_id(&mut metadata)?,
            owner: input.owner,
            name: input.name,
            default_branch: input.default_branch,
            description: input.description,
            created_at: now,
            updated_at: now,
        };
        let id = repository.id.clone();

        let repository_path = self
            .repository_file(&id)
            .ok_or_else(|| ForgeError::Backend(format!("generated invalid repository id {id}")))?;
        self.write_json(&repository_path, &repository)?;
        self.write_metadata(&metadata)?;
        self.publish_path_hint(path, ChangeKind::Unknown);
        Ok(id)
    }

    async fn require_repository(&self, path: &RepositoryPath) -> ForgeResult<RepositoryId> {
        self.find_repository_by_path(path)?
            .map(|repository| repository.id)
            .ok_or_else(|| ForgeError::NotFound(format!("repository {}/{}", path.owner, path.name)))
    }

    async fn commit_file(&self, repo: &RepositoryId, input: CommitFile) -> ForgeResult<()> {
        let _guard = self.write_lock()?;
        self.require_repository(repo)?;
        self.write_content_file(repo, &input.branch, &input.path, &input.contents)?;

        let mut branches = self.read_branches(repo)?;
        branches.insert(input.branch);
        self.write_branches(repo, &branches)?;

        self.publish_repo_hint(repo, ChangeKind::Unknown);
        Ok(())
    }

    async fn create_branch(&self, repo: &RepositoryId, input: CreateBranch) -> ForgeResult<()> {
        let _guard = self.write_lock()?;
        self.require_repository(repo)?;
        let mut branches = self.read_branches(repo)?;
        branches.insert(input.new_branch);
        self.write_branches(repo, &branches)?;
        self.publish_repo_hint(repo, ChangeKind::Unknown);
        Ok(())
    }
}

#[async_trait]
impl temper_forge_model::ForgeAdmin for FilesystemForge {
    async fn ensure_owner(&self, owner: &str) -> ForgeResult<()> {
        let _guard = self.write_lock()?;
        let mut owners = self.read_owners()?;
        owners.entry(owner.to_string()).or_default();
        self.write_owners(&owners)
    }

    async fn ensure_user(&self, input: NewUser) -> ForgeResult<()> {
        let _guard = self.write_lock()?;
        let mut users = self.read_users()?;
        users.entry(input.login.clone()).or_insert(UserRecord {
            login: input.login,
            email: input.email,
            password: input.password,
        });
        self.write_users(&users)
    }

    async fn mint_token(&self, login: &str, _scopes: &[TokenScope]) -> ForgeResult<String> {
        let _guard = self.write_lock()?;
        let mut tokens = self.read_tokens()?;
        let login_tokens = tokens.entry(login.to_string()).or_default();
        let number = login_tokens
            .len()
            .checked_add(1)
            .ok_or_else(|| ForgeError::Backend("token counter overflowed".into()))?;
        let token = format!("mem-token-{login}-{number}");
        login_tokens.push(token.clone());
        self.write_tokens(&tokens)?;
        Ok(token)
    }

    async fn grant_access(&self, grant: AccessGrant) -> ForgeResult<()> {
        let _guard = self.write_lock()?;
        match grant.scope {
            AccessScope::OrgOwners => {
                if let Some(repository) = self.find_repository_by_id(&grant.repo)? {
                    let mut owners = self.read_owners()?;
                    owners
                        .entry(repository.owner)
                        .or_default()
                        .insert(grant.login);
                    self.write_owners(&owners)?;
                }
            }
            AccessScope::RepoCollaborator => {
                let mut grants = self.read_grants(&grant.repo)?;
                grants.insert(grant.login, grant.permission);
                self.write_grants(&grant.repo, &grants)?;
            }
        }
        Ok(())
    }

    async fn ensure_webhook(&self, repo: &RepositoryId, input: WebhookSpec) -> ForgeResult<()> {
        let _guard = self.write_lock()?;
        self.require_repository(repo)?;
        let mut webhooks = self.read_webhooks(repo)?;
        if webhooks.iter().any(|hook| hook.url == input.url) {
            return Ok(());
        }
        webhooks.push(WebhookRecord {
            url: input.url,
            secret: input.secret,
            events: input.events,
        });
        self.write_webhooks(repo, &webhooks)
    }

    async fn enable_ci(&self, repo: &RepositoryId) -> ForgeResult<()> {
        let _guard = self.write_lock()?;
        self.require_repository(repo)?;
        self.write_ci_enabled(repo)
    }
}
