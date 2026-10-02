//! People (engine-model.md, sections 2, 4.6, 6 and 7): their calls through
//! the web, and the work they hand in on the forge.
//!
//! Every call reads the person's permission on its repository first, as the
//! forge reports it, and the rules say whether it may be acted on. Then:
//!
//! - **Open a session:** an issue made in the repository, keyed by the
//!   request, carrying the tracking label and the person's first message,
//!   taken in as a session.
//! - **Message an item:** a comment written for the person, keyed by the
//!   request, which the forge then reads as news from them (the engine
//!   writes it on their behalf: section 15's open question, settled so for
//!   now).
//! - **Accept, reject:** the decision is the item's, in its record; an item
//!   held for a person's acceptance is released, and its outcome applied
//!   again with the decision; any other is told of it.
//! - **Stop, release:** the hub's; a release asks the plan what it writes
//!   first, which lifts what held the item.
//! - **Watch:** the views'.
//!
//! An issue handed in by its label becomes a session (4.6). The working set
//! may have no room for an item: those the top level asked for are tracked
//! again once it has.

use alloc::boxed::Box;

use temper_engine_model_forge::{self as forge, api};
use temper_engine_model_plan as plan;
use temper_engine_model_rules as rules;
use temper_engine_model_views as views;
use temper_engine_model_work as work;
use temper_lib::bytes::copy_of;
use temper_lib::{Env, Id, List, Queue, ReplyTo, Token};

use crate::boundary::{Ask, Inbound, Item, Refusal, Reply, Request, Watched};
use crate::items::{self, Entry};
use crate::limits::Limits;
use crate::model::Model;
use crate::route;
use crate::serve;
use crate::translate;
use crate::waits::Wait;

/// A person's call.
pub(crate) fn ask(model: &mut Model, env: &Env<Limits>, to: ReplyTo, person: u64, ask: Ask, out: &mut Queue<Request>) {
    let repository = match &ask {
        Ask::Open { repository, .. } | Ask::Watch { subject: Watched::Board { repository } } => *repository,
        Ask::Message { item, .. }
        | Ask::Accept { item }
        | Ask::Reject { item }
        | Ask::Stop { item }
        | Ask::Release { item }
        | Ask::Watch { subject: Watched::Run { item } | Watched::Item { item } } => {
            if items::find(model, *item).is_none() {
                return out.push(Request::Reply { to, reply: Reply::Refused(Refusal::Unknown) });
            }
            item.repository
        }
    };
    if repository >= model.config.repositories() {
        return out.push(Request::Reply { to, reply: Reply::Refused(Refusal::Unknown) });
    }
    let read = forge::Read::Permission { repository, user: person };
    let wait = Wait::Person { to, person, ask };
    let wait = match model.waits.insert(wait) {
        Ok(wait) => wait,
        Err(Wait::Person { to, .. }) => return out.push(Request::Reply { to, reply: Reply::Refused(Refusal::Busy) }),
        Err(_) => unreachable!("a person's call is refused as it was asked"),
    };
    route::forge_step(model, env, forge::Event::Read { owner: wait.token(), read });
}

/// A forge read for a person's call ended: their permission.
pub(crate) fn read(
    model: &mut Model,
    env: &Env<Limits>,
    to: ReplyTo,
    person: u64,
    ask: Ask,
    result: Result<api::Answer, forge::Failure>,
) {
    let permission = match result {
        Ok(api::Answer::Permission(permission)) => translate::permission(permission),
        Err(forge::Failure::Busy) => return reply(model, to, Reply::Refused(Refusal::Busy)),
        Ok(_) | Err(_) => return reply(model, to, Reply::Refused(Refusal::Failed)),
    };
    let (repository, act) = act(model, &ask);
    let request = rules::Request { repository: translate::repository(repository), act, permission };
    let mut findings = Queue::with_capacity(rules::max_out(&env.limits.rules));
    if rules::check_request(&model.config.rules, &request, &mut findings) != rules::Decision::Allow {
        return reply(model, to, Reply::Refused(Refusal::Unpermitted));
    }
    permitted(model, env, to, person, ask, permission);
}

/// What a call does, in the rules' terms, and on which repository.
fn act(model: &Model, ask: &Ask) -> (u32, rules::Act) {
    match ask {
        Ask::Open { repository, .. } => (*repository, rules::Act::Open),
        Ask::Message { item, .. } => (item.repository, rules::Act::Steer),
        Ask::Accept { item } => (item.repository, rules::Act::Accept { permission: wants(model, *item) }),
        Ask::Reject { item } => (item.repository, rules::Act::Reject { permission: wants(model, *item) }),
        Ask::Stop { item } => (item.repository, rules::Act::Cancel),
        Ask::Release { item } => (item.repository, rules::Act::Release),
        Ask::Watch { subject } => match subject {
            Watched::Run { item } | Watched::Item { item } => (item.repository, rules::Act::Watch),
            Watched::Board { repository } => (*repository, rules::Act::Watch),
        },
    }
}

/// The permission the rules want of whoever accepts what the item holds: a
/// writer's, unless they said otherwise.
fn wants(model: &Model, item: Item) -> rules::Permission {
    let wanted = match items::find(model, item) {
        Some(id) => match model.items.get(id) {
            Some(entry) => entry.wants,
            None => None,
        },
        None => None,
    };
    wanted.unwrap_or(rules::Permission::Write)
}

/// A call the rules allow, acted on.
fn permitted(model: &mut Model, env: &Env<Limits>, to: ReplyTo, person: u64, ask: Ask, permission: rules::Permission) {
    match ask {
        Ask::Open { repository, key, title, message } => {
            let mut labels = List::with_capacity(1);
            labels.push(copy_of(&model.config.forge.tracking)).expect("room for the tracking label");
            let write = forge::Write::CreateIssue {
                repository,
                key: copy_of(&key),
                title,
                body: forge::Content::Text(message),
                labels: labels.into_boxed(),
            };
            let ask = Ask::Open { repository, key, title: Box::new([]), message: Box::new([]) };
            writing(model, env, to, person, ask, write);
        }
        Ask::Message { item, key, message } => {
            let write = forge::Write::Comment {
                item: translate::forge_item(item),
                key: copy_of(&key),
                person: Some(person),
                body: forge::Content::Text(message),
            };
            let ask = Ask::Message { item, key, message: Box::new([]) };
            writing(model, env, to, person, ask, write);
        }
        Ask::Accept { item } => decide(model, env, to, person, item, Some(permission)),
        Ask::Reject { item } => decide(model, env, to, person, item, None),
        Ask::Stop { item } => {
            let reply_to = hub(model, to, person, Ask::Stop { item });
            route::work_step(model, env, work::Event::Stop { reply_to, item });
        }
        Ask::Release { item } => {
            let Some(id) = items::find(model, item) else { return reply(model, to, Reply::Refused(Refusal::Unknown)) };
            let known = match model.items.get(id) {
                Some(entry) => entry.step.is_some(),
                None => false,
            };
            if !known {
                return reply(model, to, Reply::Refused(Refusal::Unknown));
            }
            release_into(model, env, id);
            let reply_to = hub(model, to, person, Ask::Release { item });
            route::work_step(model, env, work::Event::Release { reply_to, item });
        }
        Ask::Watch { subject } => {
            model.watchers = model.watchers.saturating_add(1);
            let watcher = Token::new(model.watchers);
            let subject = match subject {
                Watched::Run { item } => views::Subject::Run(translate::run_of(item)),
                Watched::Item { item } => views::Subject::Item(translate::run_of(item)),
                Watched::Board { repository } => views::Subject::Board(repository),
            };
            if model.watches.insert(watcher, to).is_err() {
                unreachable!("the watches being taken have room for every person's call");
            }
            route::views_step(model, env, views::Event::Watch { watcher, subject });
        }
    }
}

/// Asks the forge to write what a person's call makes.
fn writing(model: &mut Model, env: &Env<Limits>, to: ReplyTo, person: u64, ask: Ask, write: forge::Write) {
    let wait = Wait::Person { to, person, ask };
    let wait = match model.waits.insert(wait) {
        Ok(wait) => wait,
        Err(Wait::Person { to, .. }) => return reply(model, to, Reply::Refused(Refusal::Busy)),
        Err(_) => unreachable!("a person's call is refused as it was asked"),
    };
    route::forge_step(model, env, forge::Event::Write { owner: wait.token(), write, resumed: None });
}

/// A wait for the hub's answer to a person's call.
fn hub(model: &mut Model, to: ReplyTo, person: u64, ask: Ask) -> ReplyTo {
    let wait = Wait::Person { to, person, ask };
    let Ok(wait) = model.waits.insert(wait) else { unreachable!("the person's call had a wait a moment ago") };
    ReplyTo::new(wait.token())
}

/// A person's decision on the item: kept in its record; an item held for
/// their acceptance is released, to apply its outcome again with it.
fn decide(
    model: &mut Model,
    env: &Env<Limits>,
    to: ReplyTo,
    person: u64,
    item: Item,
    accepted: Option<rules::Permission>,
) {
    let Some(id) = items::find(model, item) else { return reply(model, to, Reply::Refused(Refusal::Unknown)) };
    let Some(entry) = model.items.get_mut(id) else { return reply(model, to, Reply::Refused(Refusal::Unknown)) };
    let decision = if accepted.is_some() { plan::Decision::Accepted } else { plan::Decision::Rejected };
    entry.relations.decision = Some(plan::Decided { decision, at: env.now });
    entry.relations.accepted = accepted;
    let held = match entry.lifecycle.phase {
        work::Phase::Held { why, .. } => why == work::Hold::Acceptance,
        work::Phase::Waiting
        | work::Phase::Parked
        | work::Phase::Retrying(_)
        | work::Phase::Claimed
        | work::Phase::Applying { .. }
        | work::Phase::Done => false,
    };
    if held {
        let ask = if accepted.is_some() { Ask::Accept { item } } else { Ask::Reject { item } };
        let reply_to = hub(model, to, person, ask);
        return route::work_step(model, env, work::Event::Release { reply_to, item });
    }
    items::aside(model, env, id);
    items::notice(model, env, id, Inbound::Decided { accepted: accepted.is_some() }, plan::Source::Message);
    reply(model, to, Reply::Done);
}

/// What a person's release writes into the item's step, as the plan says:
/// it lifts what held the item. A pull request closed unmerged is opened
/// again.
pub(crate) fn release_into(model: &mut Model, env: &Env<Limits>, id: Id<Entry>) {
    let Some(entry) = model.items.get(id) else { return };
    let Some(record) = entry.step.as_ref() else { return };
    let facts = crate::jobs::facts(model, env, entry);
    let mut writes = Queue::with_capacity(plan::max_out(&env.limits.plan));
    plan::release(&route::plan_env(env), record, &facts, &mut writes);
    let mut reopen = false;
    for _ in 0..writes.len() {
        let Some(write) = writes.pop() else { break };
        match write {
            plan::Write::Progress(progress) => {
                let Some(entry) = model.items.get_mut(id) else { return };
                if let Some(record) = entry.step.as_mut() {
                    record.progress = progress;
                }
            }
            plan::Write::ReopenPull => reopen = true,
            plan::Write::Create { .. }
            | plan::Write::OpenPull { .. }
            | plan::Write::Merge { .. }
            | plan::Write::Close
            | plan::Write::DeleteBranch
            | plan::Write::Goal(_)
            | plan::Write::Release { .. } => {}
        }
    }
    let Some(entry) = model.items.get_mut(id) else { return };
    entry.blocked = false;
    let item = entry.item;
    if reopen && let Some(pull) = entry.relations.pull {
        let Ok(wait) = model.waits.insert(Wait::Aside { entry: None }) else {
            unreachable!("the waits have room for every item's write")
        };
        let write = forge::Write::Reopen { item: forge::Item { repository: item.repository, number: pull } };
        route::forge_step(model, env, forge::Event::Write { owner: wait.token(), write, resumed: None });
    }
}

/// A forge write for a person's call ended.
pub(crate) fn wrote(
    model: &mut Model,
    env: &Env<Limits>,
    to: ReplyTo,
    ask: Ask,
    result: Result<forge::Written, forge::Failure>,
) {
    match ask {
        Ask::Open { repository, .. } => {
            let Ok(forge::Written::Created(number)) = result else {
                return reply(model, to, Reply::Refused(Refusal::Failed));
            };
            let item = Item { repository, number };
            session(model, env, item);
            reply(model, to, Reply::Opened { item });
        }
        Ask::Message { .. } => match result {
            Ok(_) => reply(model, to, Reply::Done),
            Err(_) => reply(model, to, Reply::Refused(Refusal::Failed)),
        },
        Ask::Accept { .. } | Ask::Reject { .. } | Ask::Stop { .. } | Ask::Release { .. } | Ask::Watch { .. } => {
            reply(model, to, Reply::Done);
        }
    }
}

/// Holds `item` as a session (section 6): the step configured for one.
fn session(model: &mut Model, env: &Env<Limits>, item: Item) {
    let Some(id) = items::hold(model, env, item) else { return };
    let Some(entry) = model.items.get_mut(id) else { return };
    if entry.step.is_some() {
        return;
    }
    let step = plan::Step {
        name: copy_of(b"session"),
        repository: plan::Repository(item.repository),
        work: plan::Work::Session(model.config.session.clone()),
        after: Box::new([]),
        gates: Box::new([]),
    };
    entry.step = Some(plan::Record { step, progress: plan::Progress::NEW, goal: None });
}

/// An issue handed in, by its label (4.6): taken in as a session.
pub(crate) fn offered(model: &mut Model, env: &Env<Limits>, item: forge::Item) {
    session(model, env, translate::item_of(item));
}

/// The working set has no room for an item the top level asked for: it is
/// tracked again once it has.
pub(crate) fn full(model: &mut Model, item: forge::Item) {
    let item = translate::item_of(item);
    if items::find(model, item).is_some() && model.roomless.try_push(item).is_err() {
        // As many as the table holds wait already: this one is found again by
        // a listing, or offered again.
    }
}

/// The working set has room again: the items that found none are tracked
/// again.
pub(crate) fn room(model: &mut Model, env: &Env<Limits>) {
    for _ in 0..model.roomless.len() {
        let Some(item) = model.roomless.pop() else { break };
        route::forge_step(model, env, forge::Event::Track { item: translate::forge_item(item) });
    }
}

/// The hub answered a call the top level made of it: a take, or a person's
/// stop or release.
pub(crate) fn hub_answered(model: &mut Model, env: &Env<Limits>, to: ReplyTo, result: Result<(), work::Refusal>) {
    let Some(wait) = serve::take(model, to.into_token()) else { return };
    match wait {
        Wait::Take { entry } => items::taken(model, env, entry, result.is_ok()),
        Wait::Person { to, .. } => {
            let answer = match result {
                Ok(()) => Reply::Done,
                Err(refusal) => Reply::Refused(refusal_of(refusal)),
            };
            reply(model, to, answer);
        }
        Wait::Job { .. }
        | Wait::Aside { .. }
        | Wait::Brief { .. }
        | Wait::Wiki { .. }
        | Wait::Relay { .. }
        | Wait::Views { .. }
        | Wait::Done => {}
    }
}

const fn refusal_of(refusal: work::Refusal) -> Refusal {
    match refusal {
        work::Refusal::Full | work::Refusal::Taken => Refusal::Busy,
        work::Refusal::Done | work::Refusal::Unknown => Refusal::Unknown,
        work::Refusal::Idle => Refusal::Idle,
        work::Refusal::Unheld => Refusal::Unheld,
    }
}

/// The views answered a watch.
pub(crate) fn watching(model: &mut Model, watcher: Token, refusal: Option<views::Refusal>, out: &mut Queue<Request>) {
    let Some(to) = model.watches.remove(&watcher) else { return };
    let reply = match refusal {
        None => Reply::Watching { watcher },
        Some(views::Refusal::Busy) => Reply::Refused(Refusal::Busy),
        Some(views::Refusal::Unknown) => Reply::Refused(Refusal::Unfollowed),
    };
    out.push(Request::Reply { to, reply });
}

/// Answers a person.
fn reply(model: &mut Model, to: ReplyTo, reply: Reply) {
    model.requests.push(Request::Reply { to, reply });
}
