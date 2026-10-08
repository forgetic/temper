//! Checkout items (jig's domain/connectors.md, section 10). The connector
//! selects starts and write destinations from its adopted and procedure facts.
//! Items are returned whole with their byte size; no copy is retained here.
use crate::{Domain, Role};
use alloc::boxed::Box;
use skein_lib::List;
use temper_engine_domain_forge_change as change;
use temper_engine_domain_forge_client::api;

/// An immutable checkout start, or a branch the worker resolves.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Start {
    /// The project's landing branch.
    Base(Box<[u8]>),
    /// A change's current branch.
    Branch(Box<[u8]>),
    /// The task's saved work takes precedence over its procedure's start.
    Saved(Box<[u8]>),
    /// Resolve this branch against the named base commit.
    Merge { branch: Box<[u8]>, base: api::Commit },
}
/// Root-translated identities and branch names for one repository.
#[derive(Debug)]
pub struct Seed {
    pub task: u64,
    pub parent: Option<u64>,
    pub repository: api::Repository,
    pub run_branch: Box<[u8]>,
    pub saved_branch: Option<Box<[u8]>>,
}
/// A sized workspace item ready to hand to the worker.
#[derive(Debug)]
pub struct Item {
    pub provider: api::Repository,
    pub name: Box<[u8]>,
    pub host: Box<[u8]>,
    pub owner: Box<[u8]>,
    pub start: Start,
    pub push: Option<Box<[u8]>>,
    pub holder: Option<u64>,
    pub bytes: u64,
}
impl Domain {
    /// The project's context repositories are added to every checkout read-only.
    #[must_use]
    pub fn context_repositories(&self, project: u32, limit: u32) -> Option<Box<[api::Repository]>> {
        let mut selected = List::with_capacity(limit);
        for (_, repository) in &self.repositories {
            if repository.project == project && repository.role == Role::Context {
                selected.push(repository.provider).ok()?;
            }
        }
        Some(selected.into_boxed())
    }
    /// Select one checkout item from the task's saved and procedure state.
    #[must_use]
    pub fn workspace_item(&self, seed: Seed) -> Option<Item> {
        let repository = self.repository(seed.repository)?;
        let inherited = match seed.parent {
            Some(parent) => match self.change(parent) {
                Some(row) if row.repository == seed.repository => match row.delegate {
                    Some((child, kind)) if child == seed.task => Some((row, kind)),
                    Some(_) | None => None,
                },
                Some(_) | None => None,
            },
            None => None,
        };
        let (mut start, mut push, holder) = match inherited {
            Some((row, kind)) => match kind {
                change::Delegate::Produce => {
                    let mut existing = false;
                    for (name, _) in &self.heads {
                        if name.forge == seed.repository.forge
                            && name.repository == seed.repository.repository
                            && branch_matches(name, &row.branch)
                        {
                            existing = true;
                        }
                    }
                    (
                        if existing { Start::Branch(row.branch.clone()) } else { Start::Base(row.base.clone()) },
                        Some(row.branch.clone()),
                        Some(row.task),
                    )
                }
                change::Delegate::Repair(_) => {
                    (Start::Branch(row.branch.clone()), Some(row.branch.clone()), Some(row.task))
                }
                change::Delegate::Resolve { base } => {
                    (Start::Merge { branch: row.branch.clone(), base }, Some(row.branch.clone()), Some(row.task))
                }
                change::Delegate::Gate { .. } => (Start::Branch(row.branch.clone()), None, None),
            },
            None => (Start::Base(repository.settings.default_branch.clone()), Some(seed.run_branch), None),
        };
        if let Some(saved) = seed.saved_branch {
            start = Start::Saved(saved);
        }
        if repository.role == Role::Context || !repository.kinds.push {
            push = None;
        }
        let mut bytes = u64::try_from(size_of::<Item>())
            .ok()?
            .checked_add(u64::try_from(repository.name.len()).ok()?)?
            .checked_add(u64::try_from(repository.host.len()).ok()?)?
            .checked_add(u64::try_from(repository.owner.len()).ok()?)?;
        let branch = match &start {
            Start::Base(branch) | Start::Branch(branch) | Start::Saved(branch) | Start::Merge { branch, .. } => branch,
        };
        bytes = bytes.checked_add(u64::try_from(branch.len()).ok()?)?;
        if let Some(push) = &push {
            bytes = bytes.checked_add(u64::try_from(push.len()).ok()?)?;
        }
        Some(Item {
            provider: repository.provider,
            name: repository.name.clone(),
            host: repository.host.clone(),
            owner: repository.owner.clone(),
            start,
            push,
            holder,
            bytes,
        })
    }
}

pub(crate) fn branch_matches(name: &crate::Name, branch: &[u8]) -> bool {
    let parts = match &name.what {
        crate::What::Branch(parts) => parts,
        crate::What::Repository | crate::What::Pull(_) | crate::What::Issue(_) => return false,
    };
    let mut at = 0_usize;
    for (index, part) in parts.iter().enumerate() {
        if index != 0 {
            if branch.get(at) != Some(&b'/') {
                return false;
            }
            let Some(next) = at.checked_add(1) else { return false };
            at = next;
        }
        let Some(end) = at.checked_add(part.len()) else { return false };
        if branch.get(at..end) != Some(part.as_ref()) {
            return false;
        }
        at = end;
    }
    at == branch.len()
}

/// Compare the prefix without allocating a joined branch name.
pub(crate) fn branch_starts_with(name: &crate::Name, prefix: &[u8]) -> bool {
    let parts = match &name.what {
        crate::What::Branch(parts) => parts,
        crate::What::Repository | crate::What::Pull(_) | crate::What::Issue(_) => return false,
    };
    let mut at = 0_usize;
    for (index, part) in parts.iter().enumerate() {
        if index != 0 {
            if at == prefix.len() {
                return true;
            }
            if prefix.get(at) != Some(&b'/') {
                return false;
            }
            at = at.checked_add(1).expect("prefix offset bounded by prefix length");
        }
        for byte in part {
            if at == prefix.len() {
                return true;
            }
            if prefix.get(at) != Some(byte) {
                return false;
            }
            at = at.checked_add(1).expect("prefix offset bounded by prefix length");
        }
    }
    at == prefix.len()
}
