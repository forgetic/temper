//! [`ForgeContent`] and [`ForgeAdmin`] provisioning operations for
//! [`MemoryForge`](crate::MemoryForge).
//!
//! These adapt the portable provisioning capabilities onto the in-memory
//! [`State`](crate::state::State), letting provisioning be exercised with no
//! real forge or network. Every operation is idempotent in the ensure/upsert
//! sense documented on the traits, and [`mint_token`](MemoryForge::mint_token)
//! is fully deterministic (a per-login counter, no clock or randomness) so the
//! double is reproducible across runs.

use crate::MemoryForge;
use crate::util::validate_create_repository;
use async_trait::async_trait;
use temper_forge_model::{
    AccessGrant, AccessScope, ChangeKind, CommitFile, CreateBranch, CreateRepository, ForgeContent,
    ForgeResult, NewUser, RepositoryId, RepositoryPath, TokenScope, WebhookSpec,
};

#[async_trait]
impl ForgeContent for MemoryForge {
    async fn ensure_repository(&self, input: CreateRepository) -> ForgeResult<RepositoryId> {
        validate_create_repository(&input)?;

        let mut inner = self.lock();
        let path = RepositoryPath::new(input.owner.clone(), input.name.clone());
        if let Some(existing) = inner.state.find_repository_by_path(&path) {
            return Ok(existing.id);
        }

        let now = inner.state.next_timestamp()?;
        let id = inner.state.allocate_repository_id()?;
        let repository = temper_forge_model::Repository {
            id: id.clone(),
            owner: input.owner,
            name: input.name,
            default_branch: input.default_branch,
            description: input.description,
            created_at: now,
            updated_at: now,
        };
        inner.state.insert_repository(repository.clone());
        inner.publish_path_hint(path, ChangeKind::Unknown);
        Ok(id)
    }

    async fn require_repository(&self, path: &RepositoryPath) -> ForgeResult<RepositoryId> {
        let inner = self.lock();
        inner
            .state
            .find_repository_by_path(path)
            .map(|repository| repository.id)
            .ok_or_else(|| {
                temper_forge_model::ForgeError::NotFound(format!(
                    "repository {}/{}",
                    path.owner, path.name
                ))
            })
    }

    async fn commit_file(&self, repo: &RepositoryId, input: CommitFile) -> ForgeResult<()> {
        let mut inner = self.lock();
        inner.state.require_repository(repo)?;
        inner
            .state
            .commit_file(repo, &input.branch, &input.path, input.contents);
        inner.publish_repo_hint(repo, ChangeKind::Unknown);
        Ok(())
    }

    async fn create_branch(&self, repo: &RepositoryId, input: CreateBranch) -> ForgeResult<()> {
        let mut inner = self.lock();
        inner.state.require_repository(repo)?;
        inner.state.create_branch(repo, &input.new_branch);
        inner.publish_repo_hint(repo, ChangeKind::Unknown);
        Ok(())
    }
}

#[async_trait]
impl temper_forge_model::ForgeAdmin for MemoryForge {
    async fn ensure_owner(&self, owner: &str) -> ForgeResult<()> {
        self.lock().state.ensure_owner(owner);
        Ok(())
    }

    async fn ensure_user(&self, input: NewUser) -> ForgeResult<()> {
        self.lock().state.ensure_user(crate::state::MemUser {
            login: input.login,
            email: input.email,
            password: input.password,
        });
        Ok(())
    }

    async fn mint_token(&self, login: &str, _scopes: &[TokenScope]) -> ForgeResult<String> {
        self.lock().state.mint_token(login)
    }

    async fn grant_access(&self, grant: AccessGrant) -> ForgeResult<()> {
        let mut inner = self.lock();
        match grant.scope {
            AccessScope::OrgOwners => {
                if let Some(repository) = inner.state.find_repository_by_id(&grant.repo) {
                    let owner = repository.owner;
                    inner.state.add_owner_member(&owner, &grant.login);
                }
            }
            AccessScope::RepoCollaborator => {
                inner
                    .state
                    .grant_repo(&grant.repo, &grant.login, grant.permission);
            }
        }
        Ok(())
    }

    async fn ensure_webhook(&self, repo: &RepositoryId, input: WebhookSpec) -> ForgeResult<()> {
        let mut inner = self.lock();
        inner.state.require_repository(repo)?;
        inner.state.ensure_webhook(repo, input);
        Ok(())
    }

    async fn enable_ci(&self, repo: &RepositoryId) -> ForgeResult<()> {
        let mut inner = self.lock();
        inner.state.require_repository(repo)?;
        inner.state.enable_ci(repo);
        Ok(())
    }
}
