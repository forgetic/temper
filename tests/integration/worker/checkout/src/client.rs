//! The scripted client: what stands in for the checkout's parent and the
//! hosted runs behind it. A client prepares a workspace for a workstream,
//! then, as a run's tools would, edits the writable repositories' working
//! trees between its pushes; it saves before it releases if its plan says so,
//! and it may abort what is under way, or release, at a moment of its own,
//! as a cancelled run's host would. It keeps to the boundary's contract: one
//! operation at a time, but for the pushes its plan sends twice, which are
//! refused as busy.

use skein_lib::{Duration, Token};
use temper_checkout_fake::git::Tree;
use temper_worker_domain_checkout::{Landing, Prepared};

/// What a client does, from its prepare to its release.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Plan {
    /// The workstream it works on, by its place among the world's.
    pub workstream: u32,
    /// Where every repository starts; drawn for each by the world if `None`.
    pub start: Option<Pick>,
    /// Pushes it makes once prepared, each after it has edited the trees.
    pub pushes: u32,
    /// Whether it saves before it releases.
    pub save: bool,
    /// When it interrupts, after it sent its prepare, and how, if it does.
    pub interrupt: Option<(Duration, Interrupt)>,
    /// Whether its spec is beyond the limits.
    pub invalid: bool,
    /// Whether it sends each push or save twice, the second refused as busy.
    pub twice: bool,
}

impl Plan {
    /// Prepares a workspace for `workstream`, starting from its base branch,
    /// pushes once and releases.
    #[must_use]
    pub const fn simple(workstream: u32) -> Plan {
        Plan {
            workstream,
            start: Some(Pick::Base),
            pushes: 1,
            save: false,
            interrupt: None,
            invalid: false,
            twice: false,
        }
    }
}

/// Where a spec starts a repository.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pick {
    /// The workstream's base branch, created from the default branch if the
    /// forge does not have it yet.
    Base,
    /// The default branch, by its name.
    Branch,
    /// The repository's first commit.
    Commit,
    /// The workstream's saved work, if the forge has it, and its base branch
    /// otherwise.
    Saved,
    /// A repository, a branch or a commit the forge does not have.
    MissingRepository,
    MissingBranch,
    MissingCommit,
}

/// How a client interrupts.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Interrupt {
    /// It aborts what is under way, then goes on as a cancelled run's host
    /// would: no more pushes, its save if it has one, and its release.
    Abort,
    /// It releases its hold at once.
    Release,
}

/// An operation a client waits for the end of.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Operation {
    Prepare,
    Push,
    Save,
}

/// Where a client's release stands.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Release {
    NotAsked,
    Asked,
    /// Released, or refused at the entrance: it holds nothing.
    Done,
}

/// A repository of a client's spec, as the client knows it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Repo {
    /// Its directory, and the forge's address for it.
    pub name: Vec<u8>,
    pub remote: Vec<u8>,
    pub writable: bool,
    /// The commit its spec names, if it starts from one.
    pub commit: Option<u64>,
}

/// A client, as the world runs it.
#[derive(Debug)]
pub struct Client {
    pub plan: Plan,
    /// Its spec's repositories, once it has sent its prepare.
    pub repos: Vec<Repo>,
    /// Its hold, once admitted, and its workspace, once ready.
    pub hold: Option<Token>,
    pub workspace: Option<Token>,
    /// How its prepare ended, once it has.
    pub prepared: Option<Prepared>,
    /// The operation it waits for the end of.
    pub operation: Option<Operation>,
    /// Pushes and saves sent twice, whose refusal has not come yet.
    pub refusals: u32,
    pub release: Release,
    pub pushes_left: u32,
    pub save_left: bool,
    /// For each repository: the tree it was checked out with, the tree of the
    /// last commit that landed on its push branch (the start's, until one
    /// does), and the tree it left for its last push or save.
    pub start: Vec<Tree>,
    pub pushed: Vec<Tree>,
    pub left: Vec<Tree>,
    /// What came of each push or save that was not refused: whether it was a
    /// save, and each repository's landing.
    pub landings: Vec<(bool, Box<[Landing]>)>,
}

impl Client {
    #[must_use]
    pub fn new(plan: Plan) -> Client {
        Client {
            plan,
            repos: Vec::new(),
            hold: None,
            workspace: None,
            prepared: None,
            operation: None,
            refusals: 0,
            release: Release::NotAsked,
            pushes_left: plan.pushes,
            save_left: plan.save,
            start: Vec::new(),
            pushed: Vec::new(),
            left: Vec::new(),
            landings: Vec::new(),
        }
    }
}
