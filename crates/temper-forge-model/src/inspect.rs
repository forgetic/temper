//! Read-only provisioning/readiness inspection capability.
//!
//! [`ForgeReadiness`] complements the mutating provisioning traits with a narrow
//! read-only surface used by planning/preflight commands. Methods must not
//! create, update, mint, or grant anything: they only report provider state that
//! a reconciliation/apply pass would otherwise converge.

use crate::ForgeResult;
use crate::ids::RepositoryId;
use crate::model::WebhookEvents;
use async_trait::async_trait;

/// Non-secret view of a provisioned user account.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProvisionedUserStatus {
    /// Login/username of the account.
    pub login: String,
    /// Email address if the backend exposes one to the inspecting identity.
    pub email: Option<String>,
}

/// Non-secret view of a repository webhook.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WebhookStatus {
    /// Delivery URL configured for the hook.
    pub url: String,
    /// Events the hook subscribes to, when the backend exposes them.
    pub events: WebhookEvents,
}

/// Read-only provisioning state used by deployment planning.
#[async_trait]
pub trait ForgeReadiness: Send + Sync {
    /// Looks up a backend user by login without creating it.
    async fn get_provisioned_user(&self, login: &str)
    -> ForgeResult<Option<ProvisionedUserStatus>>;

    /// Lists repository webhooks without creating or updating them.
    async fn list_webhook_statuses(&self, repo: &RepositoryId) -> ForgeResult<Vec<WebhookStatus>>;

    /// Reports whether CI/Actions is enabled for `repo` when the backend exposes
    /// the flag. `None` means the backend cannot inspect this setting.
    async fn repository_ci_enabled(&self, repo: &RepositoryId) -> ForgeResult<Option<bool>>;
}
