//! The forge child domain's parent, scripted: the engine's top level as the
//! forge child domain sees it, taking liberties the real one will not.
//!
//! It takes in what is handed in: it tracks the item, writes its record and
//! projects its labels (the tracking label on, the hand-in label off). Each
//! item's news starts a run, which answers after a while: the parent takes
//! the news and writes the record, the commit point, and now and then
//! projects its labels (only those the engine owns), starts a change (a
//! worker pushes its branch, and the parent opens its pull request, keyed by
//! the branch, and links it), asks a person to review it, writes a note in
//! the wiki at the revision it reads first, reads the item afresh, or closes
//! it. What a run creates (a reply, some of them a person's message written
//! for them; a task, which the item then depends on; a verdict on its
//! change) is applied as an outcome is: the outcome is posted on the item
//! first, keyed, read back for when the forge says it was posted, and named
//! as the cause of every creation, which is asked for again after its cause
//! if it timed out. A change whose CI passes is read afresh and merged at its
//! head, then its item closed and its branch deleted. A record that someone
//! else changed holds its item until a person releases it, after a while,
//! and the record is written over.
//!
//! When the engine restarts, so does the parent: what it held in memory is
//! gone, save what its record would say (each item's change and its pull
//! request), and the creations it had asked for and not heard of, which it
//! asks for again, resumed, after their causes, once the cold start ends.

use std::collections::{BTreeMap, BTreeSet};

use skein_lib::{Duration, Rng, Time, Token};
use temper_engine_domain_forge::api::{Answer, Error, State, Verdict};
use temper_engine_domain_forge::{
    Cause, Ci, Content, Event, Failure, Item, News, Read, Record, Request, View, Write, Written,
};
use temper_world::Span;

use crate::people::{ON_THE_WEB, PEOPLE};
use crate::referee::Planned;
use crate::translate::{self, Fill};
use crate::world::{MAIN, OWNED, TRACKING, WAITING, WORKING};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Script {
    /// From an item's news to its run's answer.
    pub run: Span,
    /// What a run's answer does besides taking the news and writing the
    /// record, by chance per mille.
    pub replies: u32,
    pub projections: u32,
    pub tasks: u32,
    pub changes: u32,
    pub verdicts: u32,
    pub requests: u32,
    pub notes: u32,
    pub reads: u32,
    pub closes: u32,
    /// Runs that do more than take the news, at most.
    pub runs: u32,
    /// Items taken in at most, besides those found tracked.
    pub takes: u32,
    /// From an item held to a person releasing it.
    pub release: Span,
}

/// What the parent does, at a moment the world draws.
#[derive(Debug)]
pub enum Action {
    /// An event for the child domain.
    Domain(Event),
    /// A write for the child domain, which the referee hears is planned.
    Write { owner: u64, write: Write, resumed: Option<Cause>, plan: Planned },
    /// An item's run answers.
    Run(Item),
    /// A worker pushes the branch of `item`'s change.
    Push { item: Item, branch: Vec<u8> },
    /// A person releases an item held.
    Release(Item),
}

/// What the parent counted.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct Tally {
    pub runs: u32,
    pub takes: u32,
    pub holds: u32,
    pub merges: u32,
    /// Creations asked for again after their causes; outcomes posted.
    pub resumed: u32,
    pub outcomes: u32,
}

/// A write the parent asked for, as it knows it.
#[derive(Clone, PartialEq, Eq, Debug)]
enum Intent {
    Record {
        item: Item,
    },
    Labels {
        item: Item,
        labels: Vec<Vec<u8>>,
    },
    /// A comment on `item`: a reply, a person's message written for them,
    /// or a run's outcome, which causes the creations `then`.
    Comment {
        item: Item,
        key: Vec<u8>,
        person: Option<u64>,
    },
    Outcome {
        item: Item,
        key: Vec<u8>,
        then: Vec<Intent>,
    },
    /// A task made from `from`, which then depends on it.
    Task {
        from: Item,
        key: Vec<u8>,
    },
    Open {
        item: Item,
        branch: Vec<u8>,
    },
    /// A verdict on the pull request `pull` of `item`'s change.
    Verdict {
        item: Item,
        pull: u64,
        key: Vec<u8>,
    },
    Reviewers {
        item: Item,
        pull: u64,
    },
    Depends {
        item: Item,
        on: Vec<u64>,
    },
    Merge {
        item: Item,
        pull: u64,
        head: [u8; 32],
    },
    Close {
        item: Item,
    },
    DeleteBranch {
        repository: u32,
        branch: Vec<u8>,
    },
    PutPage {
        repository: u32,
        name: Vec<u8>,
        revision: Option<u64>,
    },
}

impl Intent {
    /// Whether it creates something an earlier attempt may have made, which
    /// is asked for again after its cause.
    fn creates(&self) -> bool {
        match self {
            Intent::Comment { .. }
            | Intent::Outcome { .. }
            | Intent::Task { .. }
            | Intent::Open { .. }
            | Intent::Verdict { .. } => true,
            Intent::Record { .. }
            | Intent::Labels { .. }
            | Intent::Reviewers { .. }
            | Intent::Depends { .. }
            | Intent::Merge { .. }
            | Intent::Close { .. }
            | Intent::DeleteBranch { .. }
            | Intent::PutPage { .. } => false,
        }
    }
}

/// A write in flight: what it is, what caused it, and whether it was asked
/// for after its cause.
#[derive(Clone, Debug)]
struct Asked {
    intent: Intent,
    cause: Cause,
    resumed: bool,
}

/// What a fresh read of the parent's is for.
#[derive(Clone, Debug)]
enum Reading {
    /// The pull request of `item`'s change, to merge it.
    Pull(Item),
    /// A note, to write it at the revision read.
    Note { repository: u32, name: Vec<u8> },
    /// The outcome `comment` posted on an item, for when it was posted: the
    /// cause of `then`.
    Outcome { comment: u64, then: Vec<Intent> },
    /// Nothing but the read.
    Other,
}

/// An item the parent tracks.
#[derive(Debug)]
struct Held {
    labels: Vec<Vec<u8>>,
    /// The last news told, not yet taken.
    through: Option<u64>,
    running: bool,
    /// Its record was changed by someone else: held until a person releases
    /// it.
    on_hold: bool,
}

/// An item's change: its branch, and its pull request once opened.
#[derive(Clone, Debug)]
struct Change {
    branch: Vec<u8>,
    pull: Option<u64>,
}

/// A cause no earlier than anything: looked for from the first.
const FIRST: Cause = Cause { comment: 0, at: Time::ZERO };

pub struct Parent {
    script: Script,
    rng: Rng,
    items: BTreeMap<Item, Held>,
    taking: BTreeSet<Item>,
    /// The items it was refused for want of room, asked for again once there
    /// is room.
    refused: BTreeSet<Item>,
    fill: Fill,
    /// Names reads, writes and payloads.
    tokens: u64,
    writes: BTreeMap<u64, Asked>,
    /// The fresh reads in flight, by owner, and what each is for.
    reads: BTreeMap<u64, Reading>,
    resume: Vec<Asked>,
    changes: BTreeMap<Item, Change>,
    /// The tasks each item depends on.
    tasks: BTreeMap<Item, Vec<u64>>,
    runs: u32,
    takes: u32,
    tally: Tally,
}

impl Parent {
    #[must_use]
    pub fn new(script: Script, seed: u64) -> Parent {
        Parent {
            script,
            rng: Rng::new(seed),
            items: BTreeMap::new(),
            taking: BTreeSet::new(),
            refused: BTreeSet::new(),
            fill: Fill::new(),
            tokens: 0,
            writes: BTreeMap::new(),
            reads: BTreeMap::new(),
            resume: Vec::new(),
            changes: BTreeMap::new(),
            tasks: BTreeMap::new(),
            runs: script.runs,
            takes: script.takes,
            tally: Tally::default(),
        }
    }

    /// The payloads it names, for the protocol layer to fill in.
    #[must_use]
    pub fn fill(&self) -> &Fill {
        &self.fill
    }

    #[must_use]
    pub fn tally(&self) -> Tally {
        self.tally
    }

    /// Whether it still has writes or reads it waits on.
    #[must_use]
    pub fn is_waiting(&self) -> bool {
        !self.writes.is_empty() || !self.reads.is_empty() || !self.resume.is_empty()
    }

    /// The engine restarted: what the parent held in memory is gone, save the
    /// creations it asked for, which it asks for again after their causes.
    pub fn restart(&mut self) {
        self.items.clear();
        self.taking.clear();
        self.refused.clear();
        self.reads.clear();
        for asked in std::mem::take(&mut self.writes).into_values() {
            if asked.intent.creates() {
                self.resume.push(asked);
            }
        }
    }

    /// What the child domain told: what the parent does about it.
    pub fn told(&mut self, request: &Request) -> Vec<(Duration, Action)> {
        let mut actions = Vec::new();
        match request {
            Request::Call { .. } => unreachable!("calls go to the forge"),
            Request::Offered { item } => {
                if self.takes > 0 && !self.items.contains_key(item) && self.taking.insert(*item) {
                    self.takes -= 1;
                    self.tally.takes += 1;
                    actions.push((self.soon(), Action::Domain(Event::Track { item: *item })));
                }
            }
            Request::Full { item } => {
                self.taking.remove(item);
                self.refused.insert(*item);
            }
            Request::Room => {
                // What was refused is asked for again.
                for item in std::mem::take(&mut self.refused) {
                    if !self.items.contains_key(&item) && self.taking.insert(item) {
                        actions.push((self.soon(), Action::Domain(Event::Track { item })));
                    }
                }
            }
            Request::Forbidden { .. } | Request::Wiki { .. } => {}
            Request::Announced { item, view } => self.announced(*item, view, &mut actions),
            Request::Inbox { item, seq, news } => self.news(*item, *seq, *news, &mut actions),
            Request::Changed { item, labels } => {
                if let Some(held) = self.items.get_mut(item) {
                    held.labels = labels.iter().map(|label| label.to_vec()).collect();
                }
            }
            Request::Left { item, .. } => {
                self.items.remove(item);
                self.taking.remove(item);
                self.changes.remove(item);
            }
            Request::Loaded => {
                for asked in std::mem::take(&mut self.resume) {
                    self.tally.resumed += 1;
                    let write = self.ask(asked.intent, asked.cause, true);
                    actions.push((Duration::ZERO, write));
                }
            }
            Request::Read { owner, result } => {
                if let Some(reading) = self.reads.remove(&owner.raw()) {
                    self.read(reading, result, &mut actions);
                }
            }
            Request::Wrote { owner, result } => self.wrote(owner.raw(), result, &mut actions),
        }
        actions
    }

    /// A fresh read of the parent's answered.
    fn read(&mut self, reading: Reading, result: &Result<Answer, Failure>, actions: &mut Vec<(Duration, Action)>) {
        match reading {
            Reading::Pull(item) => {
                let Ok(Answer::Pull(pull)) = result else {
                    return;
                };
                let green = pull.ci == Ci::Passed;
                let open = pull.state == State::Open && pull.mergeable && pull.merged.is_none();
                if green && open && self.items.contains_key(&item) {
                    let intent = Intent::Merge { item, pull: pull.number, head: pull.commit };
                    actions.push((Duration::ZERO, self.write(intent)));
                }
            }
            Reading::Note { repository, name } => {
                let revision = match result {
                    Ok(Answer::Page(page)) => Some(page.revision),
                    Err(Failure::Forge(Error::Missing)) => None,
                    Ok(_) | Err(_) => return,
                };
                actions.push((Duration::ZERO, self.write(Intent::PutPage { repository, name, revision })));
            }
            Reading::Outcome { comment, then } => {
                // When the forge says the outcome was posted: what it causes
                // is made after it.
                let Ok(Answer::Item { comments, .. }) = result else {
                    return;
                };
                let Some(found) = comments.iter().find(|found| found.id == comment) else {
                    return;
                };
                let cause = Cause { comment, at: found.created };
                for intent in then {
                    actions.push((self.soon(), self.ask(intent, cause, false)));
                }
            }
            Reading::Other => {}
        }
    }

    fn announced(&mut self, item: Item, view: &View, actions: &mut Vec<(Duration, Action)>) {
        self.taking.remove(&item);
        let labels: Vec<Vec<u8>> = view.labels.iter().map(|label| label.to_vec()).collect();
        self.items.insert(item, Held { labels, through: None, running: false, on_hold: false });
        if let Some(pull) = self.changes.get(&item).and_then(|change| change.pull) {
            actions.push((Duration::ZERO, Action::Domain(Event::Link { item, pull: Some(pull) })));
        }
        match view.record {
            Record::Missing => actions.push((Duration::ZERO, self.write(Intent::Record { item }))),
            Record::Mangled { .. } => self.hold(item, actions),
            Record::Found { .. } => {
                if let Some(intent) = self.projection(item, false) {
                    actions.push((Duration::ZERO, self.write(intent)));
                }
            }
        }
    }

    fn news(&mut self, item: Item, seq: u64, news: News, actions: &mut Vec<(Duration, Action)>) {
        let run = self.script.run.draw(&mut self.rng);
        let Some(held) = self.items.get_mut(&item) else {
            return;
        };
        held.through = Some(seq);
        if !held.running {
            held.running = true;
            actions.push((run, Action::Run(item)));
        }
        let mergeable = match news {
            News::Pull { ci, open, merged, mergeable, .. } => ci == Ci::Passed && open && merged.is_none() && mergeable,
            News::Comment { .. } | News::Reviews { .. } => false,
        };
        let pull = self.changes.get(&item).and_then(|change| change.pull);
        if mergeable && let Some(pull) = pull {
            let pull = Item { repository: item.repository, number: pull };
            let owner = self.token();
            self.reads.insert(owner, Reading::Pull(item));
            actions.push((
                self.soon(),
                Action::Domain(Event::Read { owner: Token::new(owner), read: Read::Pull { item: pull } }),
            ));
        }
    }

    fn wrote(&mut self, owner: u64, result: &Result<Written, Failure>, actions: &mut Vec<(Duration, Action)>) {
        let Some(asked) = self.writes.remove(&owner) else {
            return;
        };
        let written = match result {
            Ok(written) => *written,
            Err(Failure::Busy) => {
                // No room: asked again in a while, as it was.
                let write = self.ask(asked.intent, asked.cause, asked.resumed);
                actions.push((Duration::from_secs(5), write));
                return;
            }
            Err(Failure::Forge(Error::Timeout)) => {
                // It may have been made: asked again, to be looked for after
                // its cause.
                if asked.intent.creates() {
                    self.tally.resumed += 1;
                    let write = self.ask(asked.intent, asked.cause, true);
                    actions.push((Duration::from_secs(5), write));
                }
                return;
            }
            Err(Failure::Edited { .. }) => {
                match asked.intent {
                    Intent::Record { item } => self.hold(item, actions),
                    Intent::Labels { .. }
                    | Intent::Comment { .. }
                    | Intent::Outcome { .. }
                    | Intent::Task { .. }
                    | Intent::Open { .. }
                    | Intent::Verdict { .. }
                    | Intent::Reviewers { .. }
                    | Intent::Depends { .. }
                    | Intent::Merge { .. }
                    | Intent::Close { .. }
                    | Intent::DeleteBranch { .. }
                    | Intent::PutPage { .. } => {}
                }
                return;
            }
            Err(Failure::Invalid | Failure::Unknown | Failure::Revised { .. } | Failure::Forge(_)) => return,
        };
        self.made(asked.intent, written, actions);
    }

    /// A write of `intent` made: what follows it.
    fn made(&mut self, intent: Intent, written: Written, actions: &mut Vec<(Duration, Action)>) {
        match intent {
            Intent::Record { item } => {
                if let Some(intent) = self.projection(item, false) {
                    actions.push((Duration::ZERO, self.write(intent)));
                }
            }
            Intent::Outcome { item, then, .. } => {
                let Written::Commented(comment) = written else {
                    return;
                };
                let owner = self.token();
                self.reads.insert(owner, Reading::Outcome { comment, then });
                let read = Read::Item { item, after: comment.saturating_sub(1) };
                actions.push((Duration::ZERO, Action::Domain(Event::Read { owner: Token::new(owner), read })));
            }
            Intent::Task { from, .. } => {
                let Written::Created(number) = written else {
                    return;
                };
                let item = Item { repository: from.repository, number };
                if self.taking.insert(item) {
                    actions.push((self.soon(), Action::Domain(Event::Track { item })));
                }
                // The item it was made from waits on it.
                let tasks = self.tasks.entry(from).or_default();
                if tasks.len() < 4 {
                    tasks.push(number);
                }
                let on = tasks.clone();
                if self.items.contains_key(&from) {
                    actions.push((self.soon(), self.write(Intent::Depends { item: from, on })));
                }
            }
            Intent::Open { item, branch } => {
                let Written::Created(pull) = written else {
                    return;
                };
                self.changes.insert(item, Change { branch, pull: Some(pull) });
                actions.push((Duration::ZERO, Action::Domain(Event::Link { item, pull: Some(pull) })));
            }
            Intent::Merge { item, .. } => {
                self.tally.merges += 1;
                actions.push((Duration::ZERO, self.write(Intent::Close { item })));
                if let Some(change) = self.changes.get(&item) {
                    let branch = change.branch.clone();
                    let intent = Intent::DeleteBranch { repository: item.repository, branch };
                    actions.push((Duration::ZERO, self.write(intent)));
                }
            }
            Intent::Labels { .. }
            | Intent::Comment { .. }
            | Intent::Verdict { .. }
            | Intent::Reviewers { .. }
            | Intent::Depends { .. }
            | Intent::Close { .. }
            | Intent::DeleteBranch { .. }
            | Intent::PutPage { .. } => {}
        }
    }

    /// An item's run answers: its news taken, its record written, and,
    /// while runs are left, what else the answer does: what it creates
    /// through an outcome posted first.
    pub fn run(&mut self, item: Item) -> Vec<(Duration, Action)> {
        let mut actions = Vec::new();
        let Some(held) = self.items.get_mut(&item) else {
            return actions;
        };
        held.running = false;
        let Some(through) = held.through.take() else {
            return actions;
        };
        self.tally.runs += 1;
        actions.push((Duration::ZERO, Action::Domain(Event::Took { item, through })));
        if !held.on_hold {
            actions.push((Duration::ZERO, self.write(Intent::Record { item })));
        }
        if self.runs == 0 {
            return actions;
        }
        self.runs -= 1;
        let mut then = Vec::new();
        if self.rng.chance(self.script.replies) {
            let key = self.key(b"reply", item);
            // Some replies are a person's message from the web.
            let person = if self.rng.chance(300) { Some(ON_THE_WEB) } else { None };
            then.push(Intent::Comment { item, key, person });
        }
        if self.rng.chance(self.script.tasks) {
            let key = self.key(b"task", item);
            then.push(Intent::Task { from: item, key });
        }
        let pull = self.changes.get(&item).and_then(|change| change.pull);
        if let Some(pull) = pull {
            if self.rng.chance(self.script.verdicts) {
                let key = self.key(b"verdict", item);
                then.push(Intent::Verdict { item, pull, key });
            }
            if self.rng.chance(self.script.requests) {
                actions.push((self.soon(), self.write(Intent::Reviewers { item, pull })));
            }
        }
        if !then.is_empty() {
            self.tally.outcomes += 1;
            let key = self.key(b"outcome", item);
            actions.push((Duration::ZERO, self.write(Intent::Outcome { item, key, then })));
        }
        if self.rng.chance(self.script.projections)
            && let Some(intent) = self.projection(item, true)
        {
            actions.push((self.soon(), self.write(intent)));
        }
        if self.rng.chance(self.script.changes) && !self.changes.contains_key(&item) {
            let branch = format!("change-{}", item.number).into_bytes();
            self.changes.insert(item, Change { branch: branch.clone(), pull: None });
            actions.push((self.soon(), Action::Push { item, branch }));
        }
        if self.rng.chance(self.script.notes) {
            let name = format!("note-{}", self.rng.below(4)).into_bytes();
            let owner = self.token();
            self.reads.insert(owner, Reading::Note { repository: item.repository, name: name.clone() });
            let read = Read::Page { repository: item.repository, name: name.into_boxed_slice() };
            actions.push((self.soon(), Action::Domain(Event::Read { owner: Token::new(owner), read })));
        }
        if self.rng.chance(self.script.reads) {
            let owner = self.token();
            self.reads.insert(owner, Reading::Other);
            // The item afresh, a note that may not be there, the statuses on
            // its change's head, or a verdict's inline comments.
            let read = match self.rng.below(4) {
                0 => Read::Item { item, after: 0 },
                1 => {
                    let name = format!("note-{}", self.rng.below(8)).into_bytes().into_boxed_slice();
                    Read::Page { repository: item.repository, name }
                }
                2 => Read::Statuses { repository: item.repository, commit: translate::commit(1), page: 1 },
                _ => Read::Remarks { item, review: 1, page: 1 },
            };
            actions.push((self.soon(), Action::Domain(Event::Read { owner: Token::new(owner), read })));
        }
        if self.rng.chance(self.script.closes) {
            actions.push((self.soon(), self.write(Intent::Close { item })));
        }
        actions
    }

    /// A worker pushed `item`'s branch: its pull request is opened.
    pub fn pushed(&mut self, item: Item, branch: Vec<u8>) -> Vec<(Duration, Action)> {
        if !self.items.contains_key(&item) {
            return Vec::new();
        }
        vec![(Duration::ZERO, self.write(Intent::Open { item, branch }))]
    }

    /// A person released `item`: its record is written over.
    pub fn release(&mut self, item: Item) -> Vec<(Duration, Action)> {
        let Some(held) = self.items.get_mut(&item) else {
            return Vec::new();
        };
        if !held.on_hold {
            return Vec::new();
        }
        held.on_hold = false;
        vec![(Duration::ZERO, self.write(Intent::Record { item }))]
    }

    /// Holds `item` for a person, who releases it after a while.
    fn hold(&mut self, item: Item, actions: &mut Vec<(Duration, Action)>) {
        let Some(held) = self.items.get_mut(&item) else {
            return;
        };
        if held.on_hold {
            return;
        }
        held.on_hold = true;
        self.tally.holds += 1;
        actions.push((self.script.release.draw(&mut self.rng), Action::Release(item)));
    }

    /// The labels the record projects for `item`, of those the engine owns:
    /// the tracking label on, the hand-in label off, and a phase label
    /// (another one if `turn`); none if they are so already. People's labels
    /// are theirs.
    fn projection(&mut self, item: Item, turn: bool) -> Option<Intent> {
        let held = self.items.get(&item)?;
        let working = held.labels.iter().any(|label| label == WORKING);
        let phase = if working == turn { WAITING } else { WORKING };
        // In order, as the forge keeps them.
        let mut labels: Vec<Vec<u8>> = vec![TRACKING.to_vec(), phase.to_vec()];
        labels.sort();
        let owned: Vec<Vec<u8>> =
            held.labels.iter().filter(|label| OWNED.contains(&label.as_slice())).cloned().collect();
        if labels == owned {
            return None;
        }
        Some(Intent::Labels { item, labels })
    }

    /// The write `intent`, caused by nothing on the forge.
    fn write(&mut self, intent: Intent) -> Action {
        self.ask(intent, FIRST, false)
    }

    /// The write `intent`, caused by `cause`, asked for again after it if
    /// `resumed`, named by a new owner; and what the referee hears of it.
    fn ask(&mut self, intent: Intent, cause: Cause, resumed: bool) -> Action {
        let owner = self.token();
        let write = self.operation(&intent);
        let plan = planned(&intent);
        self.writes.insert(owner, Asked { intent, cause, resumed });
        Action::Write { owner, write, resumed: if resumed { Some(cause) } else { None }, plan }
    }

    /// The child domain's write for `intent`.
    fn operation(&mut self, intent: &Intent) -> Write {
        match intent {
            Intent::Record { item } => {
                let payload = self.payload(format!("the record of #{}", item.number));
                Write::Record { item: *item, payload }
            }
            Intent::Labels { item, labels } => Write::SetLabels { item: *item, labels: boxed(labels) },
            Intent::Comment { item, key, person } => Write::Comment {
                item: *item,
                key: key.clone().into_boxed_slice(),
                person: *person,
                body: Content::Text(format!("a reply on #{}", item.number).into_bytes().into_boxed_slice()),
            },
            Intent::Outcome { item, key, .. } => Write::Comment {
                item: *item,
                key: key.clone().into_boxed_slice(),
                person: None,
                body: Content::Payload(self.payload(format!("the outcome of a run of #{}", item.number))),
            },
            Intent::Task { from, key } => Write::CreateIssue {
                repository: from.repository,
                key: key.clone().into_boxed_slice(),
                title: b"a task".to_vec().into_boxed_slice(),
                body: Content::Payload(self.payload("what the task is".to_owned())),
                labels: boxed(&[TRACKING.to_vec()]),
            },
            Intent::Open { item, branch } => Write::OpenPull {
                repository: item.repository,
                title: branch.clone().into_boxed_slice(),
                body: Content::Text(format!("the change of #{}", item.number).into_bytes().into_boxed_slice()),
                head: branch.clone().into_boxed_slice(),
                base: MAIN.to_vec().into_boxed_slice(),
            },
            // The engine opened the pull request: it may only comment on it.
            Intent::Verdict { item, pull, key } => Write::Review {
                item: Item { repository: item.repository, number: *pull },
                key: key.clone().into_boxed_slice(),
                verdict: Verdict::Comment,
                body: Content::Text(b"looked at it".to_vec().into_boxed_slice()),
            },
            Intent::Reviewers { item, pull } => Write::SetReviewers {
                item: Item { repository: item.repository, number: *pull },
                reviewers: Box::new([PEOPLE[1]]),
            },
            Intent::Depends { item, on } => {
                Write::SetDependencies { item: *item, dependencies: on.clone().into_boxed_slice() }
            }
            Intent::Merge { pull, head, item } => {
                Write::Merge { item: Item { repository: item.repository, number: *pull }, head: *head }
            }
            Intent::Close { item } => Write::Close { item: *item },
            Intent::DeleteBranch { repository, branch } => {
                Write::DeleteBranch { repository: *repository, branch: branch.clone().into_boxed_slice() }
            }
            Intent::PutPage { repository, name, revision } => Write::PutPage {
                repository: *repository,
                name: name.clone().into_boxed_slice(),
                content: Content::Text(b"what was learnt".to_vec().into_boxed_slice()),
                revision: *revision,
            },
        }
    }

    fn payload(&mut self, text: String) -> Token {
        let token = self.token();
        self.fill.insert(token, text.into_bytes());
        Token::new(token)
    }

    fn key(&mut self, what: &[u8], item: Item) -> Vec<u8> {
        let token = self.token();
        let mut key = what.to_vec();
        key.extend_from_slice(format!("-{}-{}-{token}", item.repository, item.number).as_bytes());
        key
    }

    fn token(&mut self) -> u64 {
        self.tokens += 1;
        self.tokens
    }

    /// A moment soon after now.
    fn soon(&mut self) -> Duration {
        Duration::from_millis(self.rng.below(2_000))
    }
}

/// What the referee hears is planned for `intent`.
fn planned(intent: &Intent) -> Planned {
    match intent {
        Intent::Record { item } => Planned::Record { item: *item },
        Intent::Labels { item, labels } => Planned::SetLabels { item: *item, labels: labels.clone() },
        Intent::Comment { item, key, .. } | Intent::Outcome { item, key, .. } => {
            Planned::Comment { item: *item, key: key.clone() }
        }
        Intent::Task { from, key } => Planned::CreateIssue { repository: from.repository, key: key.clone() },
        Intent::Open { item, branch } => Planned::OpenPull { repository: item.repository, head: branch.clone() },
        Intent::Verdict { item, pull, key } => {
            Planned::Review { item: Item { repository: item.repository, number: *pull }, key: key.clone() }
        }
        Intent::Reviewers { item, pull } => {
            Planned::SetReviewers { item: Item { repository: item.repository, number: *pull } }
        }
        Intent::Depends { item, .. } => Planned::SetDependencies { item: *item },
        Intent::Merge { item, pull, head } => {
            Planned::Merge { item: Item { repository: item.repository, number: *pull }, head: translate::count(*head) }
        }
        Intent::Close { item } => Planned::Close { item: *item },
        Intent::DeleteBranch { repository, branch } => {
            Planned::DeleteBranch { repository: *repository, branch: branch.clone() }
        }
        Intent::PutPage { repository, name, .. } => Planned::PutPage { repository: *repository, name: name.clone() },
    }
}

fn boxed(labels: &[Vec<u8>]) -> Box<[Box<[u8]>]> {
    labels.iter().map(|label| label.clone().into_boxed_slice()).collect()
}
