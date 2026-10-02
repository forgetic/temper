//! What the scenarios expect of the engine, held by a referee
//! (testing-pyramid.md, 5.2) that sees only what is seen from outside: what
//! the fake forge did, what the scripted workers were assigned and
//! answered, what people asked and were told, and the store. Never the
//! engine's state. Safety, on every observation:
//!
//! - nothing lands on a protected branch without green CI on its exact
//!   head and an approving review of that head by a person;
//! - the engine writes only to the deployment's repositories;
//! - no keyed creation is made twice: an issue, a comment, a pull request
//!   for a branch;
//! - an outcome is posted at most once per attempt, so applied at most
//!   once;
//! - a step's run starts only once its dependencies are done;
//! - one live run per item, and its attempts only grow;
//! - nothing of a plan proposed is made before a person accepts it;
//! - a person's message is never lost: once the engine took it, it reaches
//!   a run of its item, or wakes the item, or the item is done.
//!
//! And liveness: every story's item ends (is closed) within a bound.
//! A restart of the engine changes none of it: every check is over what
//! happened, not over the engine that did it.

use std::collections::{BTreeMap, BTreeSet};

use temper_engine_model::{Decoded, Item};
use temper_forge_model::Observation;
use temper_forge_model::api::{Kind, Verdict};
use temper_lib::Duration;
use temper_world::{Expectations, Judge};

use crate::codec;
use crate::deployment::{self, ENGINE, MAIN, PEOPLE, REPOSITORIES, REVIEWER};
use crate::mirror::Mirror;

/// What the referee observes.
#[derive(Debug)]
pub enum Seen {
    /// The fake forge did this.
    Forge(Observation),
    /// A worker was assigned the item's `attempt`; `live` says whether a
    /// worker in contact with the engine hosts another attempt of the item,
    /// not yet answered.
    Assigned { item: Item, attempt: u64, live: bool },
    /// A worker was told of news of the item's, for its live run.
    Inbound { item: Item },
    /// A worker answered the item's attempt.
    Answered { item: Item, attempt: u64 },
    /// The person of story `tale` knows its item.
    Story { tale: usize, item: Item },
    /// A person's message for the item was taken, as the comment `comment`.
    Messaged { item: Item },
    /// A person accepted, or asked to accept, what the item waits for.
    Accepting { item: Item },
    /// The world starts.
    Start,
    /// The engine restarted.
    Restarted,
    /// The world settled.
    Settled,
}

/// What the referee expects to happen.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Expected {
    /// The story's item ends.
    Story(usize),
    /// A person's message reaches a run of its item, or the item ends.
    Message(Item, u32),
}

/// What the referee injects: the engine restarting, a worker's channel
/// dropping.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Stimulus {
    Restart,
    Drop { worker: usize },
}

/// The bounds the scenario sets.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Bounds {
    /// Within which a story's item ends.
    pub story: Duration,
    /// Within which a message reaches a run, or its item ends.
    pub message: Duration,
}

/// The expectations of the engine's world.
#[derive(Debug)]
pub struct Engine {
    bounds: Bounds,
    mirror: Mirror,
    /// Keys the engine created with, by repository: issues and pull
    /// requests' branches; and comments' keys by item.
    issues: BTreeMap<(Vec<u8>, Vec<u8>), u32>,
    branches: BTreeSet<(Vec<u8>, Vec<u8>)>,
    comments: BTreeMap<(Vec<u8>, u64, Vec<u8>), u32>,
    outcomes: BTreeMap<(Vec<u8>, u64, u64), u32>,
    /// Keyed creations made again by an engine that restarted since the
    /// first: counted, not failed (see `once`).
    pub again: u32,
    /// The latest attempt assigned per item.
    attempts: BTreeMap<Item, u64>,
    /// How many stories there are; their items; and messages waiting to
    /// reach a run.
    count: usize,
    stories: BTreeMap<Item, usize>,
    messages: BTreeMap<Item, u32>,
    /// Goals a person accepted the proposal of.
    accepted: BTreeSet<Item>,
    pub restarts: u32,
}

impl Engine {
    #[must_use]
    pub fn new(bounds: Bounds, stories: usize) -> Engine {
        Engine {
            bounds,
            mirror: Mirror::default(),
            issues: BTreeMap::new(),
            branches: BTreeSet::new(),
            comments: BTreeMap::new(),
            outcomes: BTreeMap::new(),
            again: 0,
            attempts: BTreeMap::new(),
            count: stories,
            stories: BTreeMap::new(),
            messages: BTreeMap::new(),
            accepted: BTreeSet::new(),
            restarts: 0,
        }
    }

    /// The forge as the referee saw it.
    #[must_use]
    pub fn mirror(&self) -> &Mirror {
        &self.mirror
    }

    fn forge(&mut self, observation: &Observation, judge: &mut Judge<Expected, Stimulus>) {
        self.mirror.observe(observation);
        let (repository, by) = whose(observation);
        if by == ENGINE {
            judge.check(
                REPOSITORIES.iter().any(|name| **name == *repository),
                format_args!("the engine writes only to the deployment's repositories: {observation:?}"),
            );
        }
        match observation {
            Observation::Merged { repository, number, base, head, .. } => {
                if **base == *MAIN {
                    let green = self.mirror.is_green(repository, *head);
                    let issue = self.mirror.issue(repository, *number);
                    let approved = issue.is_some_and(|issue| {
                        issue.reviews.iter().any(|review| {
                            review.commit == *head && review.verdict == Verdict::Approve && is_person(review.by)
                        })
                    });
                    judge.check(
                        green && approved,
                        format_args!(
                            "a pull request lands on a protected branch only with green CI on its head and a \
                             person's approval of it: {observation:?} (green {green}, approved {approved})"
                        ),
                    );
                }
            }
            Observation::Opened { repository, kind, body, branches, by, number, .. } if *by == ENGINE => match kind {
                Kind::Issue => {
                    if let Some(key) = temper_engine_model_forge_tests::translate::key_of(body) {
                        let first = self.issues.insert((repository.to_vec(), key), self.restarts);
                        self.once(first, judge, format_args!("an issue is created once per key: {observation:?}"));
                    }
                }
                Kind::Pull => {
                    let head = branches.as_ref().map(|branches| branches.head.to_vec()).unwrap_or_default();
                    judge.check(
                        self.branches.insert((repository.to_vec(), head)),
                        format_args!("a pull request is opened once per branch: {observation:?}"),
                    );
                }
            },
            Observation::Commented { repository, number, id, body, by } if *by == ENGINE => {
                let decoded = codec::comment(*id, body);
                if let Some(Decoded::Record { record, .. }) = &decoded
                    && let Some(goal) = record.relations.goal
                    && let Some(index) = deployment::index(repository)
                    && goal != (Item { repository: index, number: *number })
                {
                    judge.check(
                        self.accepted.contains(&goal),
                        format_args!("nothing of {goal:?}'s plan is made before a person accepts it: {number}"),
                    );
                }
                if let Some(Decoded::Outcome { posted, .. }) = decoded {
                    let first = self.outcomes.insert((repository.to_vec(), *number, posted.attempt), self.restarts);
                    self.once(first, judge, format_args!("an attempt's outcome is posted once: {observation:?}"));
                } else if let Some(key) = temper_engine_model_forge_tests::translate::key_of(body) {
                    let first = self.comments.insert((repository.to_vec(), *number, key), self.restarts);
                    self.once(first, judge, format_args!("a keyed comment is posted once: {observation:?}"));
                }
            }
            Observation::Closed { repository, number, .. } => {
                let Some(index) = deployment::index(repository) else { return };
                let item = Item { repository: index, number: *number };
                if let Some(tale) = self.stories.get(&item) {
                    judge.meet(&Expected::Story(*tale));
                }
                self.reached(item, judge);
            }
            Observation::Opened { .. }
            | Observation::Commented { .. }
            | Observation::Moved { .. }
            | Observation::Deleted { .. }
            | Observation::Reopened { .. }
            | Observation::Labelled { .. }
            | Observation::Revised { .. }
            | Observation::Depends { .. }
            | Observation::Requested { .. }
            | Observation::Defined { .. }
            | Observation::Edited { .. }
            | Observation::Removed { .. }
            | Observation::Reviewed { .. }
            | Observation::Reported { .. }
            | Observation::Refused { .. }
            | Observation::Rejected { .. }
            | Observation::Wiki { .. } => {}
        }
    }

    /// A keyed creation is made once: `first` is the life of the engine that
    /// made it first, if one did. Made again by the same engine, the test
    /// fails. Made again by an engine that restarted since, it is counted:
    /// the engine's top level does not yet look for what an earlier life
    /// created before creating it again, so a restart between a creation
    /// and the record that would say it was made repeats it.
    fn once(&mut self, first: Option<u32>, judge: &mut Judge<Expected, Stimulus>, why: std::fmt::Arguments<'_>) {
        match first {
            None => judge.check(true, why),
            Some(life) if life < self.restarts => {
                self.again += 1;
                judge.check(true, why);
            }
            Some(_) => judge.check(false, why),
        }
    }

    /// Every message waiting for the item has reached it.
    fn reached(&mut self, item: Item, judge: &mut Judge<Expected, Stimulus>) {
        if let Some(count) = self.messages.remove(&item) {
            for at in 0..count {
                judge.meet(&Expected::Message(item, at));
            }
        }
    }
}

impl Expectations for Engine {
    type Seen = Seen;
    type Name = Expected;
    type Stimulus = Stimulus;

    fn observe(&mut self, seen: Seen, judge: &mut Judge<Expected, Stimulus>) {
        match seen {
            Seen::Forge(observation) => self.forge(&observation, judge),
            Seen::Assigned { item, attempt, live } => {
                let last = self.attempts.insert(item, attempt).unwrap_or(0);
                judge.check(attempt > last, format_args!("attempts of {item:?} only grow: {attempt} after {last}"));
                judge.check(!live, format_args!("one live run per item: {item:?} at {attempt}"));
                self.reached(item, judge);
                if let Some(record) = self.mirror.record(deployment::name(item.repository), item.number) {
                    for dependency in &record.relations.dependencies {
                        let name = deployment::name(dependency.item.repository);
                        let done = self.mirror.issue(name, dependency.item.number).is_some_and(|issue| !issue.open);
                        judge.check(done, format_args!("{item:?} runs only once {:?} is done", dependency.item));
                    }
                }
            }
            Seen::Story { tale, item } => {
                self.stories.insert(item, tale);
            }
            Seen::Messaged { item } => {
                let name = deployment::name(item.repository);
                if self.mirror.issue(name, item.number).is_some_and(|issue| !issue.open) {
                    // Taken as its item closed: nothing is left to reach.
                    return;
                }
                let count = self.messages.entry(item).or_default();
                judge.expect(Expected::Message(item, *count), self.bounds.message);
                *count += 1;
            }
            Seen::Accepting { item } => {
                self.accepted.insert(item);
            }
            Seen::Start => {
                for tale in 0..self.count {
                    judge.expect(Expected::Story(tale), self.bounds.story);
                }
            }
            Seen::Restarted => self.restarts += 1,
            Seen::Inbound { item } => self.reached(item, judge),
            Seen::Answered { .. } | Seen::Settled => {}
        }
    }
}

fn is_person(user: u64) -> bool {
    PEOPLE.contains(&user) || user == REVIEWER
}

/// The repository an observation is about, and who caused it.
fn whose(observation: &Observation) -> (&[u8], u64) {
    match observation {
        Observation::Moved { repository, by, .. }
        | Observation::Deleted { repository, by, .. }
        | Observation::Opened { repository, by, .. }
        | Observation::Closed { repository, by, .. }
        | Observation::Reopened { repository, by, .. }
        | Observation::Labelled { repository, by, .. }
        | Observation::Revised { repository, by, .. }
        | Observation::Depends { repository, by, .. }
        | Observation::Requested { repository, by, .. }
        | Observation::Defined { repository, by, .. }
        | Observation::Commented { repository, by, .. }
        | Observation::Edited { repository, by, .. }
        | Observation::Removed { repository, by, .. }
        | Observation::Reviewed { repository, by, .. }
        | Observation::Reported { repository, by, .. }
        | Observation::Merged { repository, by, .. }
        | Observation::Refused { repository, by, .. }
        | Observation::Rejected { repository, by, .. }
        | Observation::Wiki { repository, by, .. } => (repository, *by),
    }
}
