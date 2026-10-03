//! What the hub asks of the top level for an item (engine-model.md, 4.2 to
//! 4.5), each through the plan, the rules and the forge, one at a time per
//! item:
//!
//! - **What is due:** the relations not known to be done and not held are
//!   read afresh, one at a time; then the plan decides from the facts the
//!   working set and the record hold, and the hub hears it at once. A run
//!   that is due comes with the plan's progress write, made into the step
//!   so the claim's record carries it; an action's writes are kept until
//!   the hub asks for them.
//! - **The record:** composed as the write goes out, from the hub's
//!   lifecycle, the plan's step as committed and the relations. An item the
//!   forge no longer shows open has nothing left to write: its record is
//!   done with it.
//! - **The outcome:** posted on the item, keyed by its attempt, before it is
//!   applied (4.4).
//! - **An application:** the outcome as kept, or read from its comment; the
//!   item's pull request read afresh; the plan's writes, or that it is stale
//!   or invalid; each write checked by the rules and made in order, keyed by
//!   the outcome; the plan's own parts staged, and committed once every
//!   write is made, so the record's update, which the hub writes next, is
//!   the commit point. An engine action's writes are made the same way.
//!
//! A forge op refused as busy waits on the ready list and goes again; one
//! that may have been made, timed out, is asked for again once, resumed, so
//! the forge looks for what it created first.

use alloc::boxed::Box;
use core::mem;

use temper_engine_model_forge::{self as forge, api};
use temper_engine_model_plan as plan;
use temper_engine_model_rules as rules;
use temper_engine_model_work as work;
use temper_lib::bytes::copy_of;
use temper_lib::{Env, Id, List, Queue, Time, Token};

use crate::boundary::{Inbound, Item, Phase, Related};
use crate::facts::Fact;
use crate::items::{self, Applying, Doing, Entry, Job, Of, Writes};
use crate::limits::{self, Limits};
use crate::model::{self, Model};
use crate::route;
use crate::translate;
use crate::waits::Wait;

/// The hub asks what is due for `item`.
pub(crate) fn due(model: &mut Model, env: &Env<Limits>, owner: Token, item: Item) {
    let id = held(model, item);
    let entry = get_mut(model, id);
    entry.job = Job::Asking { owner };
    let token = translate::run_of(item);
    route::views_step(
        model,
        env,
        temper_engine_model_views::Event::Phase { item: token, repository: item.repository, phase: Phase::Due.code() },
    );
    ask(model, env, id, 0);
}

/// Reads afresh the first relation from `from` on that is not known to be
/// done and not held, or decides once there is none.
fn ask(model: &mut Model, env: &Env<Limits>, id: Id<Entry>, from: u32) {
    let entry = get(model, id);
    let mut found: Option<(u32, Item)> = None;
    let mut index: u32 = 0;
    for related in entry.relations.dependencies.iter().chain(entry.relations.children.iter()) {
        if index >= from && related.done.is_none() && !model.names.contains_key(&related.item) {
            found = Some((index, related.item));
            break;
        }
        index = index.saturating_add(1);
    }
    let Some((index, related)) = found else { return decide(model, env, id) };
    let Ok(wait) = model.waits.insert(Wait::Job { entry: id }) else {
        unreachable!("the waits have room for every item's job")
    };
    let read = forge::Read::Item { item: translate::forge_item(related), after: u64::MAX };
    asking_at(model, id, index);
    route::forge_step(model, env, forge::Event::Read { owner: wait.token(), read });
}

/// Remembers which relation an asking item reads, so the next starts after
/// it.
fn asking_at(model: &mut Model, id: Id<Entry>, index: u32) {
    get_mut(model, id).asking = index;
}

/// A relation read afresh: done if the forge shows it closed, or gone.
fn related(model: &mut Model, env: &Env<Limits>, id: Id<Entry>, result: Result<api::Answer, forge::Failure>) {
    let entry = get(model, id);
    let index = entry.asking;
    let Some(related) = nth(entry, index) else { return ask(model, env, id, index.saturating_add(1)) };
    let item = related.item;
    let done = match result {
        Ok(api::Answer::Item { item: summary, .. }) => summary.state == api::State::Closed,
        Err(forge::Failure::Forge(api::Error::Missing)) => true,
        Err(forge::Failure::Busy) => return stall(model, id),
        Ok(_) | Err(_) => false,
    };
    if done {
        let entry = get_mut(model, id);
        let dependency = items::mark(&mut entry.relations.dependencies, item, env.now);
        let child = items::mark(&mut entry.relations.children, item, env.now);
        items::aside(model, env, id);
        // Its end may have been noticed by a life that did not live to write
        // it: it is news again, as the plan's wake rule reads news.
        if dependency || child {
            let source = if child { plan::Source::Child } else { plan::Source::Dependency };
            items::notice(model, env, id, Inbound::Finished { item }, source);
        }
    }
    ask(model, env, id, index.saturating_add(1));
}

/// The `index`th of the item's relations, its dependencies first.
fn nth(entry: &Entry, index: u32) -> Option<&Related> {
    let index = usize::try_from(index).ok()?;
    let dependencies = entry.relations.dependencies.len();
    match index.checked_sub(dependencies) {
        Some(child) => entry.relations.children.get(child),
        None => entry.relations.dependencies.get(index),
    }
}

/// Asks the plan what is due, and tells the hub.
fn decide(model: &mut Model, env: &Env<Limits>, id: Id<Entry>) {
    let entry = get_mut(model, id);
    let owner = match mem::replace(&mut entry.job, Job::Idle) {
        Job::Asking { owner } => owner,
        Job::Idle | Job::Writing { .. } | Job::Recording { .. } | Job::Applying(_) | Job::Starting(_) => {
            unreachable!("an item asking decides")
        }
    };
    // A pull request opened and not read yet: the forge's news of it wakes
    // the item, so the plan never decides on a change it cannot see.
    let blocked = entry.blocked;
    let entry = get(model, id);
    let unread = entry.relations.pull.is_some() && model.forge.pull(translate::forge_item(entry.item)).is_none();
    if blocked || unread {
        let due = work::Due::Nothing { until: None };
        return route::work_step(model, env, work::Event::Decided { owner, due });
    }
    let entry = get(model, id);
    let Some(record) = entry.step.as_ref() else {
        let due = work::Due::Hold { reason: translate::NO_STEP };
        return route::work_step(model, env, work::Event::Decided { owner, due });
    };
    let facts = facts(model, env, entry);
    let mut writes = Queue::with_capacity(plan::max_out(&env.limits.plan));
    let decided = plan::due(&model.config.plan, &route::plan_env(env), record, &facts, &mut writes);
    let token = id.token();
    let due = match decided {
        plan::Due::Nothing { waits: _, until } => work::Due::Nothing { until },
        // A run the rules want a person to accept, or refuse, is held before
        // it is claimed (engine-model.md, section 7).
        plan::Due::Run(run) => match crate::runs::rule(model, env, id, &run) {
            rules::Decision::Accept { permission } => {
                get_mut(model, id).wants = Some(permission);
                model::keep(model, Fact::Ruled { item: get(model, id).item, refused: false });
                work::Due::Hold { reason: translate::RUN_ACCEPTANCE }
            }
            rules::Decision::Refuse => {
                model::keep(model, Fact::Ruled { item: get(model, id).item, refused: true });
                work::Due::Hold { reason: translate::RUN_REFUSED }
            }
            rules::Decision::Allow | rules::Decision::Wait => {
                let entry = get_mut(model, id);
                commit(entry, &mut writes);
                entry.due = Some(Box::new(run));
                work::Due::Run { run: token }
            }
        },
        plan::Due::Act(_) => {
            get_mut(model, id).action = Some((Of::Action, drain(&mut writes, &env.limits)));
            work::Due::Act { action: token }
        }
        plan::Due::Done => {
            get_mut(model, id).action = Some((Of::Done, drain(&mut writes, &env.limits)));
            work::Due::Done { action: token }
        }
        plan::Due::Hold(why) => {
            escalate(model, env, id, why);
            work::Due::Hold { reason: translate::hold(why) }
        }
    };
    route::work_step(model, env, work::Event::Decided { owner, due });
}

/// An item held for an escalation or a stall tells its goal's session first
/// (engine-model.md, section 6).
fn escalate(model: &mut Model, env: &Env<Limits>, id: Id<Entry>, why: plan::Hold) {
    match why {
        plan::Hold::Escalated | plan::Hold::Stalled => {}
        plan::Hold::Rejected | plan::Hold::Repairs | plan::Hold::Rebases | plan::Hold::PullClosed => return,
    }
    let entry = get(model, id);
    let item = entry.item;
    let Some(goal) = entry.relations.goal else { return };
    let Some(goal) = items::find(model, goal) else { return };
    items::notice(model, env, goal, Inbound::Held { item }, plan::Source::Child);
}

/// Makes the plan's progress writes that go with a claim into the item's
/// step.
fn commit(entry: &mut Entry, writes: &mut Queue<plan::Write>) {
    for _ in 0..writes.len() {
        let Some(write) = writes.pop() else { break };
        match write {
            plan::Write::Progress(progress) => {
                if let Some(record) = entry.step.as_mut() {
                    record.progress = progress;
                }
            }
            plan::Write::Create { .. }
            | plan::Write::OpenPull { .. }
            | plan::Write::ReopenPull
            | plan::Write::Merge { .. }
            | plan::Write::Close
            | plan::Write::DeleteBranch
            | plan::Write::Goal(_)
            | plan::Write::Release { .. } => {}
        }
    }
}

/// The writes in `writes`, in order.
fn drain(writes: &mut Queue<plan::Write>, limits: &Limits) -> Box<[plan::Write]> {
    let mut list = List::with_capacity(plan::max_out(&limits.plan));
    for _ in 0..writes.len() {
        let Some(write) = writes.pop() else { break };
        if list.push(write).is_err() {
            break;
        }
    }
    list.into_boxed()
}

/// What the plan reads of the item (seams: "Plans"): its relations and its
/// decision from its record, its pull request from the working set, and its
/// inbox.
pub(crate) fn facts(model: &Model, env: &Env<Limits>, entry: &Entry) -> plan::Facts {
    let relations = &entry.relations;
    let pull = match relations.pull {
        Some(_) => match model.forge.pull(translate::forge_item(entry.item)) {
            Some(level) => {
                let reviews = model.forge.reviews(translate::forge_item(entry.item));
                let mut pull = translate::pull(level, reviews, entry.seen);
                if entry.merged.is_some() {
                    pull.state = plan::PullState::Merged;
                }
                if entry.conflicted == Some(pull.head.0) {
                    pull.merge = plan::Mergeable::Conflicts;
                }
                Some(pull)
            }
            None => None,
        },
        None => None,
    };
    plan::Facts {
        created: relations.created,
        dependencies: count(&relations.dependencies),
        children: count(&relations.children),
        branch: head(relations.branch),
        pull,
        decision: relations.decision,
        closed: entry.closed,
        snapshot: relations.snapshot,
        woken: woken(entry, env),
    }
}

/// Whether the item's inbox wakes it now: a session's wake rule says; any
/// other step is woken by anything in it.
fn woken(entry: &Entry, env: &Env<Limits>) -> bool {
    let session = match entry.step.as_ref() {
        Some(record) => match record.step.work {
            plan::Work::Session(_) => true,
            plan::Work::Agent(_) | plan::Work::Change(_) | plan::Work::Wait(_) => false,
        },
        None => false,
    };
    if !session {
        return !entry.inbox.is_empty();
    }
    match items::wake(entry, env) {
        Some(at) => at <= env.now,
        None => false,
    }
}

fn head(head: Option<[u8; 32]>) -> Option<plan::Commit> {
    let head = head?;
    Some(plan::Commit(head))
}

fn count(related: &[Related]) -> plan::Relations {
    let mut relations = plan::Relations::NONE;
    for one in related {
        relations.total = relations.total.saturating_add(1);
        if let Some(at) = one.done {
            relations.done = relations.done.saturating_add(1);
            relations.last_done = Some(match relations.last_done {
                Some(last) if last > at => last,
                Some(_) | None => at,
            });
        }
    }
    relations
}

/// The hub writes the item's record: its part is `lifecycle`.
pub(crate) fn write(model: &mut Model, env: &Env<Limits>, owner: Token, item: Item, lifecycle: work::Lifecycle) {
    let id = held(model, item);
    let entry = get_mut(model, id);
    entry.lifecycle = lifecycle;
    let closed = entry.closed;
    let known = entry.step.is_some();
    let phase = translate::phase(lifecycle.phase);
    let token = translate::run_of(item);
    let notice =
        temper_engine_model_views::Event::Phase { item: token, repository: item.repository, phase: phase.code() };
    route::views_step(model, env, notice);
    if closed {
        // The forge shows the item closed: its record is done with it.
        items::recorded(model, id, true);
        let wrote = work::Wrote::Done;
        return route::work_step(model, env, work::Event::Written { owner, wrote });
    }
    if !known {
        items::recorded(model, id, false);
        let wrote = work::Wrote::Failed;
        return route::work_step(model, env, work::Event::Written { owner, wrote });
    }
    let Ok(wait) = model.waits.insert(Wait::Job { entry: id }) else {
        unreachable!("the waits have room for every item's job")
    };
    get_mut(model, id).job = Job::Writing { owner, retries: 0 };
    write_record(model, env, id, wait);
}

fn write_record(model: &mut Model, env: &Env<Limits>, id: Id<Entry>, wait: Id<Wait>) {
    let item = translate::forge_item(get(model, id).item);
    let owner = wait.token();
    let write = forge::Write::Record { item, payload: owner };
    route::forge_step(model, env, forge::Event::Write { owner, write, resumed: None });
}

/// The hub posts the outcome of the item's attempt `attempt`, which the
/// fleet's answer `outcome` carries.
pub(crate) fn record(model: &mut Model, env: &Env<Limits>, owner: Token, item: Item, attempt: u64, outcome: Token) {
    let id = held(model, item);
    let Ok(wait) = model.waits.insert(Wait::Job { entry: id }) else {
        unreachable!("the waits have room for every item's job")
    };
    get_mut(model, id).job = Job::Recording { owner, outcome, wait, resumed: false };
    post(model, env, id, wait, attempt, false);
}

/// Posts the outcome, keyed by its attempt. One of an attempt adopted after
/// a restart may have been posted by the earlier life, after its claim's
/// position; one that timed out, by this one: each is looked for first.
fn post(model: &mut Model, env: &Env<Limits>, id: Id<Entry>, wait: Id<Wait>, attempt: u64, retried: bool) {
    let entry = get(model, id);
    let item = translate::forge_item(entry.item);
    let resumed = match items::Resumed::cause(entry.resumed, attempt) {
        Some(cause) => Some(cause),
        None if retried => Some(forge::Cause { comment: entry.since, at: Time::ZERO }),
        None => None,
    };
    let owner = wait.token();
    let key = translate::concat(&[b"outcome/", &translate::decimal(attempt)]);
    let write = forge::Write::Comment { item, key, person: None, body: forge::Content::Payload(owner) };
    route::forge_step(model, env, forge::Event::Write { owner, write, resumed });
}

/// The hub applies the outcome of the item's attempt `attempt`, posted as
/// the comment `comment`.
pub(crate) fn apply(model: &mut Model, env: &Env<Limits>, owner: Token, item: Item, attempt: u64, comment: u64) {
    let id = held(model, item);
    let entry = get_mut(model, id);
    let kept = match &entry.outcome {
        Some((posted, _)) => *posted == comment,
        None => false,
    };
    let of = Of::Outcome { attempt, comment };
    let doing = if kept { Doing::Fresh } else { Doing::Outcome };
    entry.staged = entry.step.clone();
    let news = entry.next;
    entry.job = Job::Applying(Box::new(Applying { owner, of, doing, wait: None, resumed: !kept, news }));
    go(model, env, id);
}

/// The hub makes the writes of the action it was told is due.
pub(crate) fn act(model: &mut Model, env: &Env<Limits>, owner: Token, item: Item, action: Token) {
    let id = held(model, item);
    assert!(action == id.token(), "an action is the one decided for its item");
    let entry = get_mut(model, id);
    let Some((of, writes)) = entry.action.take() else { unreachable!("the hub acts on the action it was told") };
    entry.staged = entry.step.clone();
    let writes = Writes {
        list: writes,
        next: 0,
        then: plan::Then::Wait,
        reviews: List::with_capacity(env.limits.forge.reviewers),
        retried: false,
        landing: None,
        reading: None,
    };
    let doing = Doing::Writes(Box::new(writes));
    let news = entry.next;
    entry.job = Job::Applying(Box::new(Applying { owner, of, doing, wait: None, resumed: false, news }));
    go(model, env, id);
}

/// Goes on with an application from where it is: what it reads next, or the
/// writes from the next.
fn go(model: &mut Model, env: &Env<Limits>, id: Id<Entry>) {
    let Some(applying) = items::applying(&get(model, id).job) else { unreachable!("an item applying goes on") };
    match &applying.doing {
        Doing::Outcome => {
            let Some(comment) = items::comment_of(applying.of) else { unreachable!("only an outcome is read") };
            let item = translate::forge_item(get(model, id).item);
            let read = forge::Read::Item { item, after: comment.saturating_sub(1) };
            read_for(model, env, id, read);
        }
        Doing::Fresh => {
            let entry = get(model, id);
            let Some(pull) = entry.relations.pull else { return applied(model, env, id, None) };
            let read = forge::Read::Pull { item: forge::Item { repository: entry.item.repository, number: pull } };
            read_for(model, env, id, read);
        }
        Doing::Writes(_) => next(model, env, id),
    }
}

fn read_for(model: &mut Model, env: &Env<Limits>, id: Id<Entry>, read: forge::Read) {
    let Ok(wait) = model.waits.insert(Wait::Job { entry: id }) else {
        unreachable!("the waits have room for every item's job")
    };
    if let Some(applying) = items::applying_mut(&mut get_mut(model, id).job) {
        applying.wait = Some(wait);
    }
    route::forge_step(model, env, forge::Event::Read { owner: wait.token(), read });
}

/// The pull request read afresh, if the item has one: the plan says what the
/// outcome writes.
fn applied(model: &mut Model, env: &Env<Limits>, id: Id<Entry>, fresh: Option<plan::Pull>) {
    let entry = get(model, id);
    let Some(applying) = items::applying(&entry.job) else { unreachable!("an item applying asks the plan") };
    assert!(items::comment_of(applying.of).is_some(), "an action's writes are decided already");
    let Some(record) = entry.staged.as_ref() else { return finish(model, env, id, Finish::Failed) };
    let Some((_, posted)) = entry.outcome.as_ref() else { return finish(model, env, id, Finish::Failed) };
    let outcome = translate::outcome(posted, record);
    let mut facts = facts(model, env, entry);
    if fresh.is_some() {
        facts.pull = fresh;
    }
    let goal = goal_of(model, entry);
    let goal = goal.as_ref();
    let mut writes = Queue::with_capacity(plan::max_out(&env.limits.plan));
    let decided = plan::apply(&model.config.plan, &route::plan_env(env), record, goal, &facts, &outcome, &mut writes);
    // What the outcome makes, for the rules on plans: a plan proposed, the
    // steps it grows its goal by, or the tasks it creates. Where a plan's
    // changes land is accepted with it; growth lands within the envelope
    // accepted, and a task's change lands only as the rules let a merge:
    // their size and spend are checked, not where they land.
    let made = match &outcome {
        plan::Outcome::Plan(proposed) => Some((proposed.steps.clone(), true, rules::Goal::Outside)),
        plan::Outcome::Steps(steps) => Some((steps.clone(), false, goal_spent(model, entry))),
        plan::Outcome::Tasks(tasks) => Some((tasks.clone(), false, rules::Goal::Outside)),
        plan::Outcome::Change { .. }
        | plan::Outcome::Verdict { .. }
        | plan::Outcome::Report
        | plan::Outcome::Reply
        | plan::Outcome::Finished
        | plan::Outcome::Release { .. }
        | plan::Outcome::Escalation => None,
    };
    match decided {
        plan::Applied::Writes { accept, then, estimate } => {
            let rejected = rejected(entry);
            let person = match accept {
                plan::Accept::Person => items::accepted(entry, items::comment_of(applying.of)).is_none(),
                plan::Accept::Rules => false,
            };
            if person && rejected {
                return reject(model, env, id);
            }
            if person {
                get_mut(model, id).wants = Some(model.config.rules.plan_acceptance);
                return finish(model, env, id, Finish::Accepting);
            }
            if let Some((steps, lands, goal)) = made {
                match rule_plan(model, env, id, &steps, lands, estimate, goal) {
                    rules::Decision::Allow => {}
                    rules::Decision::Accept { .. } if rejected => return reject(model, env, id),
                    rules::Decision::Accept { permission } => {
                        get_mut(model, id).wants = Some(permission);
                        return finish(model, env, id, Finish::Accepting);
                    }
                    rules::Decision::Wait | rules::Decision::Refuse => return finish(model, env, id, Finish::Failed),
                }
            }
            let writes = Writes {
                list: drain(&mut writes, &env.limits),
                next: 0,
                then,
                reviews: List::with_capacity(env.limits.forge.reviewers),
                retried: false,
                landing: None,
                reading: None,
            };
            if let Some(applying) = items::applying_mut(&mut get_mut(model, id).job) {
                applying.doing = Doing::Writes(Box::new(writes));
            }
            next(model, env, id);
        }
        plan::Applied::Stale(_) => finish(model, env, id, Finish::Stale),
        plan::Applied::Invalid(_) => finish(model, env, id, Finish::Invalid),
    }
}

/// Whether a person rejected what the item holds for them.
fn rejected(entry: &Entry) -> bool {
    match entry.relations.decision {
        Some(decided) => decided.decision == plan::Decision::Rejected,
        None => false,
    }
}

/// A person rejected what the item held for them: the plan says what that
/// writes (engine-model.md, 5.2), and nothing of the outcome is applied.
fn reject(model: &mut Model, env: &Env<Limits>, id: Id<Entry>) {
    let entry = get_mut(model, id);
    let Some(record) = entry.staged.as_ref() else { return finish(model, env, id, Finish::Failed) };
    let mut writes = Queue::with_capacity(plan::max_out(&env.limits.plan));
    let then = plan::rejected(&route::plan_env(env), record, &mut writes);
    let mut staged = entry.staged.take();
    if let Some(staged) = staged.as_mut() {
        stage(staged, &mut writes);
    }
    entry.staged = staged;
    entry.relations.decision = None;
    entry.relations.accepted = None;
    entry.relations.accepting = None;
    let writes = Writes {
        list: Box::new([]),
        next: 0,
        then,
        reviews: List::with_capacity(env.limits.forge.reviewers),
        retried: false,
        landing: None,
        reading: None,
    };
    if let Some(applying) = items::applying_mut(&mut entry.job) {
        applying.doing = Doing::Writes(Box::new(writes));
    }
    finish(model, env, id, Finish::Made);
}

/// Makes the step's own writes in `writes` into `staged`.
fn stage(staged: &mut plan::Record, writes: &mut Queue<plan::Write>) {
    for _ in 0..writes.len() {
        let Some(write) = writes.pop() else { break };
        match write {
            plan::Write::Progress(progress) => staged.progress = progress,
            plan::Write::Goal(goal) => staged.goal = Some(goal),
            plan::Write::Create { .. }
            | plan::Write::OpenPull { .. }
            | plan::Write::ReopenPull
            | plan::Write::Merge { .. }
            | plan::Write::Close
            | plan::Write::DeleteBranch
            | plan::Write::Release { .. } => {}
        }
    }
}

/// The goal's part of the record of the goal the item is under, or of its
/// own if it is one.
fn goal_of(model: &Model, entry: &Entry) -> Option<plan::Goal> {
    if let Some(record) = entry.staged.as_ref()
        && let Some(goal) = record.goal.as_ref()
    {
        return Some(goal.clone());
    }
    let goal = items::find(model, entry.relations.goal?)?;
    Some(model.items.get(goal)?.step.as_ref()?.goal.as_ref()?.clone())
}

/// What the runs of the goal the item's outcome grows have spent: its own,
/// if it proposed the plan or supervises it, else its goal's.
fn goal_spent(model: &Model, entry: &Entry) -> rules::Goal {
    let own = match entry.staged.as_ref() {
        Some(staged) => staged.goal.is_some() || proposes(entry),
        None => false,
    };
    if own {
        return rules::Goal::Spent(entry.relations.spent);
    }
    let Some(goal) = entry.relations.goal else { return rules::Goal::Outside };
    match items::find(model, goal) {
        Some(goal) => rules::Goal::Spent(get(model, goal).relations.spent),
        None => rules::Goal::Spent(0),
    }
}

/// The rules on the steps an outcome makes (a plan proposed, the steps it
/// grows its goal by, the tasks it creates), before any of their items is
/// made.
fn rule_plan(
    model: &Model,
    env: &Env<Limits>,
    id: Id<Entry>,
    steps: &[plan::Step],
    landing: bool,
    estimate: u64,
    goal: rules::Goal,
) -> rules::Decision {
    let entry = get(model, id);
    let mut lands = List::with_capacity(u32::try_from(steps.len()).unwrap_or(0));
    for step in steps {
        if !landing {
            break;
        }
        let base = match &step.work {
            plan::Work::Change(change) => &change.base,
            plan::Work::Agent(_) | plan::Work::Wait(_) | plan::Work::Session(_) => continue,
        };
        let target = rules::Target { repository: translate::repository(step.repository.0), branch: copy_of(base) };
        if lands.push(target).is_err() {
            break;
        }
    }
    let write = rules::Write::Plan(rules::Plan {
        repository: translate::repository(entry.item.repository),
        steps: u32::try_from(steps.len()).unwrap_or(u32::MAX),
        spend: estimate,
        lands: lands.into_boxed(),
        goal,
    });
    let gates = gates(entry);
    let accepted = match items::applying(&entry.job) {
        Some(applying) => items::accepted(entry, items::comment_of(applying.of)),
        None => None,
    };
    let mut findings = Queue::with_capacity(rules::max_out(&env.limits.rules));
    rules::check_write(&model.config.rules, &env.limits.rules, &write, accepted, gates.as_slice(), &mut findings)
}

fn gates(entry: &Entry) -> List<rules::Gate> {
    match entry.step.as_ref() {
        Some(record) => translate::gates(&record.step.gates),
        None => List::with_capacity(0),
    }
}

/// How an application ends.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum Finish {
    Made,
    Stale,
    Invalid,
    Accepting,
    Failed,
}

/// Makes the next write, or ends the application once every write is made.
fn next(model: &mut Model, env: &Env<Limits>, id: Id<Entry>) {
    let bound = plan::max_out(&env.limits.plan).saturating_add(1);
    for _ in 0..bound {
        let entry = get(model, id);
        let Some(applying) = items::applying(&entry.job) else { unreachable!("an item applying makes its writes") };
        let Some(writes) = items::writes(&applying.doing) else { unreachable!("an item making writes has them") };
        let Some(write) = writes.list.get(usize::try_from(writes.next).unwrap_or(usize::MAX)) else {
            return finish(model, env, id, Finish::Made);
        };
        let write = write.clone();
        match rule(model, env, id, &write) {
            Ruled::Allow => {}
            Ruled::Reading => return,
            Ruled::Decided(decision) => return ruled(model, env, id, decision),
        }
        match make(model, env, id) {
            Made::Done => advance(model, id),
            Made::Holding => return advance(model, id),
            Made::Waiting => return,
            Made::Failed => return finish(model, env, id, Finish::Failed),
        }
    }
}

/// What the rules said of a write.
enum Ruled {
    Allow,
    /// A reviewer's permission is being read first.
    Reading,
    Decided(rules::Decision),
}

/// A write the rules did not allow: an action waits for facts, or a person;
/// an outcome is accepted by a person, or held.
fn ruled(model: &mut Model, env: &Env<Limits>, id: Id<Entry>, decision: rules::Decision) {
    let item = get(model, id).item;
    let refused = decision == rules::Decision::Refuse;
    model::keep(model, Fact::Ruled { item, refused });
    let entry = get(model, id);
    let (action, news) = match &entry.job {
        Job::Applying(applying) => (applying.of == Of::Action || applying.of == Of::Done, applying.news),
        Job::Idle | Job::Asking { .. } | Job::Writing { .. } | Job::Recording { .. } | Job::Starting(_) => (false, 0),
    };
    // What the rules wait for may have come while the action was made: the
    // item waits for news only if none came since it began.
    let unchanged = entry.next == news;
    match decision {
        rules::Decision::Allow => unreachable!("an allowed write is made"),
        rules::Decision::Wait if action => {
            if unchanged {
                get_mut(model, id).blocked = true;
            }
            finish(model, env, id, Finish::Stale);
        }
        rules::Decision::Accept { .. } if !action && rejected(get(model, id)) => reject(model, env, id),
        rules::Decision::Accept { permission } => {
            get_mut(model, id).wants = Some(permission);
            finish(model, env, id, Finish::Accepting);
        }
        rules::Decision::Wait | rules::Decision::Refuse => finish(model, env, id, Finish::Failed),
    }
}

/// The rules on the write in hand: allowed, refused, waiting or wanting a
/// person; or, for a merge, the reviewers' permissions to read first.
fn rule(model: &mut Model, env: &Env<Limits>, id: Id<Entry>, write: &plan::Write) -> Ruled {
    let entry = get(model, id);
    let repository = entry.item.repository;
    let checked = match write {
        plan::Write::Create { record, .. } => {
            rules::Write::Item { repository: translate::repository(record.step.repository.0) }
        }
        plan::Write::OpenPull { base } => {
            rules::Write::Open { repository: translate::repository(repository), base: copy_of(base) }
        }
        plan::Write::DeleteBranch => rules::Write::Delete {
            repository: translate::repository(repository),
            branch: translate::branch(&model.config.branches, entry.item),
        },
        // A merge with no pull request to land fails as it is made.
        plan::Write::Merge { .. } if entry.relations.pull.is_none() => return Ruled::Allow,
        plan::Write::Merge { head } => match landing(model, env, id, *head) {
            Some(_) if !lands_as_planned(get(model, id)) => return Ruled::Decided(rules::Decision::Refuse),
            Some(landing) => rules::Write::Land(landing),
            None => return Ruled::Reading,
        },
        plan::Write::ReopenPull
        | plan::Write::Close
        | plan::Write::Progress(_)
        | plan::Write::Goal(_)
        | plan::Write::Release { .. } => return Ruled::Allow,
    };
    let entry = get(model, id);
    let gates = gates(entry);
    let accepted = match items::applying(&entry.job) {
        Some(applying) => items::accepted(entry, items::comment_of(applying.of)),
        None => None,
    };
    let mut findings = Queue::with_capacity(rules::max_out(&env.limits.rules));
    let decision =
        rules::check_write(&model.config.rules, &env.limits.rules, &checked, accepted, gates.as_slice(), &mut findings);
    match decision {
        rules::Decision::Allow => Ruled::Allow,
        rules::Decision::Wait | rules::Decision::Accept { .. } | rules::Decision::Refuse => Ruled::Decided(decision),
    }
}

/// The landing a merge of the item's pull request at `head` makes, for the
/// rules: the base it lands on, CI on its head and the reviews on that
/// head, each with its reviewer's permission, all as the pull request is
/// read afresh; `None` while it, or a reviewer's permission, is being read.
/// The reviews are the working set's, on the head it last read, which need
/// not be the fresh one: the rules count only those on the head landed.
fn landing(model: &mut Model, env: &Env<Limits>, id: Id<Entry>, head: plan::Commit) -> Option<rules::Landing> {
    let entry = get(model, id);
    let item = translate::forge_item(entry.item);
    let Some(applying) = items::applying(&entry.job) else { unreachable!("an item applying lands") };
    let Some(writes) = items::writes(&applying.doing) else { unreachable!("an item making writes lands") };
    let Some(fresh) = writes.landing.as_ref() else {
        fresh_pull(model, env, id);
        return None;
    };
    let reviewed = match model.forge.pull(item) {
        Some(level) => level.commit,
        None => [0; 32],
    };
    let mut reviews = List::with_capacity(env.limits.forge.reviewers);
    for verdict in model.forge.reviews(item).unwrap_or(&[]) {
        let Some(stance) = translate::stance(verdict.verdict) else { continue };
        let Some(permission) = permission_of(writes, verdict.author) else {
            let user = verdict.author;
            let repository = entry.item.repository;
            permission_read(model, env, id, repository, user);
            return None;
        };
        let review = rules::Review { person: verdict.author, permission, head: reviewed, stance };
        if reviews.push(review).is_err() {
            break;
        }
    }
    Some(rules::Landing {
        repository: translate::repository(item.repository),
        base: copy_of(&fresh.base),
        head: head.0,
        ci: translate::rules_ci(fresh.ci),
        ci_head: fresh.head,
        reviews: reviews.into_boxed(),
    })
}

/// Whether a merge lands where the item's step says it does: a pull request
/// retargeted since is not merged.
fn lands_as_planned(entry: &Entry) -> bool {
    let Some(applying) = items::applying(&entry.job) else { return true };
    let Some(writes) = items::writes(&applying.doing) else { return true };
    let Some(fresh) = writes.landing.as_ref() else { return true };
    match entry.step.as_ref() {
        Some(record) => match &record.step.work {
            plan::Work::Change(change) => *change.base == *fresh.base,
            plan::Work::Agent(_) | plan::Work::Wait(_) | plan::Work::Session(_) => false,
        },
        None => false,
    }
}

/// The permission read of `person`, among those a merge read.
fn permission_of(writes: &Writes, person: u64) -> Option<rules::Permission> {
    for review in &writes.reviews {
        if review.person == person {
            return Some(review.permission);
        }
    }
    None
}

fn permission_read(model: &mut Model, env: &Env<Limits>, id: Id<Entry>, repository: u32, user: u64) {
    let read = forge::Read::Permission { repository, user };
    landing_read(model, env, id, items::Reading::Permission { person: user }, read);
}

/// Reads the item's pull request afresh, for a merge.
fn fresh_pull(model: &mut Model, env: &Env<Limits>, id: Id<Entry>) {
    let entry = get(model, id);
    let Some(pull) = entry.relations.pull else { unreachable!("a merge names the pull request it lands") };
    let read = forge::Read::Pull { item: forge::Item { repository: entry.item.repository, number: pull } };
    landing_read(model, env, id, items::Reading::Pull, read);
}

fn landing_read(model: &mut Model, env: &Env<Limits>, id: Id<Entry>, reading: items::Reading, read: forge::Read) {
    let Ok(wait) = model.waits.insert(Wait::Job { entry: id }) else {
        unreachable!("the waits have room for every item's job")
    };
    if let Some(applying) = items::applying_mut(&mut get_mut(model, id).job) {
        applying.wait = Some(wait);
        if let Some(writes) = items::writes_mut(&mut applying.doing) {
            writes.reading = Some(reading);
        }
    }
    route::forge_step(model, env, forge::Event::Read { owner: wait.token(), read });
}

/// How a write went as it was asked for.
enum Made {
    /// Made at once: the step's own parts, or nothing left to do.
    Done,
    /// Made at once, and the application waits for what it changed to be
    /// written elsewhere: it goes on from the ready list.
    Holding,
    /// Asked of the forge.
    Waiting,
    Failed,
}

/// Makes the write in hand.
fn make(model: &mut Model, env: &Env<Limits>, id: Id<Entry>) -> Made {
    let entry = get(model, id);
    let item = entry.item;
    let Some(applying) = items::applying(&entry.job) else { unreachable!("an item applying makes its writes") };
    let resumed = applying.resumed;
    let of = applying.of;
    let Some(writes) = items::writes(&applying.doing) else { unreachable!("an item making writes has them") };
    let retried = writes.retried;
    let Some(write) = writes.list.get(usize::try_from(writes.next).unwrap_or(usize::MAX)) else {
        unreachable!("a write in hand is among the writes")
    };
    let forge_item = translate::forge_item(item);
    let made = match write {
        plan::Write::Create { key, record } => {
            let key = key_of(item, of, key);
            let mut labels = List::with_capacity(1);
            labels.push(copy_of(&model.config.forge.tracking)).expect("room for the tracking label");
            forge::Write::CreateIssue {
                repository: record.step.repository.0,
                key,
                title: copy_of(&record.step.name),
                body: forge::Content::Text(copy_of(&record.step.name)),
                labels: labels.into_boxed(),
            }
        }
        plan::Write::OpenPull { base } => {
            let title = match entry.step.as_ref() {
                Some(record) => copy_of(&record.step.name),
                None => translate::decimal(item.number),
            };
            forge::Write::OpenPull {
                repository: item.repository,
                title,
                body: forge::Content::Text(translate::concat(&[b"#", &translate::decimal(item.number)])),
                head: translate::branch(&model.config.branches, item),
                base: copy_of(base),
            }
        }
        plan::Write::ReopenPull => {
            let Some(pull) = entry.relations.pull else { return Made::Done };
            forge::Write::Reopen { item: forge::Item { repository: item.repository, number: pull } }
        }
        plan::Write::Merge { head } => {
            let Some(pull) = entry.relations.pull else { return Made::Failed };
            forge::Write::Merge { item: forge::Item { repository: item.repository, number: pull }, head: head.0 }
        }
        plan::Write::Close => {
            if entry.closed {
                return Made::Done;
            }
            forge::Write::Close { item: forge_item }
        }
        plan::Write::DeleteBranch => forge::Write::DeleteBranch {
            repository: item.repository,
            branch: translate::branch(&model.config.branches, item),
        },
        plan::Write::Progress(progress) => {
            let progress = *progress;
            if let Some(staged) = get_mut(model, id).staged.as_mut() {
                staged.progress = progress;
            }
            return Made::Done;
        }
        plan::Write::Goal(goal) => {
            let goal = plan::Goal::clone(goal);
            if grown(model, env, id, goal) {
                return Made::Holding;
            }
            return Made::Done;
        }
        plan::Write::Release { step } => {
            let step = copy_of(step);
            release_step(model, env, id, &step);
            return Made::Done;
        }
    };
    // What an outcome read back may have been made by the life that read it
    // first, after its comment; an action's by an earlier life, decided
    // again after a restart, when nothing says: each is looked for first.
    let resumed = match of {
        Of::Outcome { comment, .. } if resumed || retried => Some(forge::Cause { comment, at: Time::ZERO }),
        Of::Outcome { .. } => None,
        Of::Action | Of::Done => Some(forge::Cause { comment: 0, at: Time::ZERO }),
    };
    let Ok(wait) = model.waits.insert(Wait::Job { entry: id }) else {
        unreachable!("the waits have room for every item's job")
    };
    if let Some(applying) = items::applying_mut(&mut get_mut(model, id).job) {
        applying.wait = Some(wait);
    }
    route::forge_step(model, env, forge::Event::Write { owner: wait.token(), write: made, resumed });
    Made::Waiting
}

/// The key of a creation (seams: "Keys"): the outcome's item and attempt,
/// then the plan's key.
fn key_of(item: Item, of: Of, key: &plan::Key) -> Box<[u8]> {
    let attempt = match of {
        Of::Outcome { attempt, .. } => attempt,
        Of::Action | Of::Done => 0,
    };
    let (kind, name) = match key {
        plan::Key::Step(name) => (&b"step/"[..], copy_of(name)),
        plan::Key::Task(index) => (&b"task/"[..], translate::decimal(u64::from(*index))),
    };
    translate::concat(&[
        &translate::decimal(u64::from(item.repository)),
        b"/",
        &translate::decimal(item.number),
        b"/",
        &translate::decimal(attempt),
        b"/",
        kind,
        &name,
    ])
}

/// The goal's part of a record, written by an outcome: the item's own, if it
/// proposed the plan or supervises it; else its goal's, which is written
/// there on the side. Whether the application waits for that record to be
/// written before it goes on: its own record is the commit point, and a
/// restart before the goal's lands would leave the goal without the steps
/// it grew by.
fn grown(model: &mut Model, env: &Env<Limits>, id: Id<Entry>, goal: plan::Goal) -> bool {
    let entry = get_mut(model, id);
    let own = match entry.staged.as_ref() {
        Some(staged) => staged.goal.is_some() || proposes(entry),
        None => false,
    };
    if own {
        if let Some(staged) = entry.staged.as_mut() {
            staged.goal = Some(goal);
        }
        return false;
    }
    let Some(goal_item) = entry.relations.goal else { return false };
    let Some(goal_id) = items::find(model, goal_item) else { return false };
    let Some(goal_entry) = model.items.get_mut(goal_id) else { return false };
    if let Some(record) = goal_entry.step.as_mut() {
        record.goal = Some(goal);
    }
    items::aside(model, env, goal_id);
    if !items::saving(get(model, goal_id)) {
        // Nothing of the goal's is written any more: it is done.
        return false;
    }
    get_mut(model, id).awaiting = Some(items::Awaiting::Goal(goal_id));
    true
}

/// Whether the outcome being applied proposes a plan.
fn proposes(entry: &Entry) -> bool {
    match &entry.outcome {
        Some((_, posted)) => match posted.outcome {
            crate::boundary::Outcome::Plan { .. } => true,
            crate::boundary::Outcome::Change { .. }
            | crate::boundary::Outcome::Verdict { .. }
            | crate::boundary::Outcome::Report { .. }
            | crate::boundary::Outcome::Steps { .. }
            | crate::boundary::Outcome::Tasks { .. }
            | crate::boundary::Outcome::Reply { .. }
            | crate::boundary::Outcome::Finished { .. }
            | crate::boundary::Outcome::Release { .. }
            | crate::boundary::Outcome::Escalation { .. } => false,
        },
        None => false,
    }
}

/// A supervising session releases the step of its goal named `step`, as a
/// person would: the hub releases it, and then the plan says what the
/// release writes into it.
fn release_step(model: &mut Model, env: &Env<Limits>, id: Id<Entry>, step: &[u8]) {
    let entry = get(model, id);
    let mut found: Option<Item> = None;
    for child in &entry.relations.children {
        if *child.name == *step {
            found = Some(child.item);
        }
    }
    let Some(child) = found else { return };
    let Some(child_id) = items::find(model, child) else { return };
    let Ok(wait) = model.waits.insert(Wait::Release { entry: child_id }) else {
        unreachable!("the waits have room for every item's call")
    };
    let reply_to = temper_lib::ReplyTo::new(wait.token());
    route::work_step(model, env, work::Event::Release { reply_to, item: child });
}

/// Moves on past the write in hand.
fn advance(model: &mut Model, id: Id<Entry>) {
    if let Some(applying) = items::applying_mut(&mut get_mut(model, id).job) {
        applying.wait = None;
        if let Some(writes) = items::writes_mut(&mut applying.doing) {
            writes.next = writes.next.saturating_add(1);
            writes.retried = false;
        }
    }
}

/// Ends the application, telling the hub; once its writes are made, the
/// plan's parts it staged are committed.
fn finish(model: &mut Model, env: &Env<Limits>, id: Id<Entry>, finish: Finish) {
    let entry = get_mut(model, id);
    let applying = match mem::replace(&mut entry.job, Job::Idle) {
        Job::Applying(applying) => applying,
        Job::Idle | Job::Asking { .. } | Job::Writing { .. } | Job::Recording { .. } | Job::Starting(_) => {
            unreachable!("an item applying finishes")
        }
    };
    let staged = entry.staged.take();
    entry.awaiting = None;
    let then = match &applying.doing {
        Doing::Writes(writes) => writes.then,
        Doing::Outcome | Doing::Fresh => plan::Then::Wait,
    };
    if finish == Finish::Made && staged.is_some() {
        entry.step = staged;
    }
    // A decision on the outcome counts for its application, however it
    // ends; one on the step, until an outcome is made.
    let decided = match applying.of {
        Of::Outcome { comment, .. } => finish == Finish::Made || entry.relations.accepting == Some(comment),
        Of::Action | Of::Done => false,
    };
    if decided {
        entry.relations.decision = None;
        entry.relations.accepted = None;
        entry.relations.accepting = None;
        entry.wants = None;
    }
    if finish == Finish::Made && items::comment_of(applying.of).is_some() {
        entry.outcome = None;
    }
    let owner = applying.owner;
    let event = match applying.of {
        Of::Outcome { .. } => {
            let applied = match finish {
                Finish::Made => work::Applied::Made(translate::then(then)),
                Finish::Stale => work::Applied::Stale,
                Finish::Invalid => work::Applied::Invalid,
                Finish::Accepting => work::Applied::Accepting,
                Finish::Failed => work::Applied::Failed,
            };
            work::Event::Applied { owner, applied }
        }
        Of::Action | Of::Done => {
            let acted = match finish {
                Finish::Made => work::Acted::Made,
                Finish::Stale | Finish::Invalid => work::Acted::Stale,
                Finish::Accepting => work::Acted::Accepting,
                Finish::Failed => work::Acted::Failed,
            };
            work::Event::Acted { owner, acted }
        }
    };
    route::work_step(model, env, event);
}

/// A forge read for the item's job ended.
pub(crate) fn read(model: &mut Model, env: &Env<Limits>, id: Id<Entry>, result: Result<api::Answer, forge::Failure>) {
    let Some(entry) = model.items.get(id) else { return };
    match &entry.job {
        Job::Asking { .. } => related(model, env, id, result),
        Job::Applying(applying) => match &applying.doing {
            Doing::Outcome => outcome_read(model, env, id, result),
            Doing::Fresh => fresh_read(model, env, id, result),
            Doing::Writes(_) => landing_answer(model, env, id, result),
        },
        Job::Idle | Job::Writing { .. } | Job::Recording { .. } | Job::Starting(_) => {}
    }
}

/// The outcome's comment read: the outcome decoded from it is kept, and the
/// application goes on.
fn outcome_read(model: &mut Model, env: &Env<Limits>, id: Id<Entry>, result: Result<api::Answer, forge::Failure>) {
    match result {
        Ok(_) => {}
        Err(forge::Failure::Busy) => return stall(model, id),
        Err(_) => return finish(model, env, id, Finish::Failed),
    }
    let Some(applying) = items::applying(&get(model, id).job) else {
        unreachable!("an item applying reads its outcome")
    };
    let Some(comment) = items::comment_of(applying.of) else { unreachable!("only an outcome is read") };
    let mut found = None;
    for decoded in &model.decoded {
        match decoded {
            crate::boundary::Decoded::Outcome { comment: at, posted } if *at == comment => {
                found = Some(posted.clone());
            }
            crate::boundary::Decoded::Outcome { .. }
            | crate::boundary::Decoded::Record { .. }
            | crate::boundary::Decoded::Page { .. } => {}
        }
    }
    let Some(posted) = found else { return finish(model, env, id, Finish::Failed) };
    let entry = get_mut(model, id);
    entry.outcome = Some((comment, posted));
    if let Some(applying) = items::applying_mut(&mut entry.job) {
        applying.doing = Doing::Fresh;
    }
    go(model, env, id);
}

/// The pull request read afresh.
fn fresh_read(model: &mut Model, env: &Env<Limits>, id: Id<Entry>, result: Result<api::Answer, forge::Failure>) {
    let pull = match result {
        Ok(api::Answer::Pull(pull)) => pull,
        Err(forge::Failure::Busy) => return stall(model, id),
        Ok(_) | Err(_) => return applied(model, env, id, None),
    };
    let level = forge::Level {
        number: pull.number,
        commit: pull.commit,
        base: pull.base_commit,
        ci: pull.ci,
        open: pull.state == api::State::Open,
        merged: pull.merged,
        mergeable: pull.mergeable,
    };
    let entry = get(model, id);
    let reviews = model.forge.reviews(translate::forge_item(entry.item));
    let fresh = translate::pull(level, reviews, entry.seen);
    applied(model, env, id, Some(fresh));
}

/// What a merge read before the rules decide ended: the pull request
/// afresh, or a reviewer's permission.
fn landing_answer(model: &mut Model, env: &Env<Limits>, id: Id<Entry>, result: Result<api::Answer, forge::Failure>) {
    if let Err(forge::Failure::Busy) = result {
        return stall(model, id);
    }
    let Some(applying) = items::applying_mut(&mut get_mut(model, id).job) else {
        unreachable!("an item applying reads for its merge")
    };
    applying.wait = None;
    let Some(writes) = items::writes_mut(&mut applying.doing) else {
        unreachable!("an item making writes reads for its merge")
    };
    match writes.reading.take() {
        Some(items::Reading::Pull) => {
            let Ok(api::Answer::Pull(pull)) = result else { return finish(model, env, id, Finish::Failed) };
            let ci = translate::plan_ci(pull.ci);
            writes.landing = Some(items::Fresh { base: pull.base, head: pull.commit, ci });
        }
        Some(items::Reading::Permission { person }) => {
            let permission = match result {
                Ok(api::Answer::Permission(permission)) => translate::permission(permission),
                Ok(_) | Err(_) => rules::Permission::None,
            };
            let review = rules::Review { person, permission, head: [0; 32], stance: rules::Stance::Approve };
            writes.reviews.push(review).expect("room for each of them");
        }
        None => {}
    }
    next(model, env, id);
}

/// A forge write for the item's job ended.
pub(crate) fn wrote(
    model: &mut Model,
    env: &Env<Limits>,
    id: Id<Entry>,
    result: Result<forge::Written, forge::Failure>,
) {
    let Some(entry) = model.items.get(id) else { return };
    match &entry.job {
        Job::Writing { owner, retries } => {
            let (owner, retries) = (*owner, *retries);
            let wrote = match result {
                Ok(_) => work::Wrote::Done,
                Err(forge::Failure::Busy) => return stall(model, id),
                // The forge failed for a while, past the forge sub-model's
                // own attempts: the record is what holds a run's answer, so
                // it is tried again a few times before the item is held.
                Err(forge::Failure::Forge(
                    api::Error::Timeout | api::Error::Unavailable | api::Error::RateLimited { .. },
                )) if retries < items::RECORD_RETRIES => {
                    get_mut(model, id).job = Job::Writing { owner, retries: retries.saturating_add(1) };
                    return stall(model, id);
                }
                Err(_) => work::Wrote::Failed,
            };
            items::recorded(model, id, wrote == work::Wrote::Done);
            get_mut(model, id).job = Job::Idle;
            route::work_step(model, env, work::Event::Written { owner, wrote });
        }
        Job::Recording { owner, outcome, wait, resumed } => {
            let (owner, outcome, wait, resumed) = (*owner, *outcome, *wait, *resumed);
            let comment = match result {
                Ok(forge::Written::Commented(comment)) => Some(comment),
                Err(forge::Failure::Busy) => return stall(model, id),
                Err(forge::Failure::Forge(api::Error::Timeout)) if !resumed => {
                    let attempt = crate::runs::attempt_of(model, outcome);
                    get_mut(model, id).job = Job::Recording { owner, outcome, wait, resumed: true };
                    let Ok(again) = model.waits.insert(Wait::Job { entry: id }) else {
                        unreachable!("the waits have room for every item's job")
                    };
                    get_mut(model, id).job = Job::Recording { owner, outcome, wait: again, resumed: true };
                    return post(model, env, id, again, attempt, true);
                }
                Ok(_) | Err(_) => None,
            };
            if let Some(comment) = comment
                && let Some(posted) = crate::runs::posted(model, outcome)
            {
                get_mut(model, id).outcome = Some((comment, posted));
            }
            get_mut(model, id).job = Job::Idle;
            route::work_step(model, env, work::Event::Recorded { owner, comment });
        }
        Job::Applying(_) => written(model, env, id, result),
        Job::Idle | Job::Asking { .. } | Job::Starting(_) => {}
    }
}

/// A write of an application ended: on to the next, or the application
/// fails; one that timed out goes again once, resumed.
fn written(model: &mut Model, env: &Env<Limits>, id: Id<Entry>, result: Result<forge::Written, forge::Failure>) {
    let retried = match &get(model, id).job {
        Job::Applying(applying) => match &applying.doing {
            Doing::Writes(writes) => writes.retried,
            Doing::Outcome | Doing::Fresh => true,
        },
        Job::Idle | Job::Asking { .. } | Job::Writing { .. } | Job::Recording { .. } | Job::Starting(_) => true,
    };
    let written = match result {
        Ok(written) => written,
        Err(forge::Failure::Busy) => return stall(model, id),
        Err(forge::Failure::Forge(api::Error::Timeout)) if !retried => {
            if let Some(applying) = items::applying_mut(&mut get_mut(model, id).job)
                && let Some(writes) = items::writes_mut(&mut applying.doing)
            {
                writes.retried = true;
            }
            return again(model, env, id);
        }
        Err(forge::Failure::Forge(api::Error::Missing)) if deleting(model, id) => forge::Written::Done,
        // The pull request moved on, or closed, since the merge was decided.
        Err(forge::Failure::Forge(api::Error::Stale | api::Error::Closed)) => {
            return finish(model, env, id, Finish::Stale);
        }
        // Its base moved under it since the working set read it: the head
        // conflicts, as if it had been read so, and is the change's to
        // repair (engine-model.md, 5.3).
        Err(forge::Failure::Forge(api::Error::Conflict)) if merging(model, id).is_some() => {
            let head = merging(model, id);
            get_mut(model, id).conflicted = head;
            return finish(model, env, id, Finish::Stale);
        }
        Err(_) => return finish(model, env, id, Finish::Failed),
    };
    let waits = made(model, env, id, written);
    advance(model, id);
    // An item it created goes on once its record is written: the
    // application goes on then, from the ready list.
    if !waits {
        next(model, env, id);
    }
}

/// Whether the write in hand deletes a branch, which is done if the branch is
/// gone.
fn deleting(model: &Model, id: Id<Entry>) -> bool {
    let Some(write) = in_hand(model, id) else { return false };
    match write {
        plan::Write::DeleteBranch => true,
        plan::Write::Create { .. }
        | plan::Write::OpenPull { .. }
        | plan::Write::ReopenPull
        | plan::Write::Merge { .. }
        | plan::Write::Close
        | plan::Write::Progress(_)
        | plan::Write::Goal(_)
        | plan::Write::Release { .. } => false,
    }
}

/// The head the write in hand merges, if it is a merge.
fn merging(model: &Model, id: Id<Entry>) -> Option<[u8; 32]> {
    match in_hand(model, id)? {
        plan::Write::Merge { head } => Some(head.0),
        plan::Write::Create { .. }
        | plan::Write::OpenPull { .. }
        | plan::Write::ReopenPull
        | plan::Write::Close
        | plan::Write::DeleteBranch
        | plan::Write::Progress(_)
        | plan::Write::Goal(_)
        | plan::Write::Release { .. } => None,
    }
}

fn in_hand(model: &Model, id: Id<Entry>) -> Option<&plan::Write> {
    let applying = items::applying(&model.items.get(id)?.job)?;
    let writes = items::writes(&applying.doing)?;
    writes.list.get(usize::try_from(writes.next).ok()?)
}

/// What a write made changes in the top level's state: an item created is
/// taken in, a pull request opened is linked.
fn made(model: &mut Model, env: &Env<Limits>, id: Id<Entry>, written: forge::Written) -> bool {
    let Some(write) = in_hand(model, id) else { return false };
    let number = match written {
        forge::Written::Created(number) => Some(number),
        forge::Written::Merged(_)
        | forge::Written::Commented(_)
        | forge::Written::Revision(_)
        | forge::Written::Reviewed(_)
        | forge::Written::Done => None,
    };
    match write {
        plan::Write::Create { record, .. } => {
            let Some(number) = number else { return false };
            let record = plan::Record::clone(record);
            created(model, env, id, record, number)
        }
        plan::Write::OpenPull { .. } => {
            let Some(number) = number else { return false };
            let entry = get_mut(model, id);
            entry.relations.pull = Some(number);
            let item = translate::forge_item(entry.item);
            route::forge_step(model, env, forge::Event::Link { item, pull: Some(number) });
            false
        }
        plan::Write::Merge { .. } => {
            let commit = match written {
                forge::Written::Merged(commit) => Some(commit),
                forge::Written::Created(_)
                | forge::Written::Commented(_)
                | forge::Written::Revision(_)
                | forge::Written::Reviewed(_)
                | forge::Written::Done => None,
            };
            if let Some(commit) = commit {
                get_mut(model, id).merged = Some(commit);
            }
            false
        }
        plan::Write::ReopenPull
        | plan::Write::Close
        | plan::Write::DeleteBranch
        | plan::Write::Progress(_)
        | plan::Write::Goal(_)
        | plan::Write::Release { .. } => false,
    }
}

/// An item an application made, for a step: held, its record's parts as the
/// plan gave them, its dependencies among its goal's steps, and it joins
/// its goal's steps, and the steps of the item that added it.
fn created(model: &mut Model, env: &Env<Limits>, id: Id<Entry>, record: plan::Record, number: u64) -> bool {
    let entry = get(model, id);
    let parent = entry.item;
    let supervising = match entry.staged.as_ref() {
        Some(staged) => staged.goal.is_some(),
        None => false,
    };
    let goal = if supervising || proposes(entry) { Some(parent) } else { entry.relations.goal };
    let item = Item { repository: record.step.repository.0, number };
    let name = copy_of(&record.step.name);
    let dependencies = resolve(model, goal, &record.step.after, &env.limits);
    let Some(child) = items::hold(model, env, item) else { return false };
    let child_entry = get_mut(model, child);
    if child_entry.step.is_none() {
        child_entry.step = Some(record);
        child_entry.relations.goal = goal;
        child_entry.relations.parent = Some(parent);
        child_entry.relations.dependencies = dependencies;
    }
    // Its record is written before the application goes on: the step it
    // carries is nowhere else, and a restart before then applies the outcome
    // again, which finds the item by its key and takes it in again.
    let unwritten = child_entry.taking != items::Taking::Taken;
    if unwritten {
        child_entry.holding = Some(id);
    }
    if let Some(goal) = goal {
        join(model, goal, &name, item, env.limits.plan.steps);
    }
    if goal != Some(parent) {
        join(model, parent, &name, item, env.limits.plan.steps);
    }
    unwritten
}

/// The items of the steps named `after`, among the goal's steps.
fn resolve(model: &Model, goal: Option<Item>, after: &[Box<[u8]>], limits: &Limits) -> Box<[Related]> {
    let mut found = List::with_capacity(limits.plan.dependencies);
    let Some(goal) = goal else { return found.into_boxed() };
    let Some(goal) = items::find(model, goal) else { return found.into_boxed() };
    let Some(goal) = model.items.get(goal) else { return found.into_boxed() };
    for name in after {
        for child in &goal.relations.children {
            if *child.name == **name {
                let related = Related { name: copy_of(name), item: child.item, done: child.done };
                if found.push(related).is_err() {
                    break;
                }
            }
        }
    }
    found.into_boxed()
}

/// `item`, of the step `name`, joins the steps of `to`, which keeps as many
/// as a plan holds: past them, the one done first makes room, and if none
/// is done, it does not join (its own record names its parent and goal).
fn join(model: &mut Model, to: Item, name: &[u8], item: Item, most: u32) {
    let Some(id) = items::find(model, to) else { return };
    let Some(entry) = model.items.get_mut(id) else { return };
    let mut done: Option<usize> = None;
    for (index, child) in entry.relations.children.iter().enumerate() {
        if child.item == item {
            return;
        }
        if done.is_none() && child.done.is_some() {
            done = Some(index);
        }
    }
    let full = !limits::within(entry.relations.children.len().saturating_add(1), most);
    let dropped = match done {
        Some(index) if full => Some(index),
        Some(_) | None if full => return,
        Some(_) | None => None,
    };
    let mut children = List::with_capacity(most);
    for (index, child) in entry.relations.children.iter().enumerate() {
        if Some(index) != dropped {
            children.push(child.clone()).expect("room for each of them");
        }
    }
    children.push(Related { name: copy_of(name), item, done: None }).expect("room for each of them");
    entry.relations.children = children.into_boxed();
}

/// The forge refused the job's op as busy: it goes again from the ready list.
fn stall(model: &mut Model, id: Id<Entry>) {
    if model.stalled.try_push(id).is_err() {
        unreachable!("the ready list has room for every item");
    }
}

/// Asks again for what the item's job was waiting for when the forge
/// refused it as busy, or when it timed out.
pub(crate) fn again(model: &mut Model, env: &Env<Limits>, id: Id<Entry>) {
    let Some(entry) = model.items.get(id) else { return };
    match &entry.job {
        Job::Asking { .. } => {
            let index = entry.asking;
            ask(model, env, id, index);
        }
        Job::Writing { .. } => {
            let Ok(wait) = model.waits.insert(Wait::Job { entry: id }) else {
                unreachable!("the waits have room for every item's job")
            };
            write_record(model, env, id, wait);
        }
        Job::Recording { owner, outcome, resumed, .. } => {
            let (owner, outcome, resumed) = (*owner, *outcome, *resumed);
            let attempt = crate::runs::attempt_of(model, outcome);
            let Ok(wait) = model.waits.insert(Wait::Job { entry: id }) else {
                unreachable!("the waits have room for every item's job")
            };
            get_mut(model, id).job = Job::Recording { owner, outcome, wait, resumed };
            post(model, env, id, wait, attempt, resumed);
        }
        Job::Applying(applying) => match &applying.doing {
            Doing::Outcome | Doing::Fresh => go(model, env, id),
            Doing::Writes(writes) => {
                match entry.awaiting {
                    Some(items::Awaiting::Goal(_)) => return,
                    Some(items::Awaiting::Unwritten) => {
                        get_mut(model, id).awaiting = None;
                        return finish(model, env, id, Finish::Failed);
                    }
                    None => {}
                }
                if writes.reading.is_some()
                    && let Some(applying) = items::applying_mut(&mut get_mut(model, id).job)
                    && let Some(writes) = items::writes_mut(&mut applying.doing)
                {
                    writes.reading = None;
                }
                next(model, env, id);
            }
        },
        Job::Idle | Job::Starting(_) => {}
    }
}

/// The entry of an item the hub holds.
fn held(model: &Model, item: Item) -> Id<Entry> {
    let Some(id) = items::find(model, item) else { unreachable!("the hub asks only of items the top level holds") };
    id
}

pub(crate) fn get(model: &Model, id: Id<Entry>) -> &Entry {
    model.items.get(id).expect("an entry named is held")
}

pub(crate) fn get_mut(model: &mut Model, id: Id<Entry>) -> &mut Entry {
    model.items.get_mut(id).expect("an entry named is held")
}
