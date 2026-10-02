//! A fake forge's git, beside the fake file system: repositories, each a
//! remote of branches and the commits they reach, with a default branch; and
//! the operations a protocol layer would run as git invocations, applied to
//! working trees that are directories of a [`Checkout`]. It shares no types
//! with the model: a world translates between them.
//!
//! A working tree is a directory holding the repository's files and a git
//! directory, `.git`. Checking out replaces every file beneath it but the git
//! directory with a commit's tree; committing snapshots the files beneath it,
//! less the git directory. Commits live in one store, named by a count, so a
//! seed replays to the same names; a repository's remote has those that its
//! branches reach or that were pushed to it.
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
    pub repository: Vec<u8>,
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

    /// Adds the repository `name`, whose `default` branch is at a first commit
    /// of `tree`, and returns that commit.
    pub fn repository(&mut self, name: &[u8], default: &[u8], tree: Tree) -> u64 {
        let first = self.store(Object { parent: None, tree });
        let remote = Remote {
            default: default.to_vec(),
            branches: BTreeMap::from([(default.to_vec(), first)]),
            has: BTreeSet::from([first]),
            reachable: true,
            refusing: false,
        };
        assert!(self.repositories.insert(name.to_vec(), remote).is_none(), "a repository is added once");
        first
    }

    /// Another party commits on `branch` of `repository`, writing `path` with
    /// `content`, and moves the branch to it, as a push of its own would.
    /// Returns the commit.
    pub fn advance(&mut self, repository: &[u8], branch: &[u8], path: &[u8], content: &[u8]) -> u64 {
        let tip = self.branch(repository, branch).expect("a branch to advance");
        let mut tree = self.object(tip).tree.clone();
        tree.insert(path.to_vec(), content.to_vec());
        let commit = self.store(Object { parent: Some(tip), tree });
        let remote = self.remote_mut(repository);
        remote.has.insert(commit);
        remote.branches.insert(branch.to_vec(), commit);
        self.moved(repository, branch, Some(tip), commit);
        commit
    }

    /// Makes `repository` reachable, or not.
    pub fn set_reachable(&mut self, repository: &[u8], reachable: bool) {
        self.remote_mut(repository).reachable = reachable;
    }

    /// Makes `repository` refuse what is pushed to it, branches created
    /// included, or not.
    pub fn set_refusing(&mut self, repository: &[u8], refusing: bool) {
        self.remote_mut(repository).refusing = refusing;
    }

    /// Where `branch` of `repository` is.
    #[must_use]
    pub fn branch(&self, repository: &[u8], branch: &[u8]) -> Option<u64> {
        self.repositories.get(repository)?.branches.get(branch).copied()
    }

    /// The branches of `repository`, and where each is.
    #[must_use]
    pub fn branches(&self, repository: &[u8]) -> &BTreeMap<Vec<u8>, u64> {
        &self.remote(repository).branches
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

    /// Clones `repository` into the directory `at`, which does not exist: a
    /// git directory, and no tree checked out.
    pub fn clone_repository(&self, checkout: &mut Checkout, repository: &[u8], at: &[u8]) -> Result<(), Fault> {
        self.reach(repository)?;
        assert!(checkout.tree(at).is_empty(), "a clone goes where nothing is");
        checkout.mkdir(&[at, b"/.git"].concat());
        Ok(())
    }

    /// Fetches `want` from `repository`: the commit it names.
    pub fn fetch(&self, repository: &[u8], want: Want<'_>) -> Result<u64, Fault> {
        let remote = self.reach(repository)?;
        let found = match want {
            Want::Branch(branch) => remote.branches.get(branch).copied(),
            Want::Commit(commit) => remote.has.contains(&commit).then_some(commit),
            Want::Default => remote.branches.get(&remote.default).copied(),
        };
        let missing = match want {
            Want::Branch(_) | Want::Default => What::Branch,
            Want::Commit(_) => What::Commit,
        };
        found.ok_or(Fault::Missing(missing))
    }

    /// Creates `branch` of `repository` at `commit`, only if it does not
    /// exist.
    pub fn create(&mut self, repository: &[u8], branch: &[u8], commit: u64) -> Result<Created, Fault> {
        let remote = self.reach(repository)?;
        if remote.refusing {
            return Err(Fault::Refused);
        }
        assert!(remote.has.contains(&commit), "a branch is created at a commit the forge has");
        if remote.branches.contains_key(branch) {
            return Ok(Created::Exists);
        }
        self.remote_mut(repository).branches.insert(branch.to_vec(), commit);
        self.moved(repository, branch, None, commit);
        Ok(Created::Created)
    }

    /// Makes the working tree at `at` exactly `commit`'s tree, leaving its git
    /// directory.
    pub fn check_out(&self, checkout: &mut Checkout, at: &[u8], commit: u64) {
        checkout.replace_tree(at, &self.object(commit).tree);
    }

    /// Commits the working tree at `at` on `parent`: the files beneath it,
    /// less its git directory. Returns the commit, or `None` if the tree is
    /// `parent`'s.
    pub fn commit(&mut self, checkout: &Checkout, at: &[u8], parent: u64) -> Option<u64> {
        let mut tree = checkout.tree(at);
        tree.retain(|path, _| !in_git(path));
        if tree == self.object(parent).tree {
            return None;
        }
        Some(self.store(Object { parent: Some(parent), tree }))
    }

    /// Pushes `commit` to `branch` of `repository`, as a fast-forward: a
    /// branch that is nowhere is created, and one that is not an ancestor of
    /// `commit` is left where it is.
    pub fn push(&mut self, repository: &[u8], commit: u64, branch: &[u8]) -> Result<Pushed, Fault> {
        let remote = self.reach(repository)?;
        if remote.refusing {
            return Err(Fault::Refused);
        }
        let tip = remote.branches.get(branch).copied();
        if let Some(tip) = tip {
            if !self.is_ancestor(tip, commit) {
                return Ok(Pushed::Rejected);
            }
            if tip == commit {
                return Ok(Pushed::Pushed);
            }
        }
        let mut at = Some(commit);
        while let Some(pushed) = at {
            if !self.remote_mut(repository).has.insert(pushed) {
                break;
            }
            at = self.object(pushed).parent;
        }
        self.remote_mut(repository).branches.insert(branch.to_vec(), commit);
        self.moved(repository, branch, tip, commit);
        Ok(Pushed::Pushed)
    }

    fn reach(&self, repository: &[u8]) -> Result<&Remote, Fault> {
        let remote = self.repositories.get(repository).ok_or(Fault::Missing(What::Repository))?;
        if remote.reachable { Ok(remote) } else { Err(Fault::Unreachable) }
    }

    fn remote(&self, repository: &[u8]) -> &Remote {
        self.repositories.get(repository).expect("a repository of the forge")
    }

    fn remote_mut(&mut self, repository: &[u8]) -> &mut Remote {
        self.repositories.get_mut(repository).expect("a repository of the forge")
    }

    fn store(&mut self, object: Object) -> u64 {
        let commit = u64::try_from(self.commits.len()).expect("few commits") + 1;
        self.commits.insert(commit, object);
        commit
    }

    fn moved(&mut self, repository: &[u8], branch: &[u8], from: Option<u64>, to: u64) {
        self.moves.push(Move { repository: repository.to_vec(), branch: branch.to_vec(), from, to });
    }
}
