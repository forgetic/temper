//! The forge, abstractly: issues carrying the engine's records, the items'
//! branches and pull requests, CI and reviews on exact heads, the branches
//! changes land into, and the keys creations are found by. Commits are named
//! by a count. It is the truth the world's engine reads afresh before every
//! decision, and the only place its writes go.
//!
//! The record is kept as the typed value the plan reads and writes: a codec
//! that encodes it into a comment and decodes it back stands in its place
//! (the protocol layer's, below the model).

use std::collections::{BTreeMap, BTreeSet};

use temper_engine_model_plan::Record;
use temper_lib::Time;

/// An issue the engine tracks.
#[derive(Debug)]
pub struct Item {
    pub repository: u32,
    /// The plan's part of the engine's record.
    pub record: Record,
    /// The goal it is under: its plan's session. None for a goal itself, or a
    /// task on its own.
    pub goal: Option<u64>,
    /// The item whose outcome made it.
    pub parent: Option<u64>,
    pub created: Time,
    pub closed: Option<Time>,
    /// The label the engine projects on an item it holds for a person, with
    /// why.
    pub held: Option<&'static str>,
    /// The head of its branch, once a run pushed to it, and the head of the
    /// branch it lands into that the push was made on.
    pub branch: Option<Pushed>,
    /// A person's latest decision on it: accepted, or not.
    pub decision: Option<bool>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Pushed {
    pub head: u64,
    pub on: u64,
}

/// An item's pull request.
#[derive(Debug)]
pub struct Pull {
    pub repository: u32,
    pub base: Vec<u8>,
    pub state: State,
    /// CI on each head it reported on: passed, or not.
    pub ci: BTreeMap<u64, bool>,
    /// People approving each head.
    pub approvals: BTreeMap<u64, u32>,
    /// Heads someone asked for changes on.
    pub changes: BTreeSet<u64>,
    /// Whether each head conflicts with each head of its base, once the forge
    /// has worked it out.
    pub conflicts: BTreeMap<(u64, u64), bool>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    Open,
    Merged,
    Closed,
}

/// What a creation is keyed by: the outcome's item and attempt, and its key
/// within the outcome.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Keyed {
    pub item: u64,
    pub attempt: u64,
    pub key: Made,
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Made {
    Step(Vec<u8>),
    Task(u32),
}

#[derive(Debug)]
pub struct Forge {
    pub items: BTreeMap<u64, Item>,
    /// Pull requests, by the item whose change each is.
    pub pulls: BTreeMap<u64, Pull>,
    /// The heads of the branches changes land into, by repository and name.
    pub bases: BTreeMap<(u32, Vec<u8>), u64>,
    keys: BTreeMap<Keyed, u64>,
    numbers: u64,
    commits: u64,
}

impl Forge {
    /// A forge whose repositories have the branches `bases` (by repository,
    /// their names), each at a first commit.
    #[must_use]
    pub fn new(bases: &[(u32, &[u8])]) -> Forge {
        let mut forge = Forge {
            items: BTreeMap::new(),
            pulls: BTreeMap::new(),
            bases: BTreeMap::new(),
            keys: BTreeMap::new(),
            numbers: 0,
            commits: 0,
        };
        for (repository, name) in bases {
            let commit = forge.commit();
            forge.bases.insert((*repository, name.to_vec()), commit);
        }
        forge
    }

    /// A commit never made before.
    pub fn commit(&mut self) -> u64 {
        self.commits += 1;
        self.commits
    }

    /// Makes an item, or finds the one made with `keyed` before. Returns its
    /// number, and whether it is new.
    pub fn create(&mut self, keyed: Option<Keyed>, item: Item) -> (u64, bool) {
        if let Some(keyed) = &keyed
            && let Some(made) = self.keys.get(keyed)
        {
            return (*made, false);
        }
        self.numbers += 1;
        let number = self.numbers;
        self.items.insert(number, item);
        if let Some(keyed) = keyed {
            self.keys.insert(keyed, number);
        }
        (number, true)
    }

    #[must_use]
    pub fn item(&self, number: u64) -> &Item {
        self.items.get(&number).expect("an item the forge has")
    }

    pub fn item_mut(&mut self, number: u64) -> &mut Item {
        self.items.get_mut(&number).expect("an item the forge has")
    }

    /// Whether `number` is closed.
    #[must_use]
    pub fn is_closed(&self, number: u64) -> bool {
        self.item(number).closed.is_some()
    }

    /// The head of `base` in `repository`.
    #[must_use]
    pub fn base(&self, repository: u32, base: &[u8]) -> u64 {
        *self.bases.get(&(repository, base.to_vec())).expect("a branch the deployment lands into")
    }

    /// Moves `base` in `repository` to a new commit, returning it.
    pub fn move_base(&mut self, repository: u32, base: &[u8]) -> u64 {
        let commit = self.commit();
        self.bases.insert((repository, base.to_vec()), commit);
        commit
    }

    /// The open pull requests into `base` of `repository`, by item.
    #[must_use]
    pub fn open_into(&self, repository: u32, base: &[u8]) -> Vec<u64> {
        let mut open = Vec::new();
        for (item, pull) in &self.pulls {
            if pull.state == State::Open && pull.repository == repository && pull.base == base {
                open.push(*item);
            }
        }
        open
    }
}
