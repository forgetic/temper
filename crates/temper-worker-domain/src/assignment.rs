//! An assignment's bounds, checked at the entrance (worker-domain.md, 4.1):
//! the host interprets none of it but the repositories' names, each the
//! directory a repository sits in, so one path component unique within its
//! workspace. It holds the charter and conversation until the agent starts,
//! and its parent relies on the workspace fitting the limits.

use crate::Limits;
use crate::wire::{Access, Assignment, Invalid, Repository, Start};

/// Whether `assignment` fits `limits`, and what about it does not.
pub(crate) fn check(assignment: &Assignment, limits: &Limits, next: bool) -> Result<(), Invalid> {
    if len(&assignment.charter) > limits.host.charter_bytes {
        return Err(Invalid::Charter);
    }
    if let Some(branch) = &assignment.save {
        name(branch, limits)?;
    }
    if u64::try_from(assignment.grants.len()).expect("a length fits") > u64::from(limits.host.accounts) {
        return Err(Invalid::Grants);
    }
    for (index, grant) in assignment.grants.iter().enumerate() {
        for other in assignment.grants.iter().skip(index.saturating_add(1)) {
            if grant.account == other.account {
                return Err(Invalid::Grants);
            }
        }
    }
    let workspace = &assignment.workspace;
    let count = u64::try_from(workspace.repositories.len()).expect("a length fits in a u64");
    if count > u64::from(limits.checkout.repositories) {
        return Err(Invalid::Repositories);
    }
    if count != 0 {
        name(&workspace.key, limits)?;
    }
    for repository in &workspace.repositories {
        self::repository(repository, limits, next)?;
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

fn repository(repository: &Repository, limits: &Limits, next: bool) -> Result<(), Invalid> {
    name(&repository.name, limits)?;
    component(&repository.name)?;
    name(&repository.remote, limits)?;
    match &repository.start {
        Start::Base { branch } | Start::Branch { branch } | Start::Saved { branch } => {
            name(branch, limits)?;
        }
        Start::Commit { .. } => {}
        Start::Merge { branch, .. } => {
            if !next || limits.checkout.conflicts == 0 {
                return Err(Invalid::Version);
            }
            name(branch, limits)?;
        }
    }
    match &repository.access {
        Access::ReadOnly => Ok(()),
        Access::WritableV2 { push, .. } => {
            if !next {
                return Err(Invalid::Version);
            }
            name(push, limits)
        }
    }
}

/// A name is at least a byte, and at most the limit.
fn name(bytes: &[u8], limits: &Limits) -> Result<(), Invalid> {
    if bytes.is_empty() || len(bytes) > u64::from(limits.checkout.name_bytes) {
        return Err(Invalid::Name);
    }
    Ok(())
}

/// A repository's name is the directory it sits in: one path component, which
/// names nothing else.
fn component(name: &[u8]) -> Result<(), Invalid> {
    let special = name == b"." || name == b".." || name.eq_ignore_ascii_case(b".git");
    if special || name.contains(&b'/') || name.contains(&0) {
        return Err(Invalid::Name);
    }
    Ok(())
}

/// The length of `bytes`.
pub(crate) fn len(bytes: &[u8]) -> u64 {
    u64::try_from(bytes.len()).expect("a length fits in a u64")
}
