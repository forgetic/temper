//! Privileged provisioning capability.
//!
//! [`ForgeAdmin`] abstracts the *privileged* half of provisioning: creating
//! organizations and users, minting access tokens, granting access, registering
//! webhooks, and enabling CI. It is the backend-agnostic counterpart to the raw
//! Forgejo admin REST calls that today live as free functions in
//! `temper-forgejo-ops`. It is an additive capability trait; it does not widen
//! the existing [`Forge`](crate::Forge) trait.
//!
//! Several inputs and outputs are **secret** (passwords, webhook secrets, and
//! minted tokens). They are called out per item below and must never be logged.
//! Every method is idempotent in the ensure/upsert sense documented per method.

use crate::ForgeResult;
use crate::ids::RepositoryId;
use crate::model::{AccessScope, RepoPermission, WebhookEvents};
use async_trait::async_trait;

/// Input used to create a backend user account.
///
/// `password` is **secret** and must never be logged.
#[derive(Clone)]
pub struct NewUser {
    /// Login/username for the account.
    pub login: String,
    /// Email address for the account.
    pub email: String,
    /// Initial account password. **Secret** — never log this field.
    pub password: String,
}

impl std::fmt::Debug for NewUser {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NewUser")
            .field("login", &self.login)
            .field("email", &self.email)
            .field("password", &"<redacted>")
            .finish()
    }
}

/// Access-token scope requested when minting a token.
///
/// Each variant maps onto the backend's native scope string (for Forgejo:
/// `read:organization`, `write:repository`, `write:issue`, `write:user`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TokenScope {
    /// Read access to organizations.
    ReadOrg,
    /// Write access to repositories.
    WriteRepository,
    /// Write access to issues.
    WriteIssue,
    /// Write access to the user's own account resources.
    WriteUser,
}

/// Input used to grant an identity access to a repository or organization.
///
/// This deliberately subsumes both org-Owners-team membership and repo
/// collaborator grants: the backend picks the host mechanism from
/// [`scope`](Self::scope).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AccessGrant {
    /// Login of the identity being granted access.
    pub login: String,
    /// Repository the grant targets (used by repo-scoped grants).
    pub repo: RepositoryId,
    /// Whether to grant via the org Owners team or as a repo collaborator.
    pub scope: AccessScope,
    /// Permission level for repo-scoped grants.
    pub permission: RepoPermission,
}

/// Specification for a repository webhook.
///
/// `secret` is **secret** and must never be logged.
#[derive(Clone)]
pub struct WebhookSpec {
    /// Delivery URL the webhook posts events to.
    pub url: String,
    /// Shared secret used to sign webhook deliveries. **Secret** — never log
    /// this field.
    pub secret: String,
    /// Events the webhook subscribes to.
    pub events: WebhookEvents,
}

impl std::fmt::Debug for WebhookSpec {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WebhookSpec")
            .field("url", &self.url)
            .field("secret", &"<redacted>")
            .field("events", &self.events)
            .finish()
    }
}

/// Backend-agnostic privileged-provisioning capability.
///
/// Implementations adapt these operations to a concrete backend (e.g. Forgejo).
/// All methods are idempotent so provisioning can be safely re-run.
#[async_trait]
pub trait ForgeAdmin: Send + Sync {
    /// Ensures an organization (repository owner) exists.
    ///
    /// Idempotent: an already-existing owner is left unchanged and is not an
    /// error, so re-running provisioning is benign.
    async fn ensure_owner(&self, owner: &str) -> ForgeResult<()>;

    /// Ensures a user account exists.
    ///
    /// Idempotent: an already-existing login is left unchanged and is not an
    /// error. `input.password` is **secret** and must never be logged.
    async fn ensure_user(&self, input: NewUser) -> ForgeResult<()>;

    /// Mints an access token for `login` with the requested scopes.
    ///
    /// The returned [`String`] is a **secret** token and must never be logged.
    /// Token names are backend-unique, so implementations generate a unique,
    /// non-secret name per call; repeated minting therefore does not fail with a
    /// duplicate-token conflict.
    async fn mint_token(&self, login: &str, scopes: &[TokenScope]) -> ForgeResult<String>;

    /// Grants an identity access to a repository or organization.
    ///
    /// The backend picks the host mechanism from
    /// [`grant.scope`](AccessGrant::scope): org Owners-team membership for
    /// [`AccessScope::OrgOwners`], or a repo-scoped collaborator permission for
    /// [`AccessScope::RepoCollaborator`]. Idempotent: re-granting an
    /// already-granted identity is benign.
    async fn grant_access(&self, grant: AccessGrant) -> ForgeResult<()>;

    /// Ensures a webhook is registered on `repo`.
    ///
    /// Idempotent: if a webhook with the same delivery URL is already registered
    /// it is left unchanged and no duplicate is created. `input.secret` is
    /// **secret** and must never be logged.
    async fn ensure_webhook(&self, repo: &RepositoryId, input: WebhookSpec) -> ForgeResult<()>;

    /// Enables CI (Actions) on `repo`.
    ///
    /// Idempotent: enabling CI on a repository where it is already enabled is
    /// benign so re-running provisioning does not error.
    async fn enable_ci(&self, repo: &RepositoryId) -> ForgeResult<()>;
}
