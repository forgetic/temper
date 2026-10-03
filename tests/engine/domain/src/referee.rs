//! What the scenarios expect of the engine, held by a referee
//! (testing.md, 5.2) that sees only what is seen from outside: what
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
//! - nothing of a plan proposed is made before a person accepts it, and a
//!   goal's envelope widens only after a person accepted something of it
//!   since its last record;
//! - a run's call its grants allow is never answered as ungranted;
//! - a person's message is never lost: once the engine took it, it reaches
//!   a run of its item, or wakes the item, or the item is done.
//!
//! And liveness: every story's item ends (is closed) within a bound.
//! A restart of the engine changes none of it: every check is over what
//! happened, not over the engine that did it. A scenario may have the
//! engine restart at a chosen moment, as the forge shows it (an outcome
//! posted, a plan's first item made, a goal's record grown, a claim
//! written, a person's edit of the engine's labels or record), before the
//! engine hears its call answered.

use std::collections::{BTreeMap, BTreeSet};

use skein_lib::Duration;
use temper_engine_domain::notes::Scope;
use temper_engine_domain::plan::{Envelope, Grants};
use temper_engine_domain::work::Phase;
use temper_engine_domain::{Decoded, Item, Record, Refusal, Reply};
use temper_fake_forge_domain::Observation;
use temper_fake_forge_domain::api::{Kind, Verdict};
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
    /// `brief` is the text its brief carries, and `grants` what its charter
    /// grants it, if the world knows.
    Assigned { item: Item, attempt: u64, live: bool, brief: Vec<u8>, grants: Option<Grants> },
    /// A worker relayed the call `call` of the item's attempt, which needs
    /// `needs` of its grants.
    Called { item: Item, attempt: u64, call: u64, needs: Needs },
    /// A worker was answered the call `call` of the item's attempt:
    /// `ungranted` if its grants did not allow it.
    Served { item: Item, attempt: u64, call: u64, ungranted: bool },
    /// A worker was told of news of the item's, for its live run: the
    /// comment `comment`, if it is one.
    Inbound { item: Item, comment: Option<u64> },
    /// A worker answered the item's attempt.
    Answered { item: Item, attempt: u64 },
    /// The person of story `tale` knows its item.
    Story { tale: usize, item: Item },
    /// A person's message for the item was taken: the engine writes `text`
    /// on it, keyed by `key`.
    Messaged { item: Item, key: Vec<u8>, text: Vec<u8> },
    /// A person's acceptance of what the item waits for was answered so.
    Accepted { item: Item, reply: Reply },
    /// The world starts.
    Start,
    /// The engine restarted.
    Restarted,
    /// The world settled.
    Settled,
}

/// What of its grants a run's call needs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Needs {
    /// None: a comment, an escalation, a recall.
    Nothing,
    /// To read the forge.
    Forge,
    /// To note, in `scope`.
    Note(Scope),
}

/// What the referee expects to happen.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Expected {
    /// The story's item ends.
    Story(usize),
    /// A person's message reaches a run of its item, or the item ends.
    Message(Item, u32),
}

/// A moment, as the forge shows it, at which a scenario has the engine
/// restart, once: before the engine hears that what it asked was done.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Moment {
    /// An outcome is posted, and its record does not say yet that it is
    /// applied.
    Outcome,
    /// The first item of a plan accepted is made, and the others not yet.
    PlanItem,
    /// A goal's record lists the steps its plan grew by, and the growing
    /// step's record does not say yet that its outcome is applied.
    Growth,
    /// A record says its item is claimed, and no worker has the run yet.
    Claim,
    /// A person takes the tracking label off an item.
    Unlabelled,
    /// A person edits a comment of the engine's.
    Mangled,
}

/// What the referee injects: the engine restarting, a worker's channel
/// dropping.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Stimulus {
    Restart,
    Drop { worker: usize },
    Vanish { worker: usize },
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
    messages: BTreeMap<Item, Vec<Pending>>,
    asked: BTreeMap<Item, u32>,
    /// Goals a person accepted the proposal of.
    accepted: BTreeSet<Item>,
    /// Each goal's envelope as its last record said, and the acceptances
    /// of something of it since that a widening may count on.
    envelopes: BTreeMap<Item, Envelope>,
    acceptances: BTreeMap<Item, u32>,
    /// What each attempt assigned was granted, and the calls of attempts
    /// not answered yet.
    grants: BTreeMap<(Item, u64), Grants>,
    calls: BTreeMap<(Item, u64, u64), Needs>,
    /// The moments at which the engine restarts, and how many steps each
    /// goal's record lists.
    moments: Vec<Moment>,
    steps: BTreeMap<Item, usize>,
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
            asked: BTreeMap::new(),
            accepted: BTreeSet::new(),
            envelopes: BTreeMap::new(),
            acceptances: BTreeMap::new(),
            grants: BTreeMap::new(),
            calls: BTreeMap::new(),
            moments: Vec::new(),
            steps: BTreeMap::new(),
            restarts: 0,
        }
    }

    /// The same expectations, with the engine restarting at `moments`.
    #[must_use]
    pub fn restarting_at(self, moments: &[Moment]) -> Engine {
        Engine { moments: moments.to_vec(), ..self }
    }

    /// The engine restarts at `moment`, if a scenario wants it to and it
    /// has not yet.
    fn at(&mut self, moment: Moment, judge: &mut Judge<Expected, Stimulus>) {
        if let Some(at) = self.moments.iter().position(|wanted| *wanted == moment) {
            self.moments.remove(at);
            judge.inject_now(Stimulus::Restart);
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
                    if let Some(key) = temper_engine_forge_world::translate::key_of(body) {
                        let step = key.windows(b"/step/".len()).any(|window| window == b"/step/");
                        let first = self.issues.insert((repository.to_vec(), key), self.restarts);
                        self.once(first, judge, format_args!("an issue is created once per key: {observation:?}"));
                        if step {
                            self.at(Moment::PlanItem, judge);
                        }
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
                self.commented(repository, *number, *id, body, observation, judge);
            }
            Observation::Edited { repository, number, id, body, by } if *by == ENGINE => {
                match codec::comment(*id, body) {
                    Some(Decoded::Record { record, .. }) => self.recorded(repository, *number, &record, judge),
                    Some(Decoded::Outcome { .. } | Decoded::Page { .. }) | None => {}
                }
            }
            Observation::Edited { .. } => self.at(Moment::Mangled, judge),
            Observation::Labelled { labels, by, .. } if is_person(*by) => {
                if !labels.iter().any(|label| **label == *deployment::TRACKING) {
                    self.at(Moment::Unlabelled, judge);
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
    /// fails. Made again by an engine that restarted since, it is counted,
    /// and the restart scenarios hold it to none: an engine looks for what
    /// an earlier life may have made before making it, from where the cause
    /// it knows says, which a restart at a random moment may leave short.
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
    /// The engine commented: a record, an outcome, or a keyed comment.
    fn commented(
        &mut self,
        repository: &[u8],
        number: u64,
        id: u64,
        body: &[u8],
        observation: &Observation,
        judge: &mut Judge<Expected, Stimulus>,
    ) {
        let decoded = codec::comment(id, body);
        let posted = match decoded {
            Some(Decoded::Record { record, .. }) => {
                self.recorded(repository, number, &record, judge);
                None
            }
            Some(Decoded::Outcome { posted, .. }) => Some(posted),
            Some(Decoded::Page { .. }) | None => None,
        };
        if let Some(posted) = posted {
            let first = self.outcomes.insert((repository.to_vec(), number, posted.attempt), self.restarts);
            self.once(first, judge, format_args!("an attempt's outcome is posted once: {observation:?}"));
            self.at(Moment::Outcome, judge);
        } else if let Some(key) = temper_engine_forge_world::translate::key_of(body) {
            // A person's message, written: the comment it is.
            if let Some(index) = deployment::index(repository) {
                let item = Item { repository: index, number };
                for pending in self.messages.get_mut(&item).into_iter().flatten() {
                    if pending.key == key {
                        pending.comment = Some(id);
                    }
                }
            }
            let first = self.comments.insert((repository.to_vec(), number, key), self.restarts);
            self.once(first, judge, format_args!("a keyed comment is posted once: {observation:?}"));
        }
    }

    /// The engine wrote a record on the item `number`: nothing of a goal's
    /// plan is made before a person accepts it, and a goal's envelope
    /// widens, from what its last record said, only once a person accepted
    /// something of it since (growth beyond the envelope, held for them).
    fn recorded(&mut self, repository: &[u8], number: u64, record: &Record, judge: &mut Judge<Expected, Stimulus>) {
        let Some(index) = deployment::index(repository) else { return };
        let item = Item { repository: index, number };
        match record.lifecycle.phase {
            Phase::Claimed => self.at(Moment::Claim, judge),
            Phase::Waiting
            | Phase::Parked
            | Phase::Retrying(_)
            | Phase::Applying { .. }
            | Phase::Held { .. }
            | Phase::Done => {}
        }
        if let Some(goal) = record.relations.goal
            && goal != item
        {
            judge.check(
                self.accepted.contains(&goal),
                format_args!("nothing of {goal:?}'s plan is made before a person accepts it: {number}"),
            );
        }
        let Some(goal) = &record.step.goal else { return };
        let before = self.steps.insert(item, goal.steps.len());
        if before.is_some_and(|before| before < goal.steps.len()) {
            self.at(Moment::Growth, judge);
        }
        let envelope = goal.envelope.clone();
        let Some(before) = self.envelopes.insert(item, envelope.clone()) else {
            // Its plan, accepted: what a person accepted is spent on it.
            self.acceptances.remove(&item);
            return;
        };
        if !widens(&envelope, &before) {
            return;
        }
        let accepted = self.acceptances.get(&item).copied().unwrap_or(0);
        judge.check(
            accepted > 0,
            format_args!(
                "{item:?}'s envelope widens only once a person accepted its growth: {before:?} to {envelope:?}"
            ),
        );
        self.acceptances.insert(item, accepted.saturating_sub(1));
    }

    /// The goal of whatever a person accepted on `item`: its own, if it is
    /// one, or the one its record names.
    fn goal_of(&self, item: Item) -> Item {
        let Some(record) = self.mirror.record(deployment::name(item.repository), item.number) else { return item };
        if record.step.goal.is_some() {
            return item;
        }
        record.relations.goal.unwrap_or(item)
    }

    /// The item ended: every message for it has nothing left to reach.
    fn reached(&mut self, item: Item, judge: &mut Judge<Expected, Stimulus>) {
        for pending in self.messages.remove(&item).unwrap_or_default() {
            judge.meet(&Expected::Message(item, pending.index));
        }
    }

    /// The messages for `item` that `reaches` says a run was given.
    fn delivered(&mut self, item: Item, judge: &mut Judge<Expected, Stimulus>, reaches: impl Fn(&Pending) -> bool) {
        let Some(pending) = self.messages.get_mut(&item) else { return };
        let (met, left): (Vec<Pending>, Vec<Pending>) = pending.drain(..).partition(|pending| reaches(pending));
        *pending = left;
        for pending in met {
            judge.meet(&Expected::Message(item, pending.index));
        }
    }
}

/// A person's message waiting to reach a run: its place among the item's,
/// its key and text, and the comment it was written as, once seen.
#[derive(Debug)]
struct Pending {
    index: u32,
    key: Vec<u8>,
    text: Vec<u8>,
    comment: Option<u64>,
}

impl Expectations for Engine {
    type Seen = Seen;
    type Name = Expected;
    type Stimulus = Stimulus;

    fn observe(&mut self, seen: Seen, judge: &mut Judge<Expected, Stimulus>) {
        match seen {
            Seen::Forge(observation) => self.forge(&observation, judge),
            Seen::Assigned { item, attempt, live, brief, grants } => {
                if let Some(grants) = grants {
                    self.grants.insert((item, attempt), grants);
                }
                let last = self.attempts.insert(item, attempt).unwrap_or(0);
                judge.check(attempt > last, format_args!("attempts of {item:?} only grow: {attempt} after {last}"));
                judge.check(!live, format_args!("one live run per item: {item:?} at {attempt}"));
                self.delivered(item, judge, |pending| contains(&brief, &pending.text));
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
            Seen::Messaged { item, key, text } => {
                let name = deployment::name(item.repository);
                if self.mirror.issue(name, item.number).is_some_and(|issue| !issue.open) {
                    // Taken as its item closed: nothing is left to reach.
                    return;
                }
                let count = self.asked.entry(item).or_default();
                let index = *count;
                *count += 1;
                // The comment it was written as, if the forge showed it.
                let comment = self.mirror.issue(name, item.number).and_then(|issue| {
                    issue
                        .comments
                        .iter()
                        .find(|comment| {
                            temper_engine_forge_world::translate::key_of(&comment.body).as_deref() == Some(&key[..])
                        })
                        .map(|comment| comment.id)
                });
                judge.expect(Expected::Message(item, index), self.bounds.message);
                self.messages.entry(item).or_default().push(Pending { index, key, text, comment });
            }
            Seen::Accepted { item, reply } => match reply {
                Reply::Done => {
                    self.accepted.insert(item);
                    let goal = self.goal_of(item);
                    *self.acceptances.entry(goal).or_default() += 1;
                }
                // People accept only with the permission the rules want.
                Reply::Refused(refusal) => judge.check(
                    refusal != Refusal::Unpermitted,
                    format_args!("a person who may accept {item:?} is let: {refusal:?}"),
                ),
                Reply::Opened { .. } | Reply::Watching { .. } => {}
            },
            Seen::Start => {
                for tale in 0..self.count {
                    judge.expect(Expected::Story(tale), self.bounds.story);
                }
            }
            Seen::Restarted => self.restarts += 1,
            Seen::Inbound { item, comment: Some(id) } => {
                self.delivered(item, judge, |pending| pending.comment == Some(id));
            }
            Seen::Called { item, attempt, call, needs } => {
                self.calls.insert((item, attempt, call), needs);
            }
            Seen::Served { item, attempt, call, ungranted } => {
                let Some(needs) = self.calls.remove(&(item, attempt, call)) else { return };
                // An attempt adopted after a restart was not seen assigned.
                let Some(grants) = self.grants.get(&(item, attempt)) else { return };
                let granted = match needs {
                    Needs::Nothing => true,
                    Needs::Forge => grants.forge,
                    Needs::Note(scope) => grants.note && self.in_scope(item, scope),
                };
                judge.check(
                    !(ungranted && granted),
                    format_args!("{item:?}#{attempt}'s call {call}, which its grants allow, is served: {needs:?}"),
                );
            }
            Seen::Inbound { comment: None, .. } | Seen::Answered { .. } | Seen::Settled => {}
        }
    }
}

impl Engine {
    /// Whether a run of `item` may note in `scope`: the deployment's, its
    /// repository's, its own, or its goal's.
    fn in_scope(&self, item: Item, scope: Scope) -> bool {
        match scope {
            Scope::Deployment => true,
            Scope::Repository(repository) => repository == item.repository,
            Scope::Goal { repository, number } => {
                let scoped = Item { repository, number };
                let goal = self
                    .mirror
                    .record(deployment::name(item.repository), item.number)
                    .and_then(|record| record.relations.goal);
                scoped == item || goal == Some(scoped)
            }
        }
    }
}

/// Whether `envelope` lets a plan grow further than `before` did.
fn widens(envelope: &Envelope, before: &Envelope) -> bool {
    envelope.agents > before.agents
        || envelope.changes > before.changes
        || envelope.waits > before.waits
        || envelope.sessions > before.sessions
        || envelope.repositories.iter().any(|repository| !before.repositories.contains(repository))
        || envelope.into.iter().any(|target| !before.into.contains(target))
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

/// Whether `text` is found in `within`.
fn contains(within: &[u8], text: &[u8]) -> bool {
    text.is_empty() || within.windows(text.len()).any(|window| window == text)
}
