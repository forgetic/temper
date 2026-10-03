//! The table of items (engine-model.md, section 4): an entry for each item
//! the forge sub-model announced and the top level took in, with its
//! record's parts, the job the hub asked for, what is in flight for it and
//! its inbox.
//!
//! The record is composed from its owners' parts whenever one is written,
//! and split when one is read (seams: "The record"): the hub's lifecycle as
//! last written or read, the plan's step as last committed, and the top
//! level's relations. An application stages the plan's writes and commits
//! them when its writes are made, so a record written meanwhile carries the
//! step as it was, and a restart applies the outcome again from there.
//!
//! Taking in follows the record read (4.6, 12): an item with a record is
//! taken into the hub as it says, and its claim is adopted at once; an item
//! without one is a session if it was handed in or opened, or a step if an
//! application made it, and is let go otherwise; one whose record does not
//! split is held for a person. Nothing new starts until the cold start is
//! done (`Model::loaded`).
//!
//! An item's inbox is what the forge sub-model told of it, and the top
//! level's notices of its relations, numbered as they come, kept until a run
//! has taken them: a run's answer takes what was there when it was claimed
//! and what was delivered to it in order, unless something bounced, and the
//! inbox position moves with it ([`forge::Event::Took`]).

use alloc::boxed::Box;

use temper_engine_model_brief as brief;
use temper_engine_model_forge::{self as forge, api};
use temper_engine_model_plan as plan;
use temper_engine_model_rules::Permission;
use temper_engine_model_work::{self as work, Lifecycle};
use temper_lib::{Env, Id, List, Map, ReplyTo, Time, Token};

use crate::boundary::{Assignment, Inbound, Item, Posted, Record, Refusal, Related, Relations, Reply, Request};
use crate::facts::Fact;
use crate::limits::{self, Limits};
use crate::model::{self, Model};
use crate::route;
use crate::translate;
use crate::waits::Wait;

/// An item the top level holds.
#[expect(clippy::struct_excessive_bools, reason = "independent facts of an item, each set and cleared on its own")]
#[derive(Debug)]
pub(crate) struct Entry {
    pub(crate) item: Item,
    pub(crate) taking: Taking,
    /// The hub's part, as last written or read.
    pub(crate) lifecycle: Lifecycle,
    /// The plan's part, as committed; `None` if the record did not split.
    pub(crate) step: Option<plan::Record>,
    /// The plan's part as an application leaves it, until its writes are
    /// made.
    pub(crate) staged: Option<plan::Record>,
    pub(crate) relations: Relations,
    pub(crate) job: Job,
    /// The run due, until it is claimed and started.
    pub(crate) due: Option<Box<plan::Run>>,
    /// The writes of the engine action due, until they are made: an action,
    /// or the writes that finish the step.
    pub(crate) action: Option<(Of, Box<[plan::Write]>)>,
    /// The relation an item asking reads afresh, among its dependencies then
    /// its children.
    pub(crate) asking: u32,
    /// The attempt in flight, from its start until its answer.
    pub(crate) live: Option<Live>,
    /// The assignment of the attempt in flight, while the fleet may assign
    /// it.
    pub(crate) assignment: Option<Box<Assignment>>,
    /// The grants of the attempt in flight, started or adopted: what its
    /// calls are served within.
    pub(crate) grants: Option<plan::Grants>,
    /// The attempt adopted after a restart, if the item's last claim was:
    /// what an earlier life may have made for it is looked for first.
    pub(crate) resumed: Option<Resumed>,
    /// The outcome posted and not wholly applied, by its comment.
    pub(crate) outcome: Option<(u64, Box<Posted>)>,
    pub(crate) inbox: Map<u64, Noted>,
    /// The number the next inbox event gets.
    pub(crate) next: u64,
    /// The forge's number of the first news the inbox had no room for: the
    /// inbox position stays before it, and the forge sub-model tells it
    /// again once a run's answer made room.
    pub(crate) unkept: Option<u64>,
    /// The last comment the item's runs have taken, for a brief's comments.
    pub(crate) since: u64,
    /// The head of its pull request as first seen, when, and where its base
    /// was then.
    pub(crate) seen: Option<Seen>,
    /// The forge no longer shows it open.
    pub(crate) closed: bool,
    /// The rules wait on facts before its action: nothing is due until news
    /// comes.
    pub(crate) blocked: bool,
    /// Its pull request was merged by the engine, at this commit, which the
    /// working set may not show yet.
    pub(crate) merged: Option<[u8; 32]>,
    /// The head the forge refused to merge for a conflict, which the
    /// working set may not show yet.
    pub(crate) conflicted: Option<[u8; 32]>,
    /// The permission the rules want of whoever accepts what it holds.
    pub(crate) wants: Option<Permission>,
    /// The attempt whose push the record's branch is, as far as this life
    /// knows: a later answer's push replaces it, an earlier one's does not.
    pub(crate) landed: u64,
    /// Its run, prepared before the cold start was done, waits for it.
    pub(crate) waiting: bool,
    /// The application that created it, waiting for its first record to be
    /// written before it goes on.
    pub(crate) holding: Option<Id<Entry>>,
    /// Its records written on the side, in flight.
    pub(crate) asides: u32,
    /// A record written on the side waits to go again.
    pub(crate) resave: bool,
    /// Its records written on the side that the forge failed for a while,
    /// in a row: each goes again, up to `RECORD_RETRIES`.
    pub(crate) retries: u32,
    /// What its application waits for beside its own writes: its goal's
    /// record, which its growth wrote into.
    pub(crate) awaiting: Option<Awaiting>,
    /// The person's call that opened it as a session, answered once its
    /// first record is written: a restart before then leaves it unanswered,
    /// and the call made again finds the issue by its key.
    pub(crate) opened: Option<ReplyTo>,
}

/// What an application waits for beside its own writes.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum Awaiting {
    /// The record of its goal, whose entry this is, written on the side:
    /// the application goes on once it is.
    Goal(Id<Entry>),
    /// That record could not be written: the application fails.
    Unwritten,
}

/// How many times a record write the forge failed for a while goes again.
pub(crate) const RECORD_RETRIES: u32 = 3;

/// How far an entry is taken in.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum Taking {
    /// Tracked, its record not read yet.
    Reading,
    /// Asked of the hub.
    Asked,
    /// In the hub.
    Taken,
    /// The hub is done with it: it goes at the reclaim point.
    Gone,
}

/// What the hub asked of the top level for an item, and how far it has
/// come. One at a time: the hub has at most one request in flight per item.
#[derive(Debug)]
pub(crate) enum Job {
    Idle,
    /// Asking the plan what is due, after reading afresh a relation not known
    /// to be done.
    Asking {
        owner: Token,
    },
    /// Writing the record.
    /// Writing the record, tried again `retries` times so far after the
    /// forge failed for a while.
    Writing {
        owner: Token,
        retries: u32,
    },
    /// Posting the outcome the fleet's `outcome` carries.
    Recording {
        owner: Token,
        outcome: Token,
        wait: Id<Wait>,
        resumed: bool,
    },
    /// Applying an outcome, or making an action's writes.
    Applying(Box<Applying>),
    /// Preparing the run claimed, until the fleet has it.
    Starting(Box<Starting>),
}

/// An application of an outcome or an action.
#[derive(Debug)]
pub(crate) struct Applying {
    pub(crate) owner: Token,
    pub(crate) of: Of,
    pub(crate) doing: Doing,
    /// The wait for what is in flight.
    pub(crate) wait: Option<Id<Wait>>,
    /// The outcome was read from the forge rather than kept: what it creates
    /// may have been made before.
    pub(crate) resumed: bool,
    /// The number the item's next inbox event was to get as it began.
    pub(crate) news: u64,
}

/// What is applied.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum Of {
    Outcome { attempt: u64, comment: u64 },
    Action,
    Done,
}

/// How far an application has come.
#[derive(Debug)]
pub(crate) enum Doing {
    /// Reading the outcome's comment.
    Outcome,
    /// Reading the item's pull request afresh.
    Fresh,
    /// Making the writes, from the `next`.
    Writes(Box<Writes>),
}

/// The writes of an application, and what follows them.
#[derive(Debug)]
pub(crate) struct Writes {
    pub(crate) list: Box<[plan::Write]>,
    pub(crate) next: u32,
    pub(crate) then: plan::Then,
    /// The reviews on the head a merge lands, with their reviewers'
    /// permissions as read so far.
    pub(crate) reviews: List<temper_engine_model_rules::Review>,
    /// The write in flight was tried once already and timed out.
    pub(crate) retried: bool,
    /// The pull request as read afresh for a merge: where it lands, and
    /// its head and CI then.
    pub(crate) landing: Option<Fresh>,
    /// What is being read for the write in hand.
    pub(crate) reading: Option<Reading>,
}

/// A pull request read afresh before it is merged.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub(crate) struct Fresh {
    pub(crate) base: Box<[u8]>,
    pub(crate) head: [u8; 32],
    pub(crate) ci: plan::Ci,
}

/// What a merge reads before the rules say whether it lands.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum Reading {
    /// The pull request, afresh.
    Pull,
    /// A reviewer's permission.
    Permission { person: u64 },
}

/// A run being prepared.
#[derive(Debug)]
pub(crate) struct Starting {
    pub(crate) attempt: u64,
    pub(crate) run: Box<plan::Run>,
    pub(crate) brief: Option<Box<[brief::Section]>>,
    /// The brief's render in flight, and the store's get.
    pub(crate) rendering: Option<Id<Wait>>,
    pub(crate) fetching: Option<Id<Wait>>,
    pub(crate) snapshot: Option<Box<[u8]>>,
}

/// The attempt in flight.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) struct Live {
    pub(crate) attempt: u64,
    /// The inbox's last event when it was claimed: its brief had them.
    pub(crate) start: u64,
    /// Whether the fleet has it, rather than the top level preparing it.
    pub(crate) started: bool,
    /// Whether its worker bounced an event: what it took is then only what
    /// its brief had.
    pub(crate) bounced: bool,
}

/// An attempt adopted after a restart, and the inbox position its claim's
/// record carried: what an earlier life made for it (its outcome's comment,
/// its runs' comments) came after it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) struct Resumed {
    pub(crate) attempt: u64,
    pub(crate) since: u64,
}

impl Resumed {
    /// The cause of a creation the attempt `attempt` asks for again, if it
    /// is the one adopted.
    pub(crate) const fn cause(resumed: Option<Resumed>, attempt: u64) -> Option<forge::Cause> {
        match resumed {
            Some(resumed) if resumed.attempt == attempt => {
                Some(forge::Cause { comment: resumed.since, at: Time::ZERO })
            }
            Some(_) | None => None,
        }
    }
}

/// An inbox event.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) struct Noted {
    pub(crate) inbound: Inbound,
    /// The forge sub-model's number for it, if it is news.
    pub(crate) news: Option<u64>,
    pub(crate) source: plan::Source,
    pub(crate) at: Time,
    /// Whether the fleet passed it to the attempt in flight.
    pub(crate) delivered: bool,
}

/// A pull request's head as first seen.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) struct Seen {
    pub(crate) head: [u8; 32],
    pub(crate) at: Time,
    pub(crate) base: Option<[u8; 32]>,
}

impl Entry {
    /// An entry for `item`, being read.
    pub(crate) fn new(item: Item, limits: &Limits, now: Time) -> Entry {
        let inbox = limits::inbox(limits).expect("worst_case accepted the limits");
        Entry {
            item,
            taking: Taking::Reading,
            lifecycle: Lifecycle { phase: work::Phase::Waiting, attempts: 0, failures: work::Failures::NONE },
            step: None,
            staged: None,
            relations: relations(now),
            job: Job::Idle,
            due: None,
            action: None,
            asking: 0,
            live: None,
            assignment: None,
            grants: None,
            resumed: None,
            outcome: None,
            inbox: Map::with_capacity(inbox),
            next: 1,
            unkept: None,
            since: 0,
            seen: None,
            closed: false,
            blocked: false,
            merged: None,
            conflicted: None,
            wants: None,
            landed: 0,
            waiting: false,
            holding: None,
            asides: 0,
            resave: false,
            retries: 0,
            awaiting: None,
            opened: None,
        }
    }

    /// The record as its owners' parts say now, if its step is known.
    pub(crate) fn record(&self) -> Option<Record> {
        let step = self.step.as_ref()?;
        Some(Record { lifecycle: self.lifecycle, step: step.clone(), relations: self.relations.clone() })
    }
}

/// The permission of the person who accepted what is applied now: the
/// outcome posted as the comment `outcome`, or, if `None`, the step itself
/// (its run, its action). An acceptance of anything else counts for nothing:
/// what a person accepts anew, a plan proposed or growth, counts only its
/// own.
pub(crate) fn accepted(entry: &Entry, outcome: Option<u64>) -> Option<Permission> {
    if entry.relations.accepting == outcome { entry.relations.accepted } else { None }
}

/// The permission a write of what is applied now is accepted at: by an
/// acceptance of the outcome posted as the comment `outcome`, or of the step
/// itself, which counts for everything the step writes, its outcomes' writes
/// included, until it is released or done (engine-model.md, 5.1).
pub(crate) fn accepted_write(entry: &Entry, outcome: Option<u64>) -> Option<Permission> {
    accepted(entry, outcome).or(accepted(entry, None))
}

/// Relations of an item taken in at `now`, with none yet.
pub(crate) fn relations(now: Time) -> Relations {
    Relations {
        created: now,
        goal: None,
        parent: None,
        pull: None,
        branch: None,
        dependencies: Box::new([]),
        children: Box::new([]),
        decision: None,
        accepted: None,
        accepting: None,
        snapshot: false,
        spent: 0,
    }
}

/// The entry of `item`, if the top level holds it.
pub(crate) fn find(model: &Model, item: Item) -> Option<Id<Entry>> {
    model.names.get(&item).copied()
}

/// Holds `item`, being read, if there is room: the forge sub-model is asked
/// to track it. `None` if there is no room, or the item cannot be run (its
/// names do not fit a run's token).
pub(crate) fn hold(model: &mut Model, env: &Env<Limits>, item: Item) -> Option<Id<Entry>> {
    if let Some(id) = find(model, item) {
        return Some(id);
    }
    translate::run(item)?;
    let id = model.items.insert(Entry::new(item, &env.limits, env.now)).ok()?;
    if model.names.insert(item, id).is_err() {
        model.items.retire(id);
        return None;
    }
    route::forge_step(model, env, forge::Event::Track { item: translate::forge_item(item) });
    Some(id)
}

/// The forge sub-model read a tracked item: take it in as its record says.
pub(crate) fn announced(model: &mut Model, env: &Env<Limits>, item: forge::Item, view: forge::View) {
    let item = translate::item_of(item);
    let id = match find(model, item) {
        Some(id) => id,
        None => match fresh(model, env, item) {
            Some(id) => id,
            None => return untrack(model, env, item),
        },
    };
    let Some(entry) = model.items.get_mut(id) else { unreachable!("an entry named is held") };
    if entry.taking != Taking::Reading {
        return;
    }
    match view.record {
        forge::Record::Missing => match entry.step {
            Some(_) => take(model, env, id, work::Read::New),
            None => {
                model::keep(model, Fact::Untracked { item });
                drop_entry(model, id);
                untrack(model, env, item);
            }
        },
        forge::Record::Mangled { .. } => mangled(model, env, id),
        forge::Record::Found { comment, position, .. } => {
            entry.since = position.comment;
            match decoded_record(model, comment) {
                Some(record) if sound(&record, &model.config.plan, env) => {
                    let Record { lifecycle, step, relations } = record;
                    let pull = relations.pull;
                    let Some(entry) = model.items.get_mut(id) else { unreachable!("an entry named is held") };
                    entry.lifecycle = lifecycle;
                    entry.step = Some(step);
                    entry.relations = relations;
                    // The working set follows the pull request the record
                    // names, so the plan reads it as it did before the restart.
                    if let Some(pull) = pull {
                        let item = translate::forge_item(item);
                        route::forge_step(model, env, forge::Event::Link { item, pull: Some(pull) });
                    }
                    take(model, env, id, work::Read::Record(lifecycle));
                }
                Some(_) | None => mangled(model, env, id),
            }
        }
    }
}

/// Whether a record read is one the engine could have written: its step one
/// the plan could have written (`plan::check_record`), its goal's plan one
/// the plan could have made (`plan::check_goal`), and its relations within
/// the limits. One that is not is held for a person, as a record that does
/// not decode is.
fn sound(record: &Record, config: &plan::Config, env: &Env<Limits>) -> bool {
    let plan = &env.limits.plan;
    let plan_env = route::plan_env(env);
    let mut fits = limits::within(record.relations.dependencies.len(), plan.steps)
        && limits::within(record.relations.children.len(), plan.steps);
    for related in record.relations.dependencies.iter().chain(record.relations.children.iter()) {
        fits = fits && limits::within(related.name.len(), plan.name_bytes);
    }
    let step = plan::check_record(config, &plan_env, &record.step.step).is_ok();
    let goal = match &record.step.goal {
        Some(goal) => plan::check_goal(&plan_env, goal).is_ok(),
        None => true,
    };
    fits && step && goal
}

/// An entry for an item the forge sub-model announced that the top level did
/// not ask for: one it tracked before a restart. `None` if there is no room.
fn fresh(model: &mut Model, env: &Env<Limits>, item: Item) -> Option<Id<Entry>> {
    translate::run(item)?;
    let id = model.items.insert(Entry::new(item, &env.limits, env.now)).ok()?;
    if model.names.insert(item, id).is_err() {
        model.items.retire(id);
        return None;
    }
    Some(id)
}

/// The record decoded from the comment `comment` in the answer being
/// routed, if the protocol layer found it there.
fn decoded_record(model: &mut Model, comment: u64) -> Option<Record> {
    for decoded in &model.decoded {
        match decoded {
            crate::boundary::Decoded::Record { comment: found, record } if *found == comment => {
                return Some(Record::clone(record));
            }
            crate::boundary::Decoded::Record { .. }
            | crate::boundary::Decoded::Outcome { .. }
            | crate::boundary::Decoded::Page { .. } => {}
        }
    }
    None
}

/// An item whose record does not split is held for a person; its attempts
/// are counted from the outcomes posted on it, as far as the read showed
/// them.
fn mangled(model: &mut Model, env: &Env<Limits>, id: Id<Entry>) {
    let mut attempts: u64 = 0;
    for decoded in &model.decoded {
        match decoded {
            crate::boundary::Decoded::Outcome { posted, .. } => attempts = attempts.max(posted.attempt),
            crate::boundary::Decoded::Record { .. } | crate::boundary::Decoded::Page { .. } => {}
        }
    }
    let Some(entry) = model.items.get(id) else { unreachable!("an entry named is held") };
    model::keep(model, Fact::Mangled { item: entry.item });
    take(model, env, id, work::Read::Mangled { attempts });
}

/// Asks the hub to take the item in.
pub(crate) fn take(model: &mut Model, env: &Env<Limits>, id: Id<Entry>, read: work::Read) {
    let Some(entry) = model.items.get_mut(id) else { unreachable!("an entry named is held") };
    entry.taking = Taking::Asked;
    let item = entry.item;
    // An item read with a record has one written already; a new one's first
    // is written next.
    let written = match read {
        work::Read::New => false,
        work::Read::Record(_) | work::Read::Mangled { .. } => true,
    };
    let Ok(wait) = model.waits.insert(Wait::Take { entry: id, written }) else {
        unreachable!("the waits have room for every item's call")
    };
    let reply_to = ReplyTo::new(wait.token());
    route::work_step(model, env, work::Event::Take { reply_to, item, read });
}

/// The hub's answer to a `Take` the top level made, of an item whose record
/// is `written` already, or not.
pub(crate) fn taken(model: &mut Model, env: &Env<Limits>, id: Id<Entry>, written: bool, taken: bool) {
    let Some(entry) = model.items.get_mut(id) else { return };
    if taken {
        entry.taking = Taking::Taken;
        if written {
            recorded(model, id, true);
        }
        return;
    }
    // The hub refused it: full, or done already. The forge sub-model lets it
    // go too, and a listing finds it again once there is room.
    let item = entry.item;
    drop_entry(model, id);
    untrack(model, env, item);
}

/// Forgets an entry, at the reclaim point.
pub(crate) fn drop_entry(model: &mut Model, id: Id<Entry>) {
    let Some(entry) = model.items.get_mut(id) else { return };
    if entry.taking == Taking::Gone {
        return;
    }
    entry.taking = Taking::Gone;
    let item = entry.item;
    let opened = entry.opened.take();
    let holding = entry.holding.take();
    let lingers = entry.asides > 0;
    model.names.remove(&item);
    if !lingers {
        model.items.retire(id);
        // Its record is done with it: nothing more of it is written.
        growers(model, id, true);
    }
    if let Some(to) = opened {
        model.requests.push(Request::Reply { to, reply: Reply::Refused(Refusal::Failed) });
    }
    release_holder(model, holding);
}

/// The application that created an item goes on, once the item's first
/// record is written or it is let go: from the ready list.
fn release_holder(model: &mut Model, holding: Option<Id<Entry>>) {
    if let Some(parent) = holding
        && model.stalled.try_push(parent).is_err()
    {
        unreachable!("the ready list has room for every item");
    }
}

/// The item's record was written, or could not be: the person's call that
/// opened it is answered.
pub(crate) fn recorded(model: &mut Model, id: Id<Entry>, written: bool) {
    let Some(entry) = model.items.get_mut(id) else { return };
    let holding = entry.holding.take();
    let item = entry.item;
    release_holder(model, holding);
    let Some(entry) = model.items.get_mut(id) else { return };
    let Some(to) = entry.opened.take() else { return };
    let reply = if written { Reply::Opened { item } } else { Reply::Refused(Refusal::Failed) };
    model.requests.push(Request::Reply { to, reply });
}

fn untrack(model: &mut Model, env: &Env<Limits>, item: Item) {
    route::forge_step(model, env, forge::Event::Untrack { item: translate::forge_item(item) });
}

/// The forge sub-model let an item go: closed, or not on the forge. Its
/// relations learn it is done, and the hub hears of it, so its plan finds it
/// done too.
pub(crate) fn left(model: &mut Model, env: &Env<Limits>, item: forge::Item) {
    let item = translate::item_of(item);
    finished(model, env, item);
    let Some(id) = find(model, item) else { return };
    let Some(entry) = model.items.get_mut(id) else { unreachable!("an entry named is held") };
    entry.closed = true;
    match entry.taking {
        Taking::Reading => drop_entry(model, id),
        Taking::Asked | Taking::Taken => {
            notice(model, env, id, Inbound::Finished { item }, plan::Source::Own);
        }
        Taking::Gone => {}
    }
}

/// `item` is done: the items related to it learn it, each in its record and
/// its inbox.
pub(crate) fn finished(model: &mut Model, env: &Env<Limits>, item: Item) {
    let ids = related_to(model, item, &env.limits);
    for id in &ids {
        let Some(entry) = model.items.get_mut(*id) else { continue };
        let dependency = mark(&mut entry.relations.dependencies, item, env.now);
        let child = mark(&mut entry.relations.children, item, env.now);
        if !(dependency || child) {
            continue;
        }
        let source = if child { plan::Source::Child } else { plan::Source::Dependency };
        aside(model, env, *id);
        notice(model, env, *id, Inbound::Finished { item }, source);
    }
}

/// The entries related to `item`, as a dependency or a child.
fn related_to(model: &Model, item: Item, limits: &Limits) -> List<Id<Entry>> {
    let mut found = List::with_capacity(limits.work.items);
    for (_, id) in &model.names {
        let Some(entry) = model.items.get(*id) else { continue };
        let related = names(&entry.relations.dependencies, item) || names(&entry.relations.children, item);
        if related && found.push(*id).is_err() {
            break;
        }
    }
    found
}

fn names(related: &[Related], item: Item) -> bool {
    for one in related {
        if one.item == item {
            return true;
        }
    }
    false
}

/// Marks `item` done at `now` among `related`, if it is there and was not
/// done: whether it was marked.
pub(crate) fn mark(related: &mut Box<[Related]>, item: Item, now: Time) -> bool {
    let mut marked = false;
    for one in related.iter_mut() {
        if one.item == item && one.done.is_none() {
            one.done = Some(now);
            marked = true;
        }
    }
    marked
}

/// Writes the item's record on the side, as its relations changed: it is
/// composed as the call goes out, so it carries what its owners' parts say
/// then. None goes while a run's answer is handed to the hub and not yet
/// durable, since the inbox position it would carry has moved with what the
/// run took: it goes once the answer is acknowledged.
pub(crate) fn aside(model: &mut Model, env: &Env<Limits>, id: Id<Entry>) {
    let Some(entry) = model.items.get(id) else { return };
    if entry.step.is_none() || entry.closed || entry.taking != Taking::Taken {
        return;
    }
    let handed = handing(model, entry.item);
    let item = translate::forge_item(entry.item);
    let Some(entry) = model.items.get_mut(id) else { return };
    // One at a time: the one in flight is followed by another once it ends.
    if handed || entry.asides > 0 {
        entry.resave = true;
        return;
    }
    entry.resave = false;
    entry.asides = entry.asides.saturating_add(1);
    let Ok(wait) = model.waits.insert(Wait::Record { entry: id }) else {
        unreachable!("the waits have room for every item's write")
    };
    let owner = wait.token();
    let write = forge::Write::Record { item, payload: owner };
    route::forge_step(model, env, forge::Event::Write { owner, write, resumed: None });
}

/// Whether an answer of the item's runs is handed to the hub and not yet
/// acknowledged.
fn handing(model: &Model, item: Item) -> bool {
    for (handed, _) in &model.handed {
        if handed.0 == item {
            return true;
        }
    }
    false
}

/// A record written on the side ended. One the forge had no room for goes
/// again from the ready list, and so does one the forge failed for a while,
/// as often as a record the hub writes would; one that failed otherwise is
/// carried by the item's next record. Once the record is as last changed,
/// or cannot be, the applications whose growth waits for it go on. An entry
/// the hub is done with goes once its last is out.
pub(crate) fn aside_written(model: &mut Model, id: Id<Entry>, result: Result<forge::Written, forge::Failure>) {
    let Some(entry) = model.items.get_mut(id) else { return };
    entry.asides = entry.asides.saturating_sub(1);
    let gone = entry.taking == Taking::Gone;
    let (again, written) = match result {
        Ok(_) => (false, true),
        Err(forge::Failure::Busy) => (true, false),
        Err(forge::Failure::Forge(api::Error::Timeout | api::Error::Unavailable | api::Error::RateLimited { .. }))
            if entry.retries < RECORD_RETRIES =>
        {
            entry.retries = entry.retries.saturating_add(1);
            (true, false)
        }
        Err(_) => (false, false),
    };
    if !again {
        entry.retries = 0;
    }
    if !gone && (again || entry.resave) {
        entry.resave = true;
        if model.resaves.try_push(id).is_err() {
            unreachable!("the ready list has room for every item's record");
        }
        return;
    }
    if gone && entry.asides == 0 {
        model.items.retire(id);
    }
    growers(model, id, written);
}

/// Whether a record of the item is written on the side, or waits to be.
pub(crate) fn saving(entry: &Entry) -> bool {
    entry.asides > 0 || entry.resave
}

/// The applications whose growth waits for the record of `goal` go on, from
/// the ready list: they fail if it was not `written`.
fn growers(model: &mut Model, goal: Id<Entry>, written: bool) {
    let mut found = List::with_capacity(model.names.len());
    for (_, id) in &model.names {
        let Some(entry) = model.items.get(*id) else { continue };
        if entry.awaiting == Some(Awaiting::Goal(goal)) && found.push(*id).is_err() {
            break;
        }
    }
    for id in &found {
        let Some(entry) = model.items.get_mut(*id) else { continue };
        entry.awaiting = if written { None } else { Some(Awaiting::Unwritten) };
        if model.stalled.try_push(*id).is_err() {
            unreachable!("the ready list has room for every item");
        }
    }
}

/// Puts `inbound` in the item's inbox, and tells the hub: relayed to its run
/// if one is in flight, or waking it as its step's wake rule says.
pub(crate) fn notice(model: &mut Model, env: &Env<Limits>, id: Id<Entry>, inbound: Inbound, source: plan::Source) {
    inbox(model, env, id, inbound, None, source);
}

/// Puts an inbox event in the item's inbox, numbered, and tells the hub.
pub(crate) fn inbox(
    model: &mut Model,
    env: &Env<Limits>,
    id: Id<Entry>,
    inbound: Inbound,
    news: Option<u64>,
    source: plan::Source,
) {
    let Some(entry) = model.items.get_mut(id) else { return };
    let kept = match news {
        // News after news the inbox had no room for waits on the forge with
        // it, to be told again in order.
        Some(number) => match entry.unkept {
            Some(unkept) => number < unkept,
            None => true,
        },
        None => noticed(entry, inbound, &env.limits),
    };
    if !kept {
        return;
    }
    let seq = entry.next;
    entry.next = entry.next.saturating_add(1);
    entry.blocked = false;
    let noted = Noted { inbound, news, source, at: env.now, delivered: false };
    if entry.inbox.insert(seq, noted).is_err() {
        // Notices keep to their share, so the news the forge sub-model holds
        // of an item fits beside them. News that still finds no room (were
        // the forge sub-model to hold more than it says) stays on the forge:
        // the inbox position stays before it, and once a run's answer has
        // made room, the forge sub-model tells it again (`took`).
        if let Some(number) = news {
            entry.unkept = Some(match entry.unkept {
                Some(unkept) => unkept.min(number),
                None => number,
            });
        }
        return;
    }
    if entry.taking != Taking::Taken {
        return;
    }
    let item = entry.item;
    let wake = wake(entry, env);
    route::work_step(model, env, work::Event::Inbox { item, event: Token::new(seq), wake });
}

/// Makes room for a notice of the item's relations, merging it with one
/// alike, whether a run was given that one or not: whether it is to be put
/// in. A related item done, or held, is noticed once until a run is given
/// it, and again once one was; a decision replaces the one before. So the
/// notices keep to their share of the inbox (`limits::notices`), and one
/// beyond it is dropped, never news.
fn noticed(entry: &mut Entry, inbound: Inbound, limits: &Limits) -> bool {
    let mut earlier: Option<u64> = None;
    let mut given = false;
    let mut notices: u32 = 0;
    for (seq, noted) in &entry.inbox {
        if noted.news.is_some() {
            continue;
        }
        notices = notices.saturating_add(1);
        let alike = match noted.inbound {
            Inbound::Decided { .. } => match inbound {
                Inbound::Decided { .. } => true,
                Inbound::News(_) | Inbound::Finished { .. } | Inbound::Held { .. } => false,
            },
            Inbound::News(_) | Inbound::Finished { .. } | Inbound::Held { .. } => noted.inbound == inbound,
        };
        if alike {
            earlier = Some(*seq);
            given = noted.delivered;
        }
    }
    let Some(seq) = earlier else {
        return match limits::notices(limits) {
            Some(share) => notices < share,
            None => false,
        };
    };
    let replaces = given
        || match inbound {
            Inbound::Decided { .. } => true,
            Inbound::News(_) | Inbound::Finished { .. } | Inbound::Held { .. } => false,
        };
    if replaces {
        entry.inbox.remove(&seq);
    }
    replaces
}

/// When the item's inbox wakes it: a session's wake rule says, from what its
/// inbox holds; every other step is asked what is due whenever what it
/// reads changes.
pub(crate) fn wake(entry: &Entry, env: &Env<Limits>) -> Option<Time> {
    let Some(record) = entry.step.as_ref() else { return Some(env.now) };
    match &record.step.work {
        plan::Work::Session(spec) => {
            let inbound = pending(entry, &env.limits);
            let last = match record.progress.last_run {
                Some(at) => at,
                None => entry.relations.created,
            };
            let plan_env = route::plan_env(env);
            match plan::wake(&plan_env, &spec.wake, inbound.as_slice(), last) {
                plan::Woken::Now => Some(env.now),
                plan::Woken::At(at) => Some(at),
                plan::Woken::No => None,
            }
        }
        plan::Work::Agent(_) | plan::Work::Change(_) | plan::Work::Wait(_) => Some(env.now),
    }
}

/// The inbox events no run has taken, as the plan reads them.
pub(crate) fn pending(entry: &Entry, limits: &Limits) -> List<plan::Inbound> {
    let mut found = List::with_capacity(limits::inbox(limits).expect("worst_case accepted the limits"));
    for (_, noted) in &entry.inbox {
        if found.push(plan::Inbound { source: noted.source, at: noted.at }).is_err() {
            break;
        }
    }
    found
}

/// What the attempt in flight took of the inbox, as its answer came: what
/// was there when it was claimed, and what was delivered to it in order
/// after, unless its worker bounced one. Those go; the inbox position moves
/// past the forge's news among them.
pub(crate) fn took(model: &mut Model, env: &Env<Limits>, id: Id<Entry>) {
    let Some(entry) = model.items.get_mut(id) else { return };
    let Some(live) = entry.live else { return };
    let mut through = live.start;
    if !live.bounced {
        for (seq, noted) in &entry.inbox {
            if *seq <= live.start {
                continue;
            }
            if !noted.delivered {
                break;
            }
            through = *seq;
        }
    }
    let mut news: Option<u64> = None;
    let mut comment = entry.since;
    for _ in 0..entry.inbox.len() {
        let Some((seq, noted)) = entry.inbox.first() else { break };
        if *seq > through {
            break;
        }
        let seq = *seq;
        if let Some(number) = noted.news {
            news = Some(number);
        }
        match noted.inbound {
            Inbound::News(forge::News::Comment { id: taken, .. }) => comment = comment.max(taken),
            Inbound::News(forge::News::Reviews { .. } | forge::News::Pull { .. })
            | Inbound::Finished { .. }
            | Inbound::Held { .. }
            | Inbound::Decided { .. } => {}
        }
        entry.inbox.remove(&seq);
    }
    entry.since = comment;
    let item = translate::forge_item(entry.item);
    // News the inbox had no room for is told again, now that the answer
    // made room, before the position moves: what the forge sub-model tells
    // as it moves comes after it.
    if let Some(from) = entry.unkept.take() {
        route::forge_step(model, env, forge::Event::Retell { item, from });
    }
    if let Some(through) = news {
        route::forge_step(model, env, forge::Event::Took { item, through });
    }
}

/// The application in hand, if the item's job is one.
pub(crate) const fn applying(job: &Job) -> Option<&Applying> {
    match job {
        Job::Applying(applying) => Some(applying),
        Job::Idle | Job::Asking { .. } | Job::Writing { .. } | Job::Recording { .. } | Job::Starting(_) => None,
    }
}

pub(crate) const fn applying_mut(job: &mut Job) -> Option<&mut Applying> {
    match job {
        Job::Applying(applying) => Some(applying),
        Job::Idle | Job::Asking { .. } | Job::Writing { .. } | Job::Recording { .. } | Job::Starting(_) => None,
    }
}

/// The run being prepared, if the item's job is one.
pub(crate) const fn starting_mut(job: &mut Job) -> Option<&mut Starting> {
    match job {
        Job::Starting(starting) => Some(starting),
        Job::Idle | Job::Asking { .. } | Job::Writing { .. } | Job::Recording { .. } | Job::Applying(_) => None,
    }
}

/// The writes of an application, once it makes them.
pub(crate) const fn writes(doing: &Doing) -> Option<&Writes> {
    match doing {
        Doing::Writes(writes) => Some(writes),
        Doing::Outcome | Doing::Fresh => None,
    }
}

pub(crate) const fn writes_mut(doing: &mut Doing) -> Option<&mut Writes> {
    match doing {
        Doing::Writes(writes) => Some(writes),
        Doing::Outcome | Doing::Fresh => None,
    }
}

/// The comment of the outcome an application applies, if it applies one.
pub(crate) const fn comment_of(of: Of) -> Option<u64> {
    match of {
        Of::Outcome { comment, .. } => Some(comment),
        Of::Action | Of::Done => None,
    }
}
