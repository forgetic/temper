//! What a world does to the forge from outside its API (testing.md,
//! 5.1): it sets up repositories and their users, makes a repository
//! unreachable or refusing, commits as a working tree's git does, and has
//! another party advance a branch.
//!
//! Setting up is not observed and starts no CI; what another party does is
//! observed, heard and checked as any change is.

use alloc::boxed::Box;

use skein_lib::Env;
use skein_lib::bytes::copy_of;

use crate::api::{Error, File, Permission, Setup, What};
use crate::domain::{Config, Domain};
use crate::git::{self, Object};
use crate::store::{Repository, fit_names, fits};

/// Adds the repository `setup` describes, its default branch at a first
/// commit of its tree, and returns that commit.
pub fn repository(domain: &mut Domain, config: &Config, setup: Setup) -> u64 {
    let limits = &config.limits;
    let Setup { name, default, tree, labels, checks, protection, hooked } = setup;
    assert!(!domain.names.contains_key(&*name), "a repository is added once");
    fits(&name, limits.name_bytes).expect("a repository's name within the limits");
    fits(&default, limits.name_bytes).expect("a branch name within the limits");
    fit_names(&labels, limits.labels, limits).expect("labels within the limits");
    fit_names(&checks.contexts, limits.contexts, limits).expect("contexts within the limits");
    assert!(checks.latency_min <= checks.latency_max, "CI's latency is a range");
    if let Some(cue) = &checks.cue {
        fits(&cue.path, limits.name_bytes).expect("a path within the limits");
        fits(&cue.green, limits.content_bytes).expect("content within the limits");
    }
    if let Some(protection) = &protection {
        fits(&protection.branch, limits.name_bytes).expect("a branch name within the limits");
        fit_names(&protection.contexts, limits.contexts, limits).expect("contexts within the limits");
    }
    let tree = git::tree(limits, tree).expect("a first tree within the limits");
    let first = git::store(domain, Object { parent: None, merge_parent: None, tree, message: Box::new([]) })
        .expect("room for a first commit");
    let mut repository = Repository::new(limits, copy_of(&name), default, first, checks, protection, hooked);
    for label in labels {
        repository.labels.insert(label).expect("a repository's labels are within the limits");
    }
    let id = domain.repositories.insert(repository).expect("a repository's room");
    domain.names.insert(name, id).expect("as many names as repositories");
    first
}

/// Gives `user` the permission `permission` on `repository`.
pub fn grant(domain: &mut Domain, repository: &[u8], user: u64, permission: Permission) {
    let id = domain.id(repository);
    let repository = domain.repositories.get_mut(id).expect("a repository of the forge");
    repository.permissions.insert(user, permission).expect("a repository's users are within the limits");
}

/// Set repository metadata in a world's initial setup. Its default must
/// name an existing branch; these booleans model provider merge settings.
pub fn settings(domain: &mut Domain, repository: &[u8], settings: crate::api::Settings) {
    let id = domain.id(repository);
    let repository = domain.repositories.get_mut(id).expect("a repository of the forge");
    assert!(repository.branches.contains_key(&*settings.default), "the default branch exists");
    repository.default = settings.default;
    repository.styles =
        crate::store::Styles { merge: settings.merge, rebase: settings.rebase, squash: settings.squash };
}

/// Makes `repository` reachable by git, or not.
pub fn set_reachable(domain: &mut Domain, repository: &[u8], reachable: bool) {
    let id = domain.id(repository);
    domain.repositories.get_mut(id).expect("a repository of the forge").reachable = reachable;
}

/// Makes `repository` refuse what is pushed to it, branches created included,
/// or not.
pub fn set_refusing(domain: &mut Domain, repository: &[u8], refusing: bool) {
    let id = domain.id(repository);
    domain.repositories.get_mut(id).expect("a repository of the forge").refusing = refusing;
}

/// What a working tree's git does when it commits: names a commit of `tree`
/// on `parent` in the forge's one store, where no repository has it until it
/// is pushed. Returns `None` if the tree is `parent`'s, and refuses a tree
/// past the limits, or a store that is full. The opaque message fits the
/// fixture's commit message allowance.
pub fn commit(
    domain: &mut Domain,
    config: &Config,
    parent: u64,
    tree: Box<[File]>,
    message: &[u8],
) -> Result<Option<u64>, Error> {
    let tree = git::tree(&config.limits, tree)?;
    let Some(object) = domain.commits.get(&parent) else {
        return Err(Error::Missing(What::Commit));
    };
    if git::same(&object.tree, &tree) {
        return Ok(None);
    }
    fits(message, message_limit(&config.limits)?)?;
    let commit =
        git::store(domain, Object { parent: Some(parent), merge_parent: None, tree, message: copy_of(message) })?;
    Ok(Some(commit))
}

/// A worker commits a resolved merge, with both fetched parents. Unlike an
/// ordinary commit, an unchanged tree still records the merge ancestry and
/// the opaque message.
pub fn merge_commit(
    domain: &mut Domain,
    config: &Config,
    parent: u64,
    merge_parent: u64,
    files: Box<[File]>,
    message: &[u8],
) -> Result<u64, Error> {
    if !domain.commits.contains_key(&parent) || !domain.commits.contains_key(&merge_parent) {
        return Err(Error::Missing(What::Commit));
    }
    let tree = git::tree(&config.limits, files)?;
    fits(message, message_limit(&config.limits)?)?;
    git::store(
        domain,
        Object { parent: Some(parent), merge_parent: Some(merge_parent), tree, message: copy_of(message) },
    )
}

fn message_limit(limits: &crate::limits::Limits) -> Result<u32, Error> {
    let bytes = limits.title_bytes.checked_add(limits.body_bytes).ok_or(Error::TooLarge)?;
    bytes.checked_add(2).ok_or(Error::TooLarge)
}

/// Another party commits on `branch` of `repository`, writing `path` with
/// `content`, and moves the branch to it, as a push of its own would, past
/// any protection, as the user `by`. Returns the commit, or refuses what is
/// past the limits, as a store or a tree that is full.
pub fn advance(
    domain: &mut Domain,
    env: &Env<Config>,
    repository: &[u8],
    branch: &[u8],
    path: &[u8],
    content: &[u8],
    by: u64,
) -> Result<u64, Error> {
    let limits = &env.limits.limits;
    fits(path, limits.name_bytes)?;
    fits(content, limits.content_bytes)?;
    let id = domain.id(repository);
    let tip = *domain.repositories.get(id).expect("a repository of the forge").branches.get(branch).expect("a branch");
    let mut tree = git::copy_tree(limits, &domain.commits.get(&tip).expect("a commit of the store").tree);
    if !tree.contains_key(path) && tree.len() >= tree.capacity() {
        return Err(Error::TooLarge);
    }
    tree.insert(copy_of(path), copy_of(content)).expect("checked for room above");
    let commit = git::store(domain, Object { parent: Some(tip), merge_parent: None, tree, message: Box::new([]) })?;
    let repository = domain.repositories.get_mut(id).expect("a repository of the forge");
    repository.has.insert(commit).expect("a repository has room for every commit");
    repository.branches.insert(copy_of(branch), commit).expect("the branch is there");
    git::moved(domain, env, id, branch, Some(tip), commit, by);
    Ok(commit)
}
