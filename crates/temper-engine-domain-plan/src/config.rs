//! The deployment's configuration, as plans see it: its repositories and the
//! branches changes may land into, and the templates a step may name.

use alloc::boxed::Box;

use crate::plan::Repository;

/// The deployment's configuration, all the plan keeps between calls
/// (engine-domain.md, section 3).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Config {
    /// The deployment's repositories, by index.
    pub repositories: Box<[Repo]>,
    /// Templates a step may name.
    pub templates: Box<[Template]>,
}

/// One of the deployment's repositories, as plans see it.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Repo {
    /// The branches changes may land into.
    pub bases: Box<[Box<[u8]>]>,
}

/// A step charter that has worked, which a step may name: its brief carries
/// the template's guidance. It gates nothing (5.2).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Template {
    pub name: Box<[u8]>,
    pub guidance: Box<[u8]>,
}

impl Config {
    /// The index of the template named `name`.
    #[must_use]
    pub fn template(&self, name: &[u8]) -> Option<u32> {
        for (index, template) in self.templates.iter().enumerate() {
            if *template.name == *name {
                return u32::try_from(index).ok();
            }
        }
        None
    }

    /// The deployment's repository `repository`, if it has one by that index.
    #[must_use]
    pub fn repo(&self, repository: Repository) -> Option<&Repo> {
        self.repositories.get(usize::try_from(repository.0).ok()?)
    }

    /// Whether changes may land into `base` of `repository`.
    #[must_use]
    pub fn is_base(&self, repository: Repository, base: &[u8]) -> bool {
        match self.repo(repository) {
            Some(repo) => names(&repo.bases, base),
            None => false,
        }
    }
}

/// Whether `labels` has `label` among them.
pub(crate) fn names(labels: &[Box<[u8]>], label: &[u8]) -> bool {
    for candidate in labels {
        if **candidate == *label {
            return true;
        }
    }
    false
}
