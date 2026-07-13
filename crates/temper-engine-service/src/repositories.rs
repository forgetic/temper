// SPDX-License-Identifier: MPL-2.0

//! Repository resolution, label bootstrap, and role feed target helpers.

use temper_engine::{RepositorySet, RepositoryTarget, RoleFeedMode, RoleFeedTarget};
use temper_forge::{Forge, RepositoryPath, UpsertLabel};
use temper_workflow::{CompiledWorkflow, RoleId};

/// Resolves each configured `owner/name` to a live repository (id + path),
/// failing if any is missing on the forge.
pub async fn resolve_repositories<F: Forge + ?Sized>(
    forge: &F,
    repos: &[RepositoryPath],
) -> Result<RepositorySet, String> {
    let mut resolved = Vec::with_capacity(repos.len());
    for path in repos {
        let repository = forge
            .get_repository_by_path(path)
            .await
            .map_err(|error| format!("repository {} lookup failed: {error}", repo_label(path)))?
            .ok_or_else(|| format!("repository {} not found", repo_label(path)))?;
        resolved.push(RepositoryTarget::new(
            repository.id,
            RepositoryPath::new(repository.owner, repository.name),
        ));
    }
    Ok(RepositorySet::new(resolved))
}

/// Upserts every workflow label on every repository (idempotent), so the
/// mechanical backstop's label transitions have somewhere to land.
pub async fn ensure_workflow_labels<F: Forge + ?Sized>(
    forge: &F,
    repositories: &RepositorySet,
    compiled: &CompiledWorkflow,
) -> Result<(), String> {
    for repository in repositories.repositories() {
        for label in compiled.labels().labels() {
            forge
                .upsert_label(
                    &repository.id,
                    UpsertLabel {
                        name: label.id.to_string(),
                        color: Some("#ededed".to_string()),
                        description: None,
                    },
                )
                .await
                .map_err(|error| {
                    format!(
                        "repository {} label {} upsert failed: {error}",
                        repository.display_path(),
                        label.id
                    )
                })?;
        }
    }
    Ok(())
}

/// The cross-product of configured repository routes and roles in the given
/// feed mode.
pub fn role_feed_targets(
    repos: &RepositorySet,
    roles: &[RoleId],
    mode: RoleFeedMode,
) -> Vec<RoleFeedTarget> {
    let mut targets = Vec::with_capacity(repos.repositories().len() * roles.len());
    for repo in repos.repositories() {
        for role in roles {
            targets.push(RoleFeedTarget {
                repo: repo.id.clone(),
                path: repo.path.clone(),
                role: role.clone(),
                mode,
            });
        }
    }
    targets
}

fn repo_label(path: &RepositoryPath) -> String {
    format!("{}/{}", path.owner, path.name)
}
