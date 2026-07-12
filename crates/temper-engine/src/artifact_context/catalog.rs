// SPDX-License-Identifier: MPL-2.0

use std::collections::BTreeMap;

use temper_forge::{RepositoryId, RepositoryPath};
use temper_protocol_worker::ArtifactRepository;
use temper_runner::{RepositorySet, RepositoryTarget};

/// Immutable startup catalog of repositories the engine is configured to serve.
///
/// Artifact links never cause repository discovery. Both stable ids and
/// human-facing paths must resolve through this catalog.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ConfiguredRepositoryCatalog {
    by_id: BTreeMap<RepositoryId, ArtifactRepository>,
    by_path: BTreeMap<String, RepositoryId>,
    forge_url: String,
}

impl ConfiguredRepositoryCatalog {
    pub fn new(
        repositories: impl IntoIterator<Item = RepositoryTarget>,
        forge_url: impl Into<String>,
    ) -> Result<Self, String> {
        let mut catalog = Self {
            by_id: BTreeMap::new(),
            by_path: BTreeMap::new(),
            forge_url: forge_url.into().trim_end_matches('/').to_string(),
        };
        for repository in repositories {
            let path = repository.display_path();
            if let Some(existing) = catalog.by_path.insert(path.clone(), repository.id.clone()) {
                if existing != repository.id {
                    return Err(format!("configured repository path `{path}` is duplicated"));
                }
            }
            catalog.by_id.insert(
                repository.id.clone(),
                ArtifactRepository {
                    id: repository.id.to_string(),
                    path,
                },
            );
        }
        Ok(catalog)
    }

    pub fn from_repository_set(
        repositories: &RepositorySet,
        forge_url: impl Into<String>,
    ) -> Result<Self, String> {
        Self::new(repositories.repositories().iter().cloned(), forge_url)
    }

    pub fn single(id: RepositoryId, path: RepositoryPath, forge_url: impl Into<String>) -> Self {
        Self::new([RepositoryTarget::new(id, path)], forge_url)
            .expect("one repository cannot conflict with itself")
    }

    pub fn by_id(&self, id: &RepositoryId) -> Option<&ArtifactRepository> {
        self.by_id.get(id)
    }

    pub fn by_path(&self, path: &str) -> Option<(RepositoryId, &ArtifactRepository)> {
        let id = self.by_path.get(path)?;
        Some((id.clone(), self.by_id.get(id)?))
    }

    pub fn repositories(&self) -> impl Iterator<Item = (RepositoryId, &ArtifactRepository)> {
        self.by_id
            .iter()
            .map(|(id, repository)| (id.clone(), repository))
    }

    pub fn forge_url(&self) -> &str {
        &self.forge_url
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }
}
