//! What the checkout asks of io (worker-domain.md, 8): git and file
//! operations on the worker's disk. The protocol layer runs each as one
//! contained git invocation, or one file operation, and parses what it says
//! into one terminal ([`Done`]); the domain never parses.
//!
//! A workspace is a directory io names by the workspace's token. Its
//! repositories sit side by side in it, each in a directory named by the
//! repository's name ([`Place`]), so that code that refers to a sibling by
//! path works. An operation that reaches the forge names the repository there
//! by its `remote`, the forge's address for it, which the protocol layer maps
//! to a URL and never interprets otherwise.
//!
//! io settles every operation before it ends it (5.3): one that runs out of
//! time or is cancelled ends only once its process tree has exited, so that
//! nothing of it touches the workspace afterwards.
//!
//! No credentials reach the disk: an operation that reaches the forge, or
//! commits, names the `identity` it acts as, which the protocol layer maps to
//! credentials, or to an author, for that invocation only, never into a
//! repository's configuration, where an agent could read them.

use alloc::boxed::Box;

use skein_lib::Token;

/// A commit, as git names it: its object id, in a fixed-size value the
/// protocol layer makes from git's output, the full id in 32 bytes (a SHA-256
/// id as it is, a SHA-1 id followed by twelve zero bytes). The domain compares
/// commits and never looks inside.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Commit([u8; 32]);

impl Commit {
    /// Made below the domain, from what git said.
    #[must_use]
    pub const fn new(raw: [u8; 32]) -> Commit {
        Commit(raw)
    }

    /// What it was made from, for the layer that made it.
    #[must_use]
    pub const fn raw(self) -> [u8; 32] {
        self.0
    }
}

/// A repository in a workspace: the workspace's directory, as io names it, and
/// the repository's name, the one path component its directory has there.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Place {
    pub workspace: Token,
    pub repository: Box<[u8]>,
}

/// An operation, asked of io. Each ends in the terminals its documentation
/// names, in [`Done::Succeeded`] if it names none, or in [`Done::Failed`].
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Op {
    /// Make the workspace's directory, empty: io removes whatever is there
    /// first, what the cache evicts or what an earlier worker left.
    Make { workspace: Token },
    /// Clone the repository at `remote` on the forge into its directory, which
    /// does not exist yet, without checking a tree out. Fails `Missing` if the
    /// forge has no such repository.
    Clone { at: Place, remote: Box<[u8]>, identity: Box<[u8]> },
    /// Fetch `want` from the repository at `remote` into the repository. Ends
    /// in `Fetched`, or fails `Missing` if the forge has no such branch or
    /// commit, or no default branch.
    Fetch { at: Place, remote: Box<[u8]>, want: Want, identity: Box<[u8]> },
    /// Create `branch` on the forge at `commit`, only if it does not exist:
    /// a branch that does is left where it is, and the operation ends in
    /// `Exists`.
    Create { at: Place, remote: Box<[u8]>, branch: Box<[u8]>, commit: Commit, identity: Box<[u8]> },
    /// Make the repository's working tree exactly `commit`'s tree: what is not
    /// in it is removed, and the git directory is left as it is.
    CheckOut { at: Place, commit: Commit },
    /// Commit the working tree exactly as it is, on `parent`, with the message
    /// `title` and `body`, authored as `identity`. Ends in `Committed`, or in
    /// `Unchanged` if the tree is `parent`'s.
    Commit { at: Place, parent: Commit, title: Box<[u8]>, body: Box<[u8]>, identity: Box<[u8]> },
    /// Push `commit` to `branch` on the forge, as a fast-forward, never forced:
    /// a branch that does not exist is created, and one that is not an
    /// ancestor of `commit` is left where it is, and the operation ends in
    /// `Rejected`.
    Push { at: Place, remote: Box<[u8]>, commit: Commit, branch: Box<[u8]>, identity: Box<[u8]> },
}

/// What a fetch asks for.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Want {
    /// The tip of a branch.
    Branch { branch: Box<[u8]> },
    /// A commit, by its hash.
    Commit { commit: Commit },
    /// The tip of the forge's default branch.
    Default,
}

/// io's terminal for an operation.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Done {
    /// `Make`, `Clone`, `Create`, `CheckOut`, `Push`: done as asked.
    Succeeded,
    /// `Fetch`: what was asked for is at `commit`, now in the repository.
    Fetched { commit: Commit },
    /// `Commit`: the tree is committed as `commit`.
    Committed { commit: Commit },
    /// `Commit`: the tree is its parent's, and nothing was committed.
    Unchanged,
    /// `Create`: the branch exists already, and was left where it is.
    Exists,
    /// `Push`: the branch is not an ancestor of the commit, so the push would
    /// not be a fast-forward; nothing was pushed.
    Rejected,
    /// The operation did not do what it was asked.
    Failed { fault: Fault },
}

/// Why an operation failed.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Fault {
    /// `Clone`, `Fetch`: the forge has no such repository, branch or commit.
    Missing { missing: Missing },
    /// `Clone`, `Fetch`, `Create`, `Push`: the forge refused: the identity's
    /// credentials or permissions, a protected branch, a hook.
    Refused,
    /// `Clone`, `Fetch`, `Create`, `Push`: the forge could not be reached.
    Unreachable,
    /// Something failed on the worker's side: the disk, or git itself.
    Broken,
    /// The deadline passed first, and io stopped it.
    TimedOut,
    /// The cancel won the race, and io stopped it.
    Cancelled,
}

/// What the forge does not have.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Missing {
    Repository,
    Branch,
    Commit,
}

/// An operation's kind, for the facts.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Kind {
    Make,
    Clone,
    Fetch,
    Create,
    CheckOut,
    Commit,
    Push,
}

impl Op {
    #[must_use]
    pub const fn kind(&self) -> Kind {
        match self {
            Op::Make { .. } => Kind::Make,
            Op::Clone { .. } => Kind::Clone,
            Op::Fetch { .. } => Kind::Fetch,
            Op::Create { .. } => Kind::Create,
            Op::CheckOut { .. } => Kind::CheckOut,
            Op::Commit { .. } => Kind::Commit,
            Op::Push { .. } => Kind::Push,
        }
    }

    /// Whether it reaches the forge, which decides its deadline.
    #[must_use]
    pub const fn is_remote(&self) -> bool {
        match self {
            Op::Clone { .. } | Op::Fetch { .. } | Op::Create { .. } | Op::Push { .. } => true,
            Op::Make { .. } | Op::CheckOut { .. } | Op::Commit { .. } => false,
        }
    }
}
