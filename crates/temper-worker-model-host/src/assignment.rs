//! An assignment's bounds, checked at the entrance (worker-model.md, 4.1):
//! the host interprets none of it, but it holds the charter and the snapshot
//! until the agent starts, and its parent relies on the workspace fitting the
//! limits.

use crate::boundary::{Access, Assignment, Invalid, Repository, Start};
use crate::limits::Limits;

/// Whether `assignment` fits `limits`, and what about it does not.
pub(crate) fn check(assignment: &Assignment, limits: &Limits) -> Result<(), Invalid> {
    if len(&assignment.charter) > limits.charter_bytes {
        return Err(Invalid::Charter);
    }
    if let Some(snapshot) = &assignment.snapshot
        && len(snapshot) > limits.snapshot_bytes
    {
        return Err(Invalid::Snapshot);
    }
    if let Some(branch) = &assignment.save {
        name(branch, limits)?;
    }
    let workspace = &assignment.workspace;
    name(&workspace.key, limits)?;
    let count = u64::try_from(workspace.repositories.len()).expect("a length fits in a u64");
    if count > u64::from(limits.repositories) {
        return Err(Invalid::Repositories);
    }
    for repository in &workspace.repositories {
        self::repository(repository, limits)?;
    }
    // Bounded: the repositories are within the limits, checked above.
    for (index, repository) in workspace.repositories.iter().enumerate() {
        for other in workspace.repositories.iter().skip(index.saturating_add(1)) {
            if other.name == repository.name {
                return Err(Invalid::Duplicate);
            }
        }
    }
    Ok(())
}

fn repository(repository: &Repository, limits: &Limits) -> Result<(), Invalid> {
    name(&repository.name, limits)?;
    match &repository.start {
        Start::Base { branch } | Start::Branch { branch } | Start::Saved { branch } => name(branch, limits)?,
        Start::Commit { commit } => name(commit, limits)?,
    }
    match &repository.access {
        Access::ReadOnly => Ok(()),
        Access::Writable { push, identity } => {
            name(push, limits)?;
            name(identity, limits)
        }
    }
}

/// A name is at least a byte, and at most the limit.
fn name(bytes: &[u8], limits: &Limits) -> Result<(), Invalid> {
    if bytes.is_empty() || len(bytes) > u64::from(limits.name_bytes) {
        return Err(Invalid::Name);
    }
    Ok(())
}

/// The length of `bytes`.
pub(crate) fn len(bytes: &[u8]) -> u64 {
    u64::try_from(bytes.len()).expect("a length fits in a u64")
}
