//! A fake forge's git, beside the fake file system: repositories, each a
//! remote of branches and the commits they reach, with a default branch; and
//! the operations a protocol layer would run as git invocations, applied to
//! working trees that are directories of a [`Checkout`]. It shares no types
//! with the model: a world translates between them.
//!
//! A repository on the forge is named by its remote, the forge's address for
//! it. A working tree is a directory holding the repository's files and a git
//! directory, `.git`. Checking out replaces every file beneath it but git
//! directories with a commit's tree; committing snapshots the files beneath
//! it, less git directories ([`crate::in_git`]). Commits live in one store,
//! named by a count, so a seed replays to the same names; a repository's
//! remote has those that its branches reach or that were pushed to it, and a
//! working tree those it cloned, fetched or committed, each a file in its git
//! directory (`.git/objects/COMMIT`), so that what removes the directory
//! removes them too. Checking out or committing on a commit the working tree
//! does not have fails, as it would in git.
//!
//! The forge keeps git's rules: a push moves a branch only by a fast-forward,
//! or creates it; a branch created for a base is only created, never moved.
//! Every move of a branch is recorded ([`Forge::moves`]), so that a world can
//! check them. What a world scripts: a repository that cannot be reached, one
//! that refuses what is pushed to it, and another party advancing a branch,
//! after which a push from where the branch was is rejected; and a spec that
//! names a repository, a branch or a commit the forge does not have.

use std::collections::{BTreeMap, BTreeSet};

use crate::{Checkout, in_git};

/// Files by their path from a working tree's root, with their content.
pub type Tree = BTreeMap<Vec<u8>, Vec<u8>>;

#[derive(Debug, Default)]
pub struct Forge {
    /// The repositories, by their remotes.
    repositories: BTreeMap<Vec<u8>, Remote>,
    /// Every commit there is, by its name.
    commits: BTreeMap<u64, Object>,
    moves: Vec<Move>,
}

#[derive(Clone, Debug)]
struct Remote {
    default: Vec<u8>,
    branches: BTreeMap<Vec<u8>, u64>,
    /// The commits it has: what its branches reach, and what was pushed.
    has: BTreeSet<u64>,
    reachable: bool,
    refusing: bool,
}

/// A commit: its parent, unless it is a repository's first, and its tree.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Object {
    pub parent: Option<u64>,
    pub tree: Tree,
}

/// A branch moved: created if it was nowhere.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Move {
    pub remote: Vec<u8>,
    pub branch: Vec<u8>,
    pub from: Option<u64>,
    pub to: u64,
}

/// Why an operation that reaches the forge failed.
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

/// A working tree does not have the commit: it was never cloned, fetched or
/// committed there.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct NotFetched;

/// How a push went: the branch is at the commit, or it is not an ancestor of
/// it and was left where it is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pushed {
    Pushed,
    Rejected,
}

impl Forge {
    #[must_use]
    pub fn new() -> Forge {
        Forge::default()
    }

    /// Adds the repository at `remote`, whose `default` branch is at a first
    /// commit of `tree`, and returns that commit.
    pub fn repository(&mut self, remote: &[u8], default: &[u8], tree: Tree) -> u64 {
        let first = self.store(Object { parent: None, tree });
        let repository = Remote {
            default: default.to_vec(),
            branches: BTreeMap::from([(default.to_vec(), first)]),
            has: BTreeSet::from([first]),
            reachable: true,
            refusing: false,
        };
        assert!(self.repositories.insert(remote.to_vec(), repository).is_none(), "a repository is added once");
        first
    }

    /// Another party commits on `branch` of the repository at `remote`,
    /// writing `path` with `content`, and moves the branch to it, as a push of
    /// its own would. Returns the commit.
    pub fn advance(&mut self, remote: &[u8], branch: &[u8], path: &[u8], content: &[u8]) -> u64 {
        let tip = self.branch(remote, branch).expect("a branch to advance");
        let mut tree = self.object(tip).tree.clone();
        tree.insert(path.to_vec(), content.to_vec());
        let commit = self.store(Object { parent: Some(tip), tree });
        let repository = self.remote_mut(remote);
        repository.has.insert(commit);
        repository.branches.insert(branch.to_vec(), commit);
        self.moved(remote, branch, Some(tip), commit);
        commit
    }

    /// Makes the repository at `remote` reachable, or not.
    pub fn set_reachable(&mut self, remote: &[u8], reachable: bool) {
        self.remote_mut(remote).reachable = reachable;
    }

    /// Makes the repository at `remote` refuse what is pushed to it, branches
    /// created included, or not.
    pub fn set_refusing(&mut self, remote: &[u8], refusing: bool) {
        self.remote_mut(remote).refusing = refusing;
    }

    /// Where `branch` of the repository at `remote` is.
    #[must_use]
    pub fn branch(&self, remote: &[u8], branch: &[u8]) -> Option<u64> {
        self.repositories.get(remote)?.branches.get(branch).copied()
    }

    /// The branches of the repository at `remote`, and where each is.
    #[must_use]
    pub fn branches(&self, remote: &[u8]) -> &BTreeMap<Vec<u8>, u64> {
        &self.remote(remote).branches
    }

    #[must_use]
    pub fn object(&self, commit: u64) -> &Object {
        self.commits.get(&commit).expect("a commit of the forge")
    }

    /// Whether `ancestor` is `commit` or one of its ancestors.
    #[must_use]
    pub fn is_ancestor(&self, ancestor: u64, commit: u64) -> bool {
        let mut at = Some(commit);
        while let Some(commit) = at {
            if commit == ancestor {
                return true;
            }
            at = self.object(commit).parent;
        }
        false
    }

    /// Every move of a branch so far, in order.
    #[must_use]
    pub fn moves(&self) -> &[Move] {
        &self.moves
    }

    // What git does for the protocol layer.

    /// Clones the repository at `remote` into the directory `at`, which does
    /// not exist: a git directory with every commit the forge has of it, and
    /// no tree checked out.
    pub fn clone_repository(&self, checkout: &mut Checkout, remote: &[u8], at: &[u8]) -> Result<(), Fault> {
        let has = self.reach(remote)?.has.clone();
        assert!(!checkout.exists(at), "a clone goes where nothing is");
        checkout.mkdir(&[at, b"/.git"].concat());
        for commit in has {
            checkout.write(&object(at, commit), b"");
        }
        Ok(())
    }

    /// Fetches `want` from the repository at `remote` into the working tree at
    /// `at`: the commit it names, and those before it.
    pub fn fetch(&self, checkout: &mut Checkout, remote: &[u8], at: &[u8], want: Want<'_>) -> Result<u64, Fault> {
        let repository = self.reach(remote)?;
        let found = match want {
            Want::Branch(branch) => repository.branches.get(branch).copied(),
            Want::Commit(commit) => repository.has.contains(&commit).then_some(commit),
            Want::Default => repository.branches.get(&repository.default).copied(),
        };
        let missing = match want {
            Want::Branch(_) | Want::Default => What::Branch,
            Want::Commit(_) => What::Commit,
        };
        let fetched = found.ok_or(Fault::Missing(missing))?;
        let mut next = Some(fetched);
        while let Some(commit) = next {
            if checkout.exists(&object(at, commit)) {
                break;
            }
            checkout.write(&object(at, commit), b"");
            next = self.object(commit).parent;
        }
        Ok(fetched)
    }

    /// Creates `branch` of the repository at `remote` at `commit`, only if it
    /// does not exist.
    pub fn create(&mut self, remote: &[u8], branch: &[u8], commit: u64) -> Result<Created, Fault> {
        let repository = self.reach(remote)?;
        if repository.refusing {
            return Err(Fault::Refused);
        }
        assert!(repository.has.contains(&commit), "a branch is created at a commit the forge has");
        if repository.branches.contains_key(branch) {
            return Ok(Created::Exists);
        }
        self.remote_mut(remote).branches.insert(branch.to_vec(), commit);
        self.moved(remote, branch, None, commit);
        Ok(Created::Created)
    }

    /// Makes the working tree at `at` exactly `commit`'s tree, leaving its git
    /// directories, if it has the commit.
    pub fn check_out(&self, checkout: &mut Checkout, at: &[u8], commit: u64) -> Result<(), NotFetched> {
        if !checkout.exists(&object(at, commit)) {
            return Err(NotFetched);
        }
        checkout.replace_tree(at, &self.object(commit).tree);
        Ok(())
    }

    /// Commits the working tree at `at` on `parent`, if it has that commit:
    /// the files beneath it, less its git directories. Returns the commit, or
    /// `None` if the tree is `parent`'s.
    pub fn commit(&mut self, checkout: &mut Checkout, at: &[u8], parent: u64) -> Result<Option<u64>, NotFetched> {
        if !checkout.exists(&object(at, parent)) {
            return Err(NotFetched);
        }
        let mut tree = checkout.tree(at);
        tree.retain(|path, _| !in_git(path));
        if tree == self.object(parent).tree {
            return Ok(None);
        }
        let commit = self.store(Object { parent: Some(parent), tree });
        checkout.write(&object(at, commit), b"");
        Ok(Some(commit))
    }

    /// Pushes `commit`, which the working tree at `at` has, to `branch` of the
    /// repository at `remote`, as a fast-forward: a branch that is nowhere is
    /// created, and one that is not an ancestor of `commit` is left where it
    /// is.
    pub fn push(
        &mut self,
        checkout: &Checkout,
        remote: &[u8],
        at: &[u8],
        commit: u64,
        branch: &[u8],
    ) -> Result<Pushed, Fault> {
        assert!(checkout.exists(&object(at, commit)), "a push is of a commit the working tree has");
        let repository = self.reach(remote)?;
        if repository.refusing {
            return Err(Fault::Refused);
        }
        let tip = repository.branches.get(branch).copied();
        if let Some(tip) = tip {
            if !self.is_ancestor(tip, commit) {
                return Ok(Pushed::Rejected);
            }
            if tip == commit {
                return Ok(Pushed::Pushed);
            }
        }
        let mut next = Some(commit);
        while let Some(pushed) = next {
            if !self.remote_mut(remote).has.insert(pushed) {
                break;
            }
            next = self.object(pushed).parent;
        }
        self.remote_mut(remote).branches.insert(branch.to_vec(), commit);
        self.moved(remote, branch, tip, commit);
        Ok(Pushed::Pushed)
    }

    fn reach(&self, remote: &[u8]) -> Result<&Remote, Fault> {
        let repository = self.repositories.get(remote).ok_or(Fault::Missing(What::Repository))?;
        if repository.reachable { Ok(repository) } else { Err(Fault::Unreachable) }
    }

    fn remote(&self, remote: &[u8]) -> &Remote {
        self.repositories.get(remote).expect("a repository of the forge")
    }

    fn remote_mut(&mut self, remote: &[u8]) -> &mut Remote {
        self.repositories.get_mut(remote).expect("a repository of the forge")
    }

    fn store(&mut self, object: Object) -> u64 {
        let commit = u64::try_from(self.commits.len()).expect("few commits") + 1;
        self.commits.insert(commit, object);
        commit
    }

    fn moved(&mut self, remote: &[u8], branch: &[u8], from: Option<u64>, to: u64) {
        self.moves.push(Move { remote: remote.to_vec(), branch: branch.to_vec(), from, to });
    }
}

/// Where the working tree at `at` keeps `commit`.
fn object(at: &[u8], commit: u64) -> Vec<u8> {
    [at, format!("/.git/objects/{commit}").as_bytes()].concat()
}
