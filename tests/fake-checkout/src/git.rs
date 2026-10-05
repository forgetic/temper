//! A working tree's git, beside the fake file system: the operations a
//! protocol layer would run as git invocations, applied to working trees that
//! are directories of a [`Checkout`]. It shares no types with the domain: a
//! world translates between them.
//!
//! The remote side is the forge's. git reaches it as real git does, across
//! the network: through a typed transport, [`Remote`], that a world routes to
//! a fake forge (testing.md, 4.3), carrying git's calls (where a
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
    fn push(&mut self, remote: &[u8], branch: &[u8], commit: u64, expected: Option<u64>) -> Result<Pushed, Fault>;

    /// The parent of `commit`, unless it is a repository's first.
    fn parent(&self, commit: u64) -> Option<u64>;

    /// The second parent of a merge commit.
    fn merge_parent(&self, commit: u64) -> Option<u64>;

    /// The files of `commit`.
    fn tree(&self, commit: u64) -> Tree;

    /// Names a commit of `tree` on `parent` in the store, or `None` if the
    /// tree is `parent`'s.
    fn store(&mut self, parent: u64, merging: Option<u64>, tree: Tree) -> Option<u64>;
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
    let mut pending = heads;
    while let Some(commit) = pending.pop() {
        if !reached.insert(commit) {
            continue;
        }
        pending.extend(forge.parent(commit));
        pending.extend(forge.merge_parent(commit));
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
    let mut pending = vec![fetched];
    let mut reached = BTreeSet::new();
    while let Some(commit) = pending.pop() {
        if !reached.insert(commit) || checkout.exists(&object(at, commit)) {
            continue;
        }
        checkout.write(&object(at, commit), b"");
        pending.extend(forge.parent(commit));
        pending.extend(forge.merge_parent(commit));
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
    checkout.write(&[at, b"/.git/temper-head"].concat(), &commit.to_le_bytes());
    checkout.remove(&[at, b"/.git/temper-conflicts"].concat());
    checkout.remove(&[at, b"/.git/MERGE_HEAD"].concat());
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
    let Some(commit) = forge.store(parent, None, tree) else {
        return Ok(None);
    };
    checkout.write(&object(at, commit), b"");
    checkout.write(&[at, b"/.git/temper-head"].concat(), &commit.to_le_bytes());
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
    forge.push(remote, branch, commit, None)
}

/// A merge's working tree, with the files whose contents need resolution.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Merged {
    pub conflicts: Vec<Vec<u8>>,
}

/// Merge the fetched `theirs` into the freshly checked-out branch. No
/// commit or remote reference is made. Both parents remain in the local
/// object graph until a later explicit merge commit.
pub fn merge(forge: &impl Remote, checkout: &mut Checkout, at: &[u8], theirs: u64) -> Result<Merged, NotFetched> {
    let raw = checkout.content(&[at, b"/.git/temper-head"].concat()).ok_or(NotFetched)?;
    let ours = u64::from_le_bytes(raw.try_into().expect("checkout stores an eight-byte head"));
    if !checkout.exists(&object(at, theirs)) {
        return Err(NotFetched);
    }
    let ours_tree = forge.tree(ours);
    let theirs_tree = forge.tree(theirs);
    let base = common_ancestor(forge, ours, theirs).map_or_else(Tree::new, |commit| forge.tree(commit));
    let paths: BTreeSet<_> = base.keys().chain(ours_tree.keys()).chain(theirs_tree.keys()).cloned().collect();
    let mut tree = Tree::new();
    let mut conflicts = Vec::new();
    for path in paths {
        let old = base.get(&path);
        let left = ours_tree.get(&path);
        let right = theirs_tree.get(&path);
        let content = if left == right || right == old {
            left.cloned()
        } else if left == old {
            right.cloned()
        } else if let (Some(old), Some(left), Some(right)) = (old, left, right) {
            if let Some(merged) = merge_lines(old, left, right) {
                Some(merged)
            } else {
                conflicts.push(path.clone());
                Some(markers(left, right))
            }
        } else {
            conflicts.push(path.clone());
            Some(markers(left.map_or(&[], Vec::as_slice), right.map_or(&[], Vec::as_slice)))
        };
        if let Some(content) = content {
            tree.insert(path, content);
        }
    }
    checkout.replace_tree(at, &tree);
    checkout.write(&[at, b"/.git/MERGE_HEAD"].concat(), &theirs.to_le_bytes());
    checkout.remove(&[at, b"/.git/temper-conflicts"].concat());
    for (index, path) in conflicts.iter().enumerate() {
        checkout.write(&[at, format!("/.git/temper-conflicts/{index}").as_bytes()].concat(), path);
    }
    Ok(Merged { conflicts })
}

/// Why an explicit merge commit was refused without creating an object.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum CommitFailure {
    NotFetched,
    Unresolved { files: Vec<Vec<u8>> },
}

/// Commit the resolved tree with the supplied second parent, including an
/// unchanged tree. Only paths originally conflicted are checked for markers.
pub fn commit_merging(
    forge: &mut impl Remote,
    checkout: &mut Checkout,
    at: &[u8],
    parent: u64,
    merging: u64,
) -> Result<u64, CommitFailure> {
    if !checkout.exists(&object(at, parent)) || !checkout.exists(&object(at, merging)) {
        return Err(CommitFailure::NotFetched);
    }
    let files: Vec<_> = checkout
        .tree(&[at, b"/.git/temper-conflicts"].concat())
        .into_values()
        .filter(|path| checkout.content(&[at, b"/", path].concat()).is_some_and(has_markers))
        .collect();
    if !files.is_empty() {
        return Err(CommitFailure::Unresolved { files });
    }
    let mut tree = checkout.tree(at);
    tree.retain(|path, _| !in_git(path));
    let commit = forge.store(parent, Some(merging), tree).expect("a merge always records both parents");
    checkout.write(&object(at, commit), b"");
    checkout.write(&[at, b"/.git/temper-head"].concat(), &commit.to_le_bytes());
    checkout.remove(&[at, b"/.git/temper-conflicts"].concat());
    checkout.remove(&[at, b"/.git/MERGE_HEAD"].concat());
    Ok(commit)
}

/// Push with an exact old-head condition in addition to fast-forward rules.
pub fn push_expected(
    forge: &mut impl Remote,
    checkout: &Checkout,
    remote: &[u8],
    at: &[u8],
    commit: u64,
    branch: &[u8],
    expected: u64,
) -> Result<Pushed, Fault> {
    assert!(checkout.exists(&object(at, commit)), "a push is of a locally held commit");
    forge.push(remote, branch, commit, Some(expected))
}

fn ancestors(forge: &impl Remote, start: u64) -> BTreeSet<u64> {
    let mut reached = BTreeSet::new();
    let mut pending = vec![start];
    while let Some(commit) = pending.pop() {
        if reached.insert(commit) {
            pending.extend(forge.parent(commit));
            pending.extend(forge.merge_parent(commit));
        }
    }
    reached
}

fn common_ancestor(forge: &impl Remote, ours: u64, theirs: u64) -> Option<u64> {
    // Store names are increasing topological numbers. The latest common
    // ancestor suffices for the world's single-base histories.
    let left = ancestors(forge, ours);
    let right = ancestors(forge, theirs);
    left.intersection(&right).copied().max()
}

fn markers(left: &[u8], right: &[u8]) -> Vec<u8> {
    let mut result = b"<<<<<<< ours\n".to_vec();
    result.extend_from_slice(left);
    if !left.ends_with(b"\n") {
        result.push(b'\n');
    }
    result.extend_from_slice(b"=======\n");
    result.extend_from_slice(right);
    if !right.ends_with(b"\n") {
        result.push(b'\n');
    }
    result.extend_from_slice(b">>>>>>> theirs\n");
    result
}

fn has_markers(content: &[u8]) -> bool {
    content
        .split(|byte| *byte == b'\n')
        .any(|line| line.starts_with(b"<<<<<<< ") || line == b"=======" || line.starts_with(b">>>>>>> "))
}

#[derive(PartialEq, Eq)]
struct Edit<'a> {
    start: usize,
    end: usize,
    replacement: Vec<&'a [u8]>,
}

fn edits<'a>(base: &[&[u8]], changed: &[&'a [u8]]) -> Vec<Edit<'a>> {
    let prefix = base.iter().zip(changed).take_while(|(left, right)| left == right).count();
    let suffix = base[prefix..]
        .iter()
        .rev()
        .zip(changed[prefix..].iter().rev())
        .take_while(|(left, right)| left == right)
        .count();
    let base = &base[prefix..base.len() - suffix];
    let changed = &changed[prefix..changed.len() - suffix];
    let mut lengths = vec![vec![0; changed.len() + 1]; base.len() + 1];
    for i in (0..base.len()).rev() {
        for j in (0..changed.len()).rev() {
            lengths[i][j] = if base[i] == changed[j] {
                lengths[i + 1][j + 1] + 1
            } else {
                lengths[i + 1][j].max(lengths[i][j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut result = Vec::new();
    while i < base.len() || j < changed.len() {
        if i < base.len() && j < changed.len() && base[i] == changed[j] {
            i += 1;
            j += 1;
            continue;
        }
        let start = i;
        let mut replacement = Vec::new();
        while i < base.len() || j < changed.len() {
            if i < base.len() && j < changed.len() && base[i] == changed[j] {
                break;
            }
            if j < changed.len() && (i == base.len() || lengths[i][j + 1] >= lengths[i + 1][j]) {
                replacement.push(changed[j]);
                j += 1;
            } else {
                i += 1;
            }
        }
        result.push(Edit { start: prefix + start, end: prefix + i, replacement });
    }
    result
}

fn merge_lines(base: &[u8], left: &[u8], right: &[u8]) -> Option<Vec<u8>> {
    let base: Vec<_> = base.split_inclusive(|byte| *byte == b'\n').collect();
    let left: Vec<_> = left.split_inclusive(|byte| *byte == b'\n').collect();
    let right: Vec<_> = right.split_inclusive(|byte| *byte == b'\n').collect();
    let mut changes = edits(&base, &left);
    for edit in edits(&base, &right) {
        if changes.contains(&edit) {
            continue;
        }
        if changes.iter().any(|old| {
            (old.start < edit.end && edit.start < old.end)
                || (old.start == old.end && old.start >= edit.start && old.start <= edit.end)
                || (edit.start == edit.end && edit.start >= old.start && edit.start <= old.end)
        }) {
            return None;
        }
        changes.push(edit);
    }
    changes.sort_by_key(|edit| edit.start);
    let mut result = Vec::new();
    let mut cursor = 0;
    for edit in changes {
        for line in &base[cursor..edit.start] {
            result.extend_from_slice(line);
        }
        for line in edit.replacement {
            result.extend_from_slice(line);
        }
        cursor = edit.end;
    }
    for line in &base[cursor..] {
        result.extend_from_slice(line);
    }
    Some(result)
}

/// Where the working tree at `at` keeps `commit`.
fn object(at: &[u8], commit: u64) -> Vec<u8> {
    [at, format!("/.git/objects/{commit}").as_bytes()].concat()
}
