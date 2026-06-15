//! Deterministic repository target set and hint-ordering helpers.

use std::collections::BTreeSet;
use temper_forge_model::{ChangeHint, Forge, ForgeError, RepositoryId, RepositoryPath};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepositoryTarget {
    pub id: RepositoryId,
    pub path: RepositoryPath,
}

impl RepositoryTarget {
    pub fn new(id: RepositoryId, path: RepositoryPath) -> Self {
        Self { id, path }
    }

    pub fn display_path(&self) -> String {
        format!("{}/{}", self.path.owner, self.path.name)
    }

    fn order_key(&self) -> (&str, &str, &RepositoryId) {
        (&self.path.owner, &self.path.name, &self.id)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RepositorySet {
    repositories: Vec<RepositoryTarget>,
}

impl RepositorySet {
    pub fn new(mut repositories: Vec<RepositoryTarget>) -> Self {
        repositories.sort_by(|left, right| left.order_key().cmp(&right.order_key()));
        let mut seen = BTreeSet::new();
        repositories.retain(|repo| seen.insert(repo.id.clone()));
        Self { repositories }
    }

    pub async fn resolve<F, I>(forge: &F, ids: I) -> Result<Self, ForgeError>
    where
        F: Forge + ?Sized,
        I: IntoIterator<Item = RepositoryId>,
    {
        let mut targets = Vec::new();
        for id in ids {
            let repo = forge
                .get_repository(&id)
                .await?
                .ok_or_else(|| ForgeError::NotFound(format!("repository {id}")))?;
            targets.push(RepositoryTarget::new(
                repo.id,
                RepositoryPath::new(repo.owner, repo.name),
            ));
        }
        Ok(Self::new(targets))
    }

    pub fn repositories(&self) -> &[RepositoryTarget] {
        &self.repositories
    }

    pub fn matching_hints<'a>(&'a self, hints: &[ChangeHint]) -> Vec<&'a RepositoryTarget> {
        let hinted = hinted_paths(hints);
        self.repositories
            .iter()
            .filter(|repo| hinted.contains(&path_key(&repo.path)))
            .collect()
    }

    pub fn hinted_order<'a>(&'a self, hints: &[ChangeHint]) -> Vec<&'a RepositoryTarget> {
        let hinted = hinted_paths(hints);
        let mut ordered = Vec::with_capacity(self.repositories.len());
        for repo in &self.repositories {
            if hinted.contains(&path_key(&repo.path)) {
                ordered.push(repo);
            }
        }
        for repo in &self.repositories {
            if !hinted.contains(&path_key(&repo.path)) {
                ordered.push(repo);
            }
        }
        ordered
    }

    pub(super) fn iter_refs(&self) -> Vec<&RepositoryTarget> {
        self.repositories.iter().collect()
    }
}

pub(super) fn hinted_paths(hints: &[ChangeHint]) -> BTreeSet<(String, String)> {
    hints.iter().map(|hint| path_key(&hint.repo)).collect()
}

pub(super) fn path_key(path: &RepositoryPath) -> (String, String) {
    (path.owner.clone(), path.name.clone())
}
