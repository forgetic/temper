//! Cached reference-delivery Forgejo worlds.
//!
//! This module turns the reusable generic cache in `temper-forgejo-fixture` into
//! Temper's concrete reference-delivery state: admin token, org, role users,
//! labels, CI workflow, and one or more initialized repositories.

use super::provision::{
    bootstrap_admin, provision_repository, provision_role_identities, ProvisionError, Provisioned,
    ProvisionedRoles, Result, ADMIN_USER, CI_WORKFLOW, WORKFLOW_PATH,
};
use super::{ForgejoServer, ForgejoState};
use crate::{runner_config, workflow};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use temper_forge::RepositoryId;

/// Cached metadata for one initial reference-delivery Forgejo world.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ProvisionedRepositories {
    /// Shared admin/org/role identity state.
    pub roles: ProvisionedRoles,
    /// Default branch used by every provisioned repository.
    pub default_branch: String,
    /// Repository ids keyed by repository name.
    pub repositories: BTreeMap<String, RepositoryId>,
}

impl ProvisionedRepositories {
    /// Materializes repository `name` as a [`Provisioned`] value.
    pub fn provisioned(&self, name: &str) -> Option<Provisioned> {
        self.repositories
            .get(name)
            .map(|repo| self.roles.for_repository(name.to_string(), repo.clone()))
    }
}

/// A per-test Forgejo process restored from a cached provisioned world.
pub struct CachedProvisionedWorld {
    /// Running Forgejo server backed by a fresh `/tmp` copy of cached state.
    pub server: ForgejoServer,
    /// Tokens, roles, and repository ids for the cached state.
    pub state: ProvisionedRepositories,
    /// Whether this call reused an existing `.cache` tree.
    pub cache_hit: bool,
    /// Stable state-cache key, for diagnostics.
    pub cache_key: String,
}

/// A convenience wrapper for tests that need only the runner-config repository.
pub struct CachedProvisionedServer {
    /// Running Forgejo server backed by a fresh `/tmp` copy of cached state.
    pub server: ForgejoServer,
    /// Provisioned metadata for the default repository.
    pub provisioned: Provisioned,
    /// Whether this call reused an existing `.cache` tree.
    pub cache_hit: bool,
    /// Stable state-cache key, for diagnostics.
    pub cache_key: String,
}

/// Starts a per-test Forgejo from the cached reference-delivery state for the
/// runner-config repository.
pub fn start_cached_provisioned_server() -> Result<CachedProvisionedServer> {
    let config = runner_config();
    let world =
        start_cached_provisioned_repositories(std::slice::from_ref(&config.repository.name))?;
    let provisioned = world
        .state
        .provisioned(&config.repository.name)
        .ok_or_else(|| ProvisionError::Shape {
            what: "cached provisioned repository".into(),
            detail: format!(
                "{} missing from provisioned repository map",
                config.repository.name
            ),
        })?;
    Ok(CachedProvisionedServer {
        server: world.server,
        provisioned,
        cache_hit: world.cache_hit,
        cache_key: world.cache_key,
    })
}

/// Starts a per-test Forgejo from a cached reference-delivery world containing
/// exactly `repo_names` (after sorting and deduplication).
pub fn start_cached_provisioned_repositories(
    repo_names: &[String],
) -> Result<CachedProvisionedWorld> {
    let repo_names = normalize_repo_names(repo_names);
    let state = ForgejoState::new(reference_delivery_state_description(&repo_names))
        .map_err(ProvisionError::Fixture)?;
    let cached = ForgejoServer::start_with_state(&state, |server| {
        block_on_fixture(provision_repositories(server, &repo_names))
    })
    .map_err(ProvisionError::Fixture)?;
    Ok(CachedProvisionedWorld {
        server: cached.server,
        state: cached.metadata,
        cache_hit: cached.cache_hit,
        cache_key: cached.cache_key,
    })
}

pub(super) async fn provision_repositories(
    server: &ForgejoServer,
    repo_names: &[String],
) -> Result<ProvisionedRepositories> {
    let admin_token = bootstrap_admin(server)?;
    let config = runner_config();
    let roles = provision_role_identities(
        server.base_url(),
        &admin_token,
        &config.repository.owner,
        &config.role_bindings,
    )
    .await?;

    let mut repositories = BTreeMap::new();
    for name in repo_names {
        let provisioned = provision_repository(
            server.base_url(),
            &roles,
            name,
            &config.repository.default_branch,
        )
        .await?;
        repositories.insert(name.clone(), provisioned.repository);
    }

    Ok(ProvisionedRepositories {
        roles,
        default_branch: config.repository.default_branch,
        repositories,
    })
}

fn normalize_repo_names(repo_names: &[String]) -> Vec<String> {
    let config = runner_config();
    let mut names = repo_names
        .iter()
        .map(|name| name.trim())
        .filter(|name| !name.is_empty())
        .map(ToOwned::to_owned)
        .collect::<BTreeSet<_>>();
    if names.is_empty() {
        names.insert(config.repository.name);
    }
    names.into_iter().collect()
}

fn reference_delivery_state_description(repo_names: &[String]) -> serde_json::Value {
    let config = runner_config();
    let labels = workflow()
        .compile()
        .labels()
        .labels()
        .iter()
        .map(|label| label.id.to_string())
        .collect::<Vec<_>>();
    let roles = config
        .role_bindings
        .iter()
        .map(|binding| {
            serde_json::json!({
                "role": binding.role.to_string(),
                "user": binding.user.handle,
            })
        })
        .collect::<Vec<_>>();
    serde_json::json!({
        "kind": "temper-reference-delivery-provisioned-world",
        "version": 1,
        "owner": config.repository.owner,
        "default_branch": config.repository.default_branch,
        "repositories": repo_names,
        "roles": roles,
        "labels": labels,
        "ci_workflow_sha256": sha256_hex(CI_WORKFLOW.as_bytes()),
        "ci_workflow_path": WORKFLOW_PATH,
        "admin_user": ADMIN_USER,
    })
}

fn block_on_fixture<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime builds")
        .block_on(future)
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_repo_names_sorts_deduplicates_and_defaults() {
        assert_eq!(
            normalize_repo_names(&["b".into(), "a".into(), "b".into()]),
            vec!["a".to_string(), "b".to_string()]
        );
        assert!(!normalize_repo_names(&[]).is_empty());
    }

    #[test]
    fn state_description_mentions_repos_roles_labels_and_ci() {
        let json = reference_delivery_state_description(&["service-a".into()]);
        assert_eq!(json["kind"], "temper-reference-delivery-provisioned-world");
        assert_eq!(json["repositories"][0], "service-a");
        assert!(json["roles"]
            .as_array()
            .is_some_and(|roles| !roles.is_empty()));
        assert!(json["labels"]
            .as_array()
            .is_some_and(|labels| !labels.is_empty()));
        assert_eq!(json["ci_workflow_path"], WORKFLOW_PATH);
    }
}
