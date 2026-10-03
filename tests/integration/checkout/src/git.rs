//! A working tree's git, beside the fake file system: the operations a
//! protocol layer would run as git invocations, applied to working trees that
//! are directories of a [`Checkout`]. It shares no types with the domain: a
//! world translates between them.
//!
//! The remote side is the forge's. git reaches it as real git does, across
//! the network: through a typed transport, [`Remote`], that a world routes to
//! a fake forge (testing-pyramid.md, 4.3), carrying git's calls (where a
//! repository's branches are, a fetch, a push, a branch created for a base)
//! and their answers. Commits are named in the forge's one store, by a
//! count, so a seed replays to the same names: a working tree commits into
//! it, and finds there the parents and trees of the commits a call brought.
//!
//! A working tree is a directory holding the repository's files and a git
//! directory, `.git`. Checking out replaces every file beneath it but git
//! directories with a commit's tree; committing snapshots the files beneath
//! it, less git directories ([`crate::in_git`]). A working tree has the
//! commits it cloned, fetched or committed, each a file in its git directory
//! (`.git/objects/COMMIT`), so that what removes the directory removes them
//! too. Checking out, committing or pushing a commit the working tree does
//! not have fails, as it would in git.

use std::collections::{BTreeMap, BTreeSet};

use crate::{Checkout, in_git};

/// Files by their path from a working tree's root, with their content.
pub type Tree = BTreeMap<Vec<u8>, Vec<u8>>;

/// The forge as git meets it, by a repository's remote, the forge's address
/// for it: the calls the transport carries, each answered at once, and the
/// one store the commits they bring are named in.
pub trait Remote {
    /// Where each branch of the repository at `remote` is: what a clone
    /// asks first.
    fn heads(&mut self, remote: &[u8]) -> Result<Vec<u64>, Fault>;

    /// Where `want` is in the repository at `remote`.
    fn fetch(&mut self, remote: &[u8], want: Want<'_>) -> Result<u64, Fault>;

    /// Creates `branch` of the repository at `remote` at `commit`, which it
    /// has, only if it is nowhere.
    fn create(&mut self, remote: &[u8], branch: &[u8], commit: u64) -> Result<Created, Fault>;

    /// Moves `branch` of the repository at `remote` to `commit` as a
    /// fast-forward, or creates it.
    fn push(&mut self, remote: &[u8], branch: &[u8], commit: u64) -> Result<Pushed, Fault>;

    /// The parent of `commit`, unless it is a repository's first.
    fn parent(&self, commit: u64) -> Option<u64>;

    /// The files of `commit`.
    fn tree(&self, commit: u64) -> Tree;

    /// Names a commit of `tree` on `parent` in the store, or `None` if the
    /// tree is `parent`'s.
    fn store(&mut self, parent: u64, tree: Tree) -> Option<u64>;
}

/// Why a call to the forge failed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Fault {
    /// The forge has no such repository, branch or commit.
    Missing(What),
    /// The repository refuses what is pushed to it.
    Refused,
    /// The repository cannot be reached.
    Unreachable,
}

/// What the forge does not have.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum What {
    Repository,
    Branch,
    Commit,
}

/// What a fetch asks for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Want<'a> {
    Branch(&'a [u8]),
    Commit(u64),
    Default,
}

/// How a creation went: the branch was made, or it existed and was left
/// where it is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Created {
    Created,
    Exists,
}

/// How a push went: the branch is at the commit, or it is not an ancestor of
/// it and was left where it is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pushed {
    Pushed,
    Rejected,
}

/// A working tree does not have the commit: it was never cloned, fetched or
/// committed there.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct NotFetched;

/// Clones the repository at `remote` into the directory `at`, which does not
/// exist: a git directory with every commit its branches reach, and no tree
/// checked out.
pub fn clone_repository(
    forge: &mut impl Remote,
    checkout: &mut Checkout,
    remote: &[u8],
    at: &[u8],
) -> Result<(), Fault> {
    let heads = forge.heads(remote)?;
    let mut reached = BTreeSet::new();
    for head in heads {
        let mut next = Some(head);
        while let Some(commit) = next
            && reached.insert(commit)
        {
            next = forge.parent(commit);
        }
    }
    assert!(!checkout.exists(at), "a clone goes where nothing is");
    checkout.mkdir(&[at, b"/.git"].concat());
    for commit in reached {
        checkout.write(&object(at, commit), b"");
    }
    Ok(())
}

/// Fetches `want` from the repository at `remote` into the working tree at
/// `at`: the commit it names, and those before it the working tree lacks.
pub fn fetch(
    forge: &mut impl Remote,
    checkout: &mut Checkout,
    remote: &[u8],
    at: &[u8],
    want: Want<'_>,
) -> Result<u64, Fault> {
    let fetched = forge.fetch(remote, want)?;
    let mut next = Some(fetched);
    while let Some(commit) = next {
        if checkout.exists(&object(at, commit)) {
            break;
        }
        checkout.write(&object(at, commit), b"");
        next = forge.parent(commit);
    }
    Ok(fetched)
}

/// Creates `branch` of the repository at `remote` at `commit`, only if it
/// does not exist.
pub fn create(forge: &mut impl Remote, remote: &[u8], branch: &[u8], commit: u64) -> Result<Created, Fault> {
    forge.create(remote, branch, commit)
}

/// Makes the working tree at `at` exactly `commit`'s tree, leaving its git
/// directories, if it has the commit.
pub fn check_out(forge: &impl Remote, checkout: &mut Checkout, at: &[u8], commit: u64) -> Result<(), NotFetched> {
    if !checkout.exists(&object(at, commit)) {
        return Err(NotFetched);
    }
    checkout.replace_tree(at, &forge.tree(commit));
    Ok(())
}

/// Commits the working tree at `at` on `parent`, if it has that commit: the
/// files beneath it, less its git directories. Returns the commit, or `None`
/// if the tree is `parent`'s.
pub fn commit(
    forge: &mut impl Remote,
    checkout: &mut Checkout,
    at: &[u8],
    parent: u64,
) -> Result<Option<u64>, NotFetched> {
    if !checkout.exists(&object(at, parent)) {
        return Err(NotFetched);
    }
    let mut tree = checkout.tree(at);
    tree.retain(|path, _| !in_git(path));
    let Some(commit) = forge.store(parent, tree) else {
        return Ok(None);
    };
    checkout.write(&object(at, commit), b"");
    Ok(Some(commit))
}

/// Pushes `commit`, which the working tree at `at` has, to `branch` of the
/// repository at `remote`, as a fast-forward: a branch that is nowhere is
/// created, and one that is not an ancestor of `commit` is left where it is.
pub fn push(
    forge: &mut impl Remote,
    checkout: &Checkout,
    remote: &[u8],
    at: &[u8],
    commit: u64,
    branch: &[u8],
) -> Result<Pushed, Fault> {
    assert!(checkout.exists(&object(at, commit)), "a push is of a commit the working tree has");
    forge.push(remote, branch, commit)
}

/// Where the working tree at `at` keeps `commit`.
fn object(at: &[u8], commit: u64) -> Vec<u8> {
    [at, format!("/.git/objects/{commit}").as_bytes()].concat()
}
