//! People (engine-domain.md, sections 2, 4.6, 6 and 7): their calls through
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
//!   again with the decision; any other is told of it. An acceptance of an
//!   outcome counts for it alone; one of the step, for everything the step
//!   runs and writes until it is released or done (engine-domain.md, 5.1).
//! - **Stop, release:** the hub's; once the hub releases the item, and
//!   before its record is written, the plan says what the release writes,
//!   which lifts what held it. A release the hub refuses changes nothing.
//! - **Watch:** the views'.
//!
//! An issue handed in by its label becomes a session (4.6). The working set
//! may have no room for an item: those the top level asked for are tracked
//! again once it has.

use alloc::boxed::Box;

use skein_lib::bytes::copy_of;
use skein_lib::{Env, Id, List, Queue, ReplyTo, Time, Token};
use temper_engine_domain_forge::{self as forge, api};
use temper_engine_domain_plan as plan;
use temper_engine_domain_rules as rules;
use temper_engine_domain_views as views;
use temper_engine_domain_work as work;

use crate::boundary::{Ask, Inbound, Item, Phase, Refusal, Reply, Request, Watched};
use crate::domain::Domain;
use crate::items::{self, Entry};
use crate::limits::Limits;
use crate::route;
use crate::serve;
use crate::translate;
use crate::waits::Wait;

/// A person's call.
pub(crate) fn ask(
    domain: &mut Domain,
    env: &Env<Limits>,
    to: ReplyTo,
    person: u64,
    ask: Ask,
    out: &mut Queue<Request>,
) {
    let repository = match &ask {
        Ask::Open { repository, .. } | Ask::Watch { subject: Watched::Board { repository } } => *repository,
        Ask::Message { item, .. }
        | Ask::Accept { item }
        | Ask::Reject { item }
        | Ask::Stop { item }
        | Ask::Release { item }
        | Ask::Watch { subject: Watched::Run { item } | Watched::Item { item } } => {
            if items::find(domain, *item).is_none() {
                return out.push(Request::Reply { to, reply: Reply::Refused(Refusal::Unknown) });
            }
            item.repository
        }
    };
    if repository >= domain.config.repositories() {
        return out.push(Request::Reply { to, reply: Reply::Refused(Refusal::Unknown) });
    }
    let read = forge::Read::Permission { repository, user: person };
    let wait = Wait::Person { to, person, ask };
    let wait = match domain.waits.insert(wait) {
        Ok(wait) => wait,
        Err(Wait::Person { to, .. }) => return out.push(Request::Reply { to, reply: Reply::Refused(Refusal::Busy) }),
        Err(_) => unreachable!("a person's call is refused as it was asked"),
    };
    route::forge_step(domain, env, forge::Event::Read { owner: wait.token(), read });
}

/// A forge read for a person's call ended: their permission.
pub(crate) fn read(
    domain: &mut Domain,
    env: &Env<Limits>,
    to: ReplyTo,
    person: u64,
    ask: Ask,
    result: Result<api::Answer, forge::Failure>,
) {
    let permission = match result {
        Ok(api::Answer::Permission(permission)) => translate::permission(permission),
        Err(forge::Failure::Busy) => return reply(domain, to, Reply::Refused(Refusal::Busy)),
        Ok(_) | Err(_) => return reply(domain, to, Reply::Refused(Refusal::Failed)),
    };
    let (repository, act) = act(domain, &ask);
    let request = rules::Request { repository: translate::repository(repository), act, permission };
    let mut findings = Queue::with_capacity(rules::max_out(&env.limits.rules));
    if rules::check_request(&domain.config.rules, &request, &mut findings) != rules::Decision::Allow {
        return reply(domain, to, Reply::Refused(Refusal::Unpermitted));
    }
    permitted(domain, env, to, person, ask, permission);
}

/// What a call does, in the rules' terms, and on which repository.
fn act(domain: &Domain, ask: &Ask) -> (u32, rules::Act) {
    match ask {
        Ask::Open { repository, .. } => (*repository, rules::Act::Open),
        Ask::Message { item, .. } => (item.repository, rules::Act::Steer),
        Ask::Accept { item } => (item.repository, rules::Act::Accept { permission: wants(domain, *item) }),
        Ask::Reject { item } => (item.repository, rules::Act::Reject { permission: wants(domain, *item) }),
        Ask::Stop { item } => (item.repository, rules::Act::Cancel),
        Ask::Release { item } => (item.repository, rules::Act::Release),
        Ask::Watch { subject } => match subject {
            Watched::Run { item } | Watched::Item { item } => (item.repository, rules::Act::Watch),
            Watched::Board { repository } => (*repository, rules::Act::Watch),
        },
    }
}

/// The permission the rules want of whoever accepts what the item holds, as
/// its record keeps it across restarts: a writer's, unless they said
/// otherwise.
fn wants(domain: &Domain, item: Item) -> rules::Permission {
    let wanted = match items::find(domain, item) {
        Some(id) => match domain.items.get(id) {
            Some(entry) => entry.relations.wants,
            None => None,
        },
        None => None,
    };
    wanted.unwrap_or(rules::Permission::Write)
}

/// A call the rules allow, acted on.
fn permitted(
    domain: &mut Domain,
    env: &Env<Limits>,
    to: ReplyTo,
    person: u64,
    ask: Ask,
    permission: rules::Permission,
) {
    match ask {
        Ask::Open { repository, key, title, message } => {
            let mut labels = List::with_capacity(1);
            labels.push(copy_of(&domain.config.forge.tracking)).expect("room for the tracking label");
            let write = forge::Write::CreateIssue {
                repository,
                key: copy_of(&key),
                title,
                body: forge::Content::Text(message),
                labels: labels.into_boxed(),
            };
            let ask = Ask::Open { repository, key, title: Box::new([]), message: Box::new([]) };
            writing(domain, env, to, person, ask, write);
        }
        Ask::Message { item, key, message } => {
            let write = forge::Write::Comment {
                item: translate::forge_item(item),
                key: copy_of(&key),
                person: Some(person),
                body: forge::Content::Text(message),
            };
            let ask = Ask::Message { item, key, message: Box::new([]) };
            writing(domain, env, to, person, ask, write);
        }
        Ask::Accept { item } => decide(domain, env, to, person, item, Some(permission)),
        Ask::Reject { item } => decide(domain, env, to, person, item, None),
        Ask::Stop { item } => {
            let reply_to = hub(domain, to, person, Ask::Stop { item });
            route::work_step(domain, env, work::Event::Stop { reply_to, item });
        }
        Ask::Release { item } => {
            let Some(id) = items::find(domain, item) else {
                return reply(domain, to, Reply::Refused(Refusal::Unknown));
            };
            let known = match domain.items.get(id) {
                Some(entry) => entry.step.is_some(),
                None => false,
            };
            if !known {
                return reply(domain, to, Reply::Refused(Refusal::Unknown));
            }
            let reply_to = hub(domain, to, person, Ask::Release { item });
            route::work_step(domain, env, work::Event::Release { reply_to, item });
        }
        Ask::Watch { subject } => {
            domain.watchers = domain.watchers.saturating_add(1);
            let watcher = Token::new(domain.watchers);
            let subject = match subject {
                Watched::Run { item } => views::Subject::Run(translate::run_of(item)),
                Watched::Item { item } => views::Subject::Item(translate::run_of(item)),
                Watched::Board { repository } => views::Subject::Board(repository),
            };
            if domain.watches.insert(watcher, to).is_err() {
                unreachable!("the watches being taken have room for every person's call");
            }
            let snapshot = snapshot(domain, subject, env.limits.views.snapshot_bytes);
            route::views_step(domain, env, views::Event::Watch { watcher, subject, snapshot });
        }
    }
}

/// Asks the forge to write what a person's call makes, keyed by their
/// request. The same request may have been asked of an earlier life, which
/// may have made it: what it makes is looked for first, anywhere, as
/// nothing says when that was.
fn writing(domain: &mut Domain, env: &Env<Limits>, to: ReplyTo, person: u64, ask: Ask, write: forge::Write) {
    let wait = Wait::Person { to, person, ask };
    let wait = match domain.waits.insert(wait) {
        Ok(wait) => wait,
        Err(Wait::Person { to, .. }) => return reply(domain, to, Reply::Refused(Refusal::Busy)),
        Err(_) => unreachable!("a person's call is refused as it was asked"),
    };
    let resumed = Some(forge::Cause { comment: 0, at: Time::ZERO });
    route::forge_step(domain, env, forge::Event::Write { owner: wait.token(), write, resumed });
}

/// A wait for the hub's answer to a person's call.
fn hub(domain: &mut Domain, to: ReplyTo, person: u64, ask: Ask) -> ReplyTo {
    let wait = Wait::Person { to, person, ask };
    let Ok(wait) = domain.waits.insert(wait) else { unreachable!("the person's call had a wait a moment ago") };
    ReplyTo::new(wait.token())
}

/// A person's decision on the item: kept in its record, with the outcome it
/// is on; an item held for their acceptance is released, to apply its
/// outcome again (or make its run or action) with it. A decision is taken
/// only on what waits for one: an item held for acceptance, or a step that
/// waits for a person's decision or acceptance.
fn decide(
    domain: &mut Domain,
    env: &Env<Limits>,
    to: ReplyTo,
    person: u64,
    item: Item,
    accepted: Option<rules::Permission>,
) {
    let Some(id) = items::find(domain, item) else { return reply(domain, to, Reply::Refused(Refusal::Unknown)) };
    let Some(entry) = domain.items.get_mut(id) else { return reply(domain, to, Reply::Refused(Refusal::Unknown)) };
    let held = match entry.lifecycle.phase {
        work::Phase::Held { why: work::Hold::Acceptance, outcome } => Some(outcome),
        work::Phase::Held { why: work::Hold::Plan { reason }, .. } if reason == translate::RUN_ACCEPTANCE => Some(None),
        work::Phase::Held { .. }
        | work::Phase::Waiting
        | work::Phase::Parked
        | work::Phase::Retrying(_)
        | work::Phase::Claimed
        | work::Phase::Applying { .. }
        | work::Phase::Done => None,
    };
    let waits = match entry.step.as_ref() {
        Some(record) => waits_for_decision(&record.step),
        None => false,
    };
    if held.is_none() && !waits {
        return reply(domain, to, Reply::Refused(Refusal::Unheld));
    }
    let decision = if accepted.is_some() { plan::Decision::Accepted } else { plan::Decision::Rejected };
    entry.relations.decision = Some(plan::Decided { decision, at: env.now });
    entry.relations.accepted = accepted;
    entry.relations.accepting = held.flatten();
    if held.is_some() {
        let ask = if accepted.is_some() { Ask::Accept { item } } else { Ask::Reject { item } };
        let reply_to = hub(domain, to, person, ask);
        return route::work_step(domain, env, work::Event::Release { reply_to, item });
    }
    items::aside(domain, env, id);
    items::notice(domain, env, id, Inbound::Decided { accepted: accepted.is_some() }, plan::Source::Message);
    reply(domain, to, Reply::Done);
}

/// Whether a step waits for a person's decision on its item: a wait for
/// one, or a step gated on its acceptance.
fn waits_for_decision(step: &plan::Step) -> bool {
    let mut gated = false;
    for gate in &step.gates {
        match gate {
            plan::Gate::Accepted => gated = true,
            plan::Gate::Approvals(_) => {}
        }
    }
    let waits = match step.work {
        plan::Work::Wait(spec) => spec == plan::WaitSpec::Decision,
        plan::Work::Agent(_) | plan::Work::Change(_) | plan::Work::Session(_) => false,
    };
    gated || waits
}

/// What a release writes into the item's step, as the plan says: it lifts
/// what held the item. A pull request closed unmerged is opened again. Made
/// only once the hub has released the item, before the record it writes
/// next: a release the hub refuses (the item is not held) changes nothing.
pub(crate) fn release_into(domain: &mut Domain, env: &Env<Limits>, id: Id<Entry>) {
    let Some(entry) = domain.items.get(id) else { return };
    let Some(record) = entry.step.as_ref() else { return };
    let facts = crate::jobs::facts(domain, env, entry);
    let mut writes = Queue::with_capacity(plan::max_out(&env.limits.plan));
    plan::release(&route::plan_env(env), record, &facts, &mut writes);
    let mut reopen = false;
    for _ in 0..writes.len() {
        let Some(write) = writes.pop() else { break };
        match write {
            plan::Write::Progress(progress) => {
                let Some(entry) = domain.items.get_mut(id) else { return };
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
    let Some(entry) = domain.items.get_mut(id) else { return };
    entry.blocked = false;
    // A decision on the step made before the release counts for nothing
    // after it (engine-domain.md, 5.3).
    if entry.relations.accepting.is_none() {
        entry.relations.decision = None;
        entry.relations.accepted = None;
        entry.relations.wants = None;
    }
    let item = entry.item;
    if reopen && let Some(pull) = entry.relations.pull {
        let Ok(wait) = domain.waits.insert(Wait::Aside { entry: None }) else {
            unreachable!("the waits have room for every item's write")
        };
        let write = forge::Write::Reopen { item: forge::Item { repository: item.repository, number: pull } };
        route::forge_step(domain, env, forge::Event::Write { owner: wait.token(), write, resumed: None });
    }
}

/// A forge write for a person's call ended.
pub(crate) fn wrote(
    domain: &mut Domain,
    env: &Env<Limits>,
    to: ReplyTo,
    ask: Ask,
    result: Result<forge::Written, forge::Failure>,
) {
    match ask {
        Ask::Open { repository, .. } => {
            let created = match result {
                Ok(forge::Written::Created(number)) => Some(number),
                Ok(
                    forge::Written::Commented(_)
                    | forge::Written::Merged(_)
                    | forge::Written::Revision(_)
                    | forge::Written::Reviewed(_)
                    | forge::Written::Done,
                )
                | Err(_) => None,
            };
            let Some(number) = created else { return reply(domain, to, Reply::Refused(Refusal::Failed)) };
            opened(domain, env, to, Item { repository, number });
        }
        Ask::Message { .. } => match result {
            Ok(_) => reply(domain, to, Reply::Done),
            Err(_) => reply(domain, to, Reply::Refused(Refusal::Failed)),
        },
        Ask::Accept { .. } | Ask::Reject { .. } | Ask::Stop { .. } | Ask::Release { .. } | Ask::Watch { .. } => {
            reply(domain, to, Reply::Done);
        }
    }
}

/// The session a person opened is made: it is held, and the person
/// answered once its first record is written (at once, if the engine took
/// it in already).
fn opened(domain: &mut Domain, env: &Env<Limits>, to: ReplyTo, item: Item) {
    let Some(id) = session(domain, env, item) else { return reply(domain, to, Reply::Refused(Refusal::Busy)) };
    let Some(entry) = domain.items.get_mut(id) else { return reply(domain, to, Reply::Refused(Refusal::Busy)) };
    if entry.taking == items::Taking::Taken {
        return reply(domain, to, Reply::Opened { item });
    }
    if let Some(earlier) = entry.opened.replace(to) {
        reply(domain, earlier, Reply::Refused(Refusal::Busy));
    }
}

/// Holds `item` as a session (section 6): the step configured for one.
fn session(domain: &mut Domain, env: &Env<Limits>, item: Item) -> Option<Id<Entry>> {
    let id = items::hold(domain, env, item)?;
    let entry = domain.items.get_mut(id)?;
    if entry.step.is_some() {
        return Some(id);
    }
    let step = domain.config.session_step(item.repository);
    entry.step = Some(plan::Record { step, progress: plan::Progress::NEW, goal: None });
    Some(id)
}

/// An issue handed in, by its label (4.6): taken in as a session.
pub(crate) fn offered(domain: &mut Domain, env: &Env<Limits>, item: forge::Item) {
    session(domain, env, translate::item_of(item));
}

/// The working set has no room for an item the top level asked for: it is
/// tracked again once it has.
pub(crate) fn full(domain: &mut Domain, item: forge::Item) {
    let item = translate::item_of(item);
    if items::find(domain, item).is_some() && domain.roomless.try_push(item).is_err() {
        // As many as the table holds wait already: this one is found again by
        // a listing, or offered again.
    }
}

/// The working set has room again: the items that found none are tracked
/// again.
pub(crate) fn room(domain: &mut Domain, env: &Env<Limits>) {
    for _ in 0..domain.roomless.len() {
        let Some(item) = domain.roomless.pop() else { break };
        route::forge_step(domain, env, forge::Event::Track { item: translate::forge_item(item) });
    }
}

/// The hub answered a call the top level made of it: a take, or a person's
/// stop or release.
pub(crate) fn hub_answered(domain: &mut Domain, env: &Env<Limits>, to: ReplyTo, result: Result<(), work::Refusal>) {
    let Some(wait) = serve::take(domain, to.into_token()) else { return };
    match wait {
        Wait::Take { entry, written } => items::taken(domain, env, entry, written, result.is_ok()),
        Wait::Release { entry } => {
            if result.is_ok() {
                release_into(domain, env, entry);
            }
        }
        Wait::Person { to, ask: Ask::Release { item }, .. } => {
            if result.is_ok()
                && let Some(id) = items::find(domain, item)
            {
                release_into(domain, env, id);
            }
            let answer = match result {
                Ok(()) => Reply::Done,
                Err(refusal) => Reply::Refused(refusal_of(refusal)),
            };
            reply(domain, to, answer);
        }
        Wait::Person { to, .. } => {
            let answer = match result {
                Ok(()) => Reply::Done,
                Err(refusal) => Reply::Refused(refusal_of(refusal)),
            };
            reply(domain, to, answer);
        }
        Wait::Job { .. }
        | Wait::Record { .. }
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
pub(crate) fn watching(domain: &mut Domain, watcher: Token, refusal: Option<views::Refusal>, out: &mut Queue<Request>) {
    let Some(to) = domain.watches.remove(&watcher) else { return };
    let reply = match refusal {
        None => Reply::Watching { watcher },
        Some(views::Refusal::Busy | views::Refusal::Oversized) => Reply::Refused(Refusal::Busy),
        Some(views::Refusal::Unknown | views::Refusal::Unfollowed) => Reply::Refused(Refusal::Unfollowed),
    };
    out.push(Request::Reply { to, reply });
}

/// What a watch begins from: a line per item it covers (the item, or the
/// board's), its number and its phase, as many as fit `most` bytes.
fn snapshot(domain: &Domain, subject: views::Subject, most: u32) -> Box<[u8]> {
    let mut lines: List<Box<[u8]>> = List::with_capacity(domain.names.len().max(1));
    let mut room = usize::try_from(most).unwrap_or(usize::MAX);
    for (item, id) in &domain.names {
        let covered = match subject {
            views::Subject::Run(run) | views::Subject::Item(run) => translate::run(*item) == Some(run),
            views::Subject::Board(repository) => item.repository == repository,
        };
        let Some(entry) = domain.items.get(*id) else { continue };
        if !covered {
            continue;
        }
        let phase = match entry.live {
            Some(live) if live.started => Phase::Running,
            Some(_) | None => translate::phase(entry.lifecycle.phase),
        };
        let line = translate::concat(&[&translate::decimal(item.number), b" ", phase.name(), b"\n"]);
        if line.len() > room {
            break;
        }
        room = room.saturating_sub(line.len());
        lines.push(line).expect("a line per item held");
    }
    let mut parts: List<&[u8]> = List::with_capacity(lines.len());
    for line in &lines {
        parts.push(&line[..]).expect("a part per line");
    }
    translate::concat(parts.as_slice())
}

/// Answers a person.
fn reply(domain: &mut Domain, to: ReplyTo, reply: Reply) {
    domain.requests.push(Request::Reply { to, reply });
}
