//! Shared provisioning vocabulary: access scopes, permissions, and webhook
//! event selection.
//!
//! These portable types describe *how* an identity is granted access to a
//! repository or organization and *which* events a webhook subscribes to. They
//! are backend-agnostic: a concrete backend maps each variant onto its own host
//! mechanism (for Forgejo, Owners-team membership vs. a repo collaborator grant,
//! and the Gitea webhook event list).

use serde::{Deserialize, Serialize};

/// How a provisioned identity is granted access to a repository.
///
/// This deliberately spans both org-level and repo-level mechanisms so a single
/// [`ForgeAdmin::grant_access`](crate::ForgeAdmin::grant_access) call can pick
/// the host mechanism from the scope:
///
/// - [`AccessScope::OrgOwners`] adds the identity to the organization **Owners**
///   team, making it an owner of *every* repository in the org — correct only
///   for a throwaway org dedicated to a single demo run.
/// - [`AccessScope::RepoCollaborator`] grants a repo-scoped collaborator
///   permission on a single target repository and never touches the Owners
///   team — the narrower scope used when provisioning onto a shared org.
///
/// Defaults to [`AccessScope::OrgOwners`], reproducing today's throwaway-repo
/// behavior.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessScope {
    /// Add the identity to the org **Owners** team (owner of every repo in the
    /// org).
    #[default]
    OrgOwners,
    /// Grant the identity a repo-scoped collaborator permission on the target
    /// repo and never touch the Owners team.
    RepoCollaborator,
}

/// Repository-scoped permission level granted to a collaborator.
///
/// Maps onto the host's collaborator permission (for Forgejo/Gitea,
/// `"read" | "write" | "admin"`). [`RepoPermission::Write`] is enough for an
/// automation account to merge approved, green PRs and read Actions status over
/// the web UI; [`RepoPermission::Admin`] is intentionally reserved for a
/// concrete need.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RepoPermission {
    /// Read-only access.
    Read,
    /// Push access — enough to merge approved PRs and read Actions status.
    #[default]
    Write,
    /// Full administrative access to the repository.
    Admin,
}

/// Selection of webhook events a [`WebhookSpec`](crate::WebhookSpec) subscribes
/// to.
///
/// [`WebhookEvents::All`] requests the backend's full set of collaboration and
/// CI events (pushes, issues, comments, pull requests, reviews, and Actions
/// run lifecycle). [`WebhookEvents::Only`] names an explicit list of
/// backend-specific event identifiers for callers that need a narrower
/// subscription.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WebhookEvents {
    /// Subscribe to the backend's full set of collaboration and CI events.
    #[default]
    All,
    /// Subscribe to exactly the named backend-specific event identifiers.
    Only(Vec<String>),
}
