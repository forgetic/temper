//! The forge's git, the remote side: each repository's branches and the
//! commits it has, over one store where a commit is a parent and a tree of
//! paths to contents, named by a count so a seed replays to the same names.
//!
//! It keeps git's rules: a push moves a branch only by a fast-forward, or
//! creates it, and is otherwise rejected, leaving the branch where it is; a
//! branch made for a base is only made where none is, never moved. A world
//! scripts the rest: a repository that cannot be reached, one that refuses
//! what is pushed to it, and another party advancing a branch, after which a
//! push from where the branch was is rejected.
//!
//! Every move of a branch is observed, starts CI on its new head, and moves
//! the head of each open pull request that follows the branch.

use alloc::boxed::Box;

use temper_lib::bytes::copy_of;
use temper_lib::{Env, Id, List, Map, Set};

use crate::api::{Answer, Change, Created, Error, File, Git, Head, Permission, Pushed, Want, What};
use crate::limits::Limits;
use crate::model::{self, Config, Model};
use crate::observe::Observation;
use crate::store::{Repository, fits};
use crate::{ci, issues};

/// Files by their paths, with their content.
pub type Tree = Map<Box<[u8]>, Box<[u8]>>;

/// A commit: its parent, unless it is a repository's first, and its tree,
/// files by their paths.
#[derive(Debug)]
pub struct Object {
    pub parent: Option<u64>,
    pub tree: Tree,
}

/// Names `object` in the store, or refuses it when the store is full.
pub(crate) fn store(model: &mut Model, object: Object) -> Result<u64, Error> {
    if model.commits.len() >= model.commits.capacity() {
        return Err(Error::Full);
    }
    let commit = model.made.checked_add(1).expect("names do not run out");
    model.commits.insert(commit, object).expect("checked for room above");
    model.made = commit;
    Ok(commit)
}

/// The tree `files` describe, or why it is past the limits.
pub(crate) fn tree(limits: &Limits, files: Box<[File]>) -> Result<Tree, Error> {
    let mut tree = Map::with_capacity(limits.files);
    for File { path, content } in files {
        fits(&path, limits.name_bytes)?;
        fits(&content, limits.content_bytes)?;
        if tree.insert(path, content).is_err() {
            return Err(Error::TooLarge);
        }
    }
    Ok(tree)
}

pub(crate) fn copy_tree(limits: &Limits, tree: &Tree) -> Tree {
    let mut copy = Map::with_capacity(limits.files);
    for (path, content) in tree {
        copy.insert(copy_of(path), copy_of(content)).expect("a copy of a tree fits as it did");
    }
    copy
}

/// Whether two trees hold the same files.
pub(crate) fn same(a: &Tree, b: &Tree) -> bool {
    a.len() == b.len() && a.iter().eq(b.iter())
}

/// Whether `ancestor` is `commit` or one of its ancestors. No history is
/// longer than the store.
pub(crate) fn is_ancestor(model: &Model, ancestor: u64, commit: u64) -> bool {
    let mut at = Some(commit);
    for _ in 0..=model.commits.len() {
        let Some(current) = at else {
            return false;
        };
        if current == ancestor {
            return true;
        }
        at = parent(model, current);
    }
    false
}

/// The newest commit both `a` and `b` descend from, if they share one.
pub(crate) fn merge_base(model: &Model, a: u64, b: u64) -> Option<u64> {
    let mut history = Set::with_capacity(model.commits.len());
    let mut at = Some(b);
    for _ in 0..=model.commits.len() {
        let Some(current) = at else {
            break;
        };
        history.insert(current).expect("a history no longer than the store");
        at = parent(model, current);
    }
    let mut at = Some(a);
    for _ in 0..=model.commits.len() {
        let current = at?;
        if history.contains(&current) {
            return Some(current);
        }
        at = parent(model, current);
    }
    None
}

fn parent(model: &Model, commit: u64) -> Option<u64> {
    model.commits.get(&commit).expect("a commit of the store").parent
}

/// What git asks of the repository `id`, as `user`.
pub(crate) fn serve(
    model: &mut Model,
    env: &Env<Config>,
    id: Id<Repository>,
    user: u64,
    git: Git,
) -> Result<Answer, Error> {
    let repository = model.repositories.get(id).expect("a repository of the forge");
    if !repository.reachable {
        return Err(Error::Unreachable);
    }
    match git {
        Git::Clone => {
            repository.require(user, Permission::Read)?;
            let mut branches = List::with_capacity(repository.branches.len());
            for (branch, &commit) in &repository.branches {
                branches.push(Head { branch: copy_of(branch), commit }).expect("a list as long as the branches");
            }
            Ok(Answer::Cloned { default: copy_of(&repository.default), branches: branches.into_boxed() })
        }
        Git::Fetch { want } => {
            repository.require(user, Permission::Read)?;
            let found = match want {
                Want::Branch(branch) => repository.branches.get(&*branch).copied().ok_or(What::Branch),
                Want::Commit(commit) => {
                    if repository.has.contains(&commit) {
                        Ok(commit)
                    } else {
                        Err(What::Commit)
                    }
                }
                Want::Default => repository.branches.get(&*repository.default).copied().ok_or(What::Branch),
            };
            match found {
                Ok(commit) => Ok(Answer::Commit(commit)),
                Err(what) => Err(Error::Missing(what)),
            }
        }
        Git::Push { branch, commit } => push(model, env, id, user, &branch, commit),
        Git::Create { branch, commit } => create(model, env, id, user, &branch, commit),
    }
}

/// Moves `branch` to `commit` as a fast-forward, or creates it.
fn push(
    model: &mut Model,
    env: &Env<Config>,
    id: Id<Repository>,
    user: u64,
    branch: &[u8],
    commit: u64,
) -> Result<Answer, Error> {
    let repository = model.repositories.get(id).expect("a repository of the forge");
    repository.require(user, Permission::Write)?;
    fits(branch, env.limits.limits.name_bytes)?;
    if repository.refusing {
        return Err(Error::Refused);
    }
    if repository.is_protected(branch) {
        return Err(Error::Protected);
    }
    if !model.commits.contains_key(&commit) {
        return Err(Error::Missing(What::Commit));
    }
    let tip = repository.branches.get(branch).copied();
    match tip {
        Some(tip) => {
            if !is_ancestor(model, tip, commit) {
                return Ok(Answer::Pushed(Pushed::Rejected));
            }
            if tip == commit {
                return Ok(Answer::Pushed(Pushed::Pushed));
            }
        }
        None => {
            if repository.branches.len() >= repository.branches.capacity() {
                return Err(Error::Full);
            }
        }
    }
    // The repository takes the commit and those before it it lacks.
    let mut next = Some(commit);
    for _ in 0..=model.commits.len() {
        let Some(pushed) = next else {
            break;
        };
        let repository = model.repositories.get_mut(id).expect("a repository of the forge");
        if !repository.has.insert(pushed).expect("a repository has room for every commit") {
            break;
        }
        next = parent(model, pushed);
    }
    let repository = model.repositories.get_mut(id).expect("a repository of the forge");
    repository.branches.insert(copy_of(branch), commit).expect("checked for room above");
    moved(model, env, id, branch, tip, commit, user);
    Ok(Answer::Pushed(Pushed::Pushed))
}

/// Creates `branch` at `commit`, only where none is.
fn create(
    model: &mut Model,
    env: &Env<Config>,
    id: Id<Repository>,
    user: u64,
    branch: &[u8],
    commit: u64,
) -> Result<Answer, Error> {
    let repository = model.repositories.get_mut(id).expect("a repository of the forge");
    repository.require(user, Permission::Write)?;
    fits(branch, env.limits.limits.name_bytes)?;
    if repository.refusing {
        return Err(Error::Refused);
    }
    if repository.is_protected(branch) {
        return Err(Error::Protected);
    }
    if !repository.has.contains(&commit) {
        return Err(Error::Missing(What::Commit));
    }
    if repository.branches.contains_key(branch) {
        return Ok(Answer::Branch(Created::Exists));
    }
    if repository.branches.insert(copy_of(branch), commit).is_err() {
        return Err(Error::Full);
    }
    moved(model, env, id, branch, None, commit, user);
    Ok(Answer::Branch(Created::Created))
}

/// Deletes `branch`, which is neither the default nor protected, closing
/// the open pull requests whose head or base it is.
pub(crate) fn delete(
    model: &mut Model,
    env: &Env<Config>,
    id: Id<Repository>,
    user: u64,
    branch: &[u8],
) -> Result<Answer, Error> {
    let repository = model.repositories.get_mut(id).expect("a repository of the forge");
    repository.require(user, Permission::Write)?;
    if *repository.default == *branch || repository.is_protected(branch) {
        return Err(Error::Protected);
    }
    let Some(at) = repository.branches.remove(branch) else {
        return Err(Error::Missing(What::Branch));
    };
    let closing = repository.on_branch(branch);
    let observation =
        Observation::Deleted { repository: copy_of(&repository.name), branch: copy_of(branch), at, by: user };
    model::changed(model, env, id, observation, Change::Push, None);
    // As Forgejo does, the open pull requests from or into it close.
    for &number in &closing {
        issues::shut(model, env, id, number, user);
    }
    Ok(Answer::Done)
}

/// `branch` of the repository `id` moved from `from` to `to`, by `by`: the
/// open pull requests that follow it move with it, and CI starts on it.
pub(crate) fn moved(
    model: &mut Model,
    env: &Env<Config>,
    id: Id<Repository>,
    branch: &[u8],
    from: Option<u64>,
    to: u64,
    by: u64,
) {
    let repository = model.repositories.get_mut(id).expect("a repository of the forge");
    let observation =
        Observation::Moved { repository: copy_of(&repository.name), branch: copy_of(branch), from, to, by };
    let following = repository.following(branch);
    for &number in &following {
        let item = repository.items.get_mut(&number).expect("a pull request that follows the branch");
        let pull = item.pull.as_mut().expect("a pull request");
        pull.commit = to;
        repository.touch(number, model::clock(env));
    }
    model::changed(model, env, id, observation, Change::Push, None);
    for &number in &following {
        crate::hooks::notify(model, env, id, Change::Pull, Some(number));
    }
    ci::start(model, env, id, to);
}
