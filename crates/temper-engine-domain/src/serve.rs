//! Serving what the child domains and runs ask of the forge, the notes and the
//! store, and routing the answers back by their waits.
//!
//! - **Forge calls** go out with the payloads they name filled in: an
//!   item's record, composed as the call goes out; an outcome posted; a
//!   note's page.
//! - **A brief's reads** (engine-domain.md, section 9): the item and the
//!   comments since its runs' last turn (people's, the oldest first, whole,
//!   as many as fit: the rest wait for the next run, which takes them), CI
//!   failures and reviews on its pull request's head, and how that head
//!   stands against its base, read from the forge; its earlier attempts and its plan's status from its record;
//!   the notes' index from the notes; a template from the configuration.
//!   Each is cut to what the brief asked for. Its dependencies' outcomes
//!   are not read yet: that section is missing.
//! - **The notes' wiki** (section 10): a scope's pages are a repository's
//!   wiki's top-level pages, a goal's under `goals/<number>/` in its
//!   repository's wiki, and the deployment's under `deployment/` in its home
//!   repository's wiki. An edit reads the page first, and is made only if it
//!   is still at the revision read.
//! - **A run's calls:** a forge read within its grants, a `recall`, a `note`
//!   the rules allow, a comment and an escalation on its item.

use alloc::boxed::Box;
use core::mem;

use skein_lib::bytes::copy_of;
use skein_lib::{Env, Id, List, Queue, ReplyTo, Time, Token};
use temper_engine_domain_brief as brief;
use temper_engine_domain_forge::{self as forge, api};
use temper_engine_domain_notes as notes;
use temper_engine_domain_plan as plan;
use temper_engine_domain_rules as rules;
use temper_engine_domain_views as views;
use temper_engine_domain_work as work;

use crate::boundary::{Call, Decoded, Inbound, Item, Payload, Request, Served, Store, Stored, Unserved};
use crate::domain::Domain;
use crate::items::{self, Job};
use crate::jobs;
use crate::limits::Limits;
use crate::people;
use crate::route;
use crate::runs;
use crate::translate;
use crate::waits::{Bounds, Wait, Wiki};

/// Takes the wait `token` names, which is answered: it goes at the reclaim
/// point. Missing or already answered tokens are boundary-contract bugs.
pub(crate) fn take(domain: &mut Domain, token: Token) -> Wait {
    let id = Id::from_token(token);
    let wait = domain.waits.get_mut(id).expect("a terminal's upward token names an outstanding wait");
    let answered = mem::replace(wait, Wait::Done);
    match answered {
        Wait::Done => unreachable!("each wait receives exactly one terminal"),
        Wait::Job { .. }
        | Wait::Take { .. }
        | Wait::Record { .. }
        | Wait::Aside { .. }
        | Wait::Release { .. }
        | Wait::Brief { .. }
        | Wait::Wiki { .. }
        | Wait::Relay { .. }
        | Wait::Person { .. }
        | Wait::Views { .. } => {
            domain.waits.retire(id);
            answered
        }
    }
}

/// A forge call goes out, the payload it names filled in.
pub(crate) fn call(domain: &Domain, call: Token, repository: u32, op: api::Op, out: &mut Queue<Request>) {
    let payload = match named(&op) {
        Some(token) => payload(domain, token),
        None => None,
    };
    out.push(Request::Forge { call, repository, op, payload });
}

/// The token of the payload `op` carries, if it carries one.
fn named(op: &api::Op) -> Option<Token> {
    let body = match op {
        api::Op::CreateIssue { body, .. }
        | api::Op::Post { body, .. }
        | api::Op::EditComment { body, .. }
        | api::Op::OpenPull { body, .. }
        | api::Op::Review { body, .. } => body,
        api::Op::PutPage { content, .. } => content,
        api::Op::Items { .. }
        | api::Op::Item { .. }
        | api::Op::Comment { .. }
        | api::Op::Pull { .. }
        | api::Op::PullFor { .. }
        | api::Op::Reviews { .. }
        | api::Op::Statuses { .. }
        | api::Op::Remarks { .. }
        | api::Op::Permission { .. }
        | api::Op::Branch { .. }
        | api::Op::Pages { .. }
        | api::Op::Page { .. }
        | api::Op::AddLabels { .. }
        | api::Op::RemoveLabels { .. }
        | api::Op::Merge { .. }
        | api::Op::SetReviewers { .. }
        | api::Op::SetDependencies { .. }
        | api::Op::Close { .. }
        | api::Op::Reopen { .. }
        | api::Op::DeleteBranch { .. }
        | api::Op::DeletePage { .. } => return None,
    };
    match body {
        api::Body::Text(_) => None,
        api::Body::Payload(token) | api::Body::Record { payload: token, .. } => Some(*token),
    }
}

/// What the wait `token` says the payload is: the record of the item it is
/// for, the outcome it posts, or the page it writes.
fn payload(domain: &Domain, token: Token) -> Option<Payload> {
    match domain.waits.get(Id::from_token(token))? {
        Wait::Job { entry } | Wait::Take { entry, .. } => {
            let entry = domain.items.get(*entry)?;
            match &entry.job {
                Job::Recording { outcome, .. } => Some(Payload::Outcome(runs::posted(domain, *outcome)?)),
                Job::Idle | Job::Asking { .. } | Job::Writing { .. } | Job::Applying(_) | Job::Starting(_) => {
                    Some(Payload::Record(Box::new(entry.record()?)))
                }
            }
        }
        // A record written on the side: an entry the hub is done with stays
        // until its last such write has gone out.
        Wait::Record { entry } => Some(Payload::Record(Box::new(domain.items.get(*entry)?.record()?))),
        Wait::Wiki { op: Wiki::Create { page }, .. } | Wait::Wiki { op: Wiki::Edit { page }, .. } => {
            Some(Payload::Page(page.clone()))
        }
        Wait::Aside { .. }
        | Wait::Release { .. }
        | Wait::Wiki { .. }
        | Wait::Brief { .. }
        | Wait::Relay { .. }
        | Wait::Person { .. }
        | Wait::Views { .. }
        | Wait::Done => None,
    }
}

/// A forge read ended: to whoever asked.
pub(crate) fn read(domain: &mut Domain, env: &Env<Limits>, owner: Token, result: Result<api::Answer, forge::Failure>) {
    let wait = take(domain, owner);
    match wait {
        Wait::Job { entry } => jobs::read(domain, env, entry, Id::from_token(owner), result),
        Wait::Brief { owner, bounds, source } => {
            brief_got(domain, env, owner, bounds, &source, result);
        }
        Wait::Wiki { owner, op } => wiki_read(domain, env, owner, op, result),
        Wait::Relay { to, .. } => {
            let served = match result {
                Ok(answer) => Served::Read(answer),
                Err(forge::Failure::Busy) => Served::Unserved(Unserved::Busy),
                Err(forge::Failure::Invalid) => Served::Unserved(Unserved::Invalid),
                Err(_) => Served::Unserved(Unserved::Failed),
            };
            runs::serve_answer(domain, env, to, served);
        }
        Wait::Person { to, person, ask } => people::read(domain, env, to, person, ask, result),
        Wait::Take { .. }
        | Wait::Record { .. }
        | Wait::Aside { .. }
        | Wait::Release { .. }
        | Wait::Views { .. }
        | Wait::Done => {}
    }
}

/// A forge write ended: to whoever asked.
pub(crate) fn wrote(
    domain: &mut Domain,
    env: &Env<Limits>,
    owner: Token,
    result: Result<forge::Written, forge::Failure>,
) {
    let wait = take(domain, owner);
    match wait {
        Wait::Job { entry } => jobs::wrote(domain, env, entry, result),
        Wait::Wiki { owner, op } => wiki_wrote(domain, env, owner, op, result),
        Wait::Relay { to, .. } => {
            let served = match result {
                Ok(forge::Written::Commented(comment)) => Served::Posted { comment },
                Ok(_) | Err(_) => Served::Unserved(Unserved::Failed),
            };
            runs::serve_answer(domain, env, to, served);
        }
        Wait::Person { to, person: _, ask } => people::wrote(domain, env, to, ask, result),
        Wait::Record { entry } => items::aside_written(domain, entry, result),
        Wait::Take { .. }
        | Wait::Aside { .. }
        | Wait::Release { .. }
        | Wait::Brief { .. }
        | Wait::Views { .. }
        | Wait::Done => {}
    }
}

/// The store answered.
pub(crate) fn stored(domain: &mut Domain, env: &Env<Limits>, owner: Token, stored: Stored) {
    let wait = take(domain, owner);
    match wait {
        Wait::Job { entry } => {
            let snapshot = match stored {
                Stored::Got(snapshot) => snapshot,
                Stored::Done | Stored::Failed => None,
            };
            runs::fetched(domain, env, entry, Id::from_token(owner), snapshot);
        }
        Wait::Views { owner, expire } => {
            let done = stored == Stored::Done;
            let event =
                if expire { views::Event::Expired { owner, done } } else { views::Event::Appended { owner, done } };
            route::views_step(domain, env, event);
        }
        Wait::Aside { entry: Some(entry) } => {
            if stored == Stored::Failed
                && let Some(entry) = domain.items.get_mut(entry)
            {
                entry.relations.snapshot = false;
            }
        }
        Wait::Take { .. }
        | Wait::Record { .. }
        | Wait::Aside { entry: None }
        | Wait::Release { .. }
        | Wait::Brief { .. }
        | Wait::Wiki { .. }
        | Wait::Relay { .. }
        | Wait::Person { .. }
        | Wait::Done => {}
    }
}

/// The views append traces to the store.
pub(crate) fn append(
    domain: &mut Domain,
    env: &Env<Limits>,
    owner: Token,
    records: Box<[views::Record]>,
    out: &mut Queue<Request>,
) {
    let Ok(wait) = domain.waits.insert(Wait::Views { owner, expire: false }) else {
        return route::views_step(domain, env, views::Event::Appended { owner, done: false });
    };
    out.push(Request::Store { owner: wait.token(), op: Store::Append { traces: translate::traces(records) } });
}

/// The views expire traces.
pub(crate) fn expire(domain: &mut Domain, owner: Token, before: Time, out: &mut Queue<Request>) {
    let Ok(wait) = domain.waits.insert(Wait::Views { owner, expire: true }) else {
        unreachable!("the waits have room for the views' operations")
    };
    out.push(Request::Store { owner: wait.token(), op: Store::Expire { before } });
}

/// The brief reads a section's source.
pub(crate) fn brief_read(domain: &mut Domain, env: &Env<Limits>, owner: Token, source: brief::Source, bounds: Bounds) {
    let read = match &source {
        brief::Source::Template(index) => {
            let guidance = match domain.config.plan.templates.get(usize::try_from(*index).unwrap_or(usize::MAX)) {
                Some(template) => copy_of(&template.guidance),
                None => return answer_brief(domain, env, owner, brief::Read::Failed),
            };
            let got = cut(&[&guidance], bounds);
            return answer_brief(domain, env, owner, got);
        }
        brief::Source::Plan { goal } => {
            let got = plan_status(domain, translate::from_brief(*goal), bounds);
            return answer_brief(domain, env, owner, got);
        }
        brief::Source::Attempts(item) => {
            let got = attempts(domain, translate::from_brief(*item), bounds);
            return answer_brief(domain, env, owner, got);
        }
        brief::Source::Dependencies(_) => return answer_brief(domain, env, owner, brief::Read::Failed),
        brief::Source::Notes { repository, goal } => {
            let scopes = notes::Scopes { repository: *repository, goal: notes_item(*goal) };
            let wait = Wait::Brief { owner, bounds, source: source.clone() };
            let Ok(wait) = domain.waits.insert(wait) else {
                return answer_brief(domain, env, owner, brief::Read::Failed);
            };
            let reply_to = ReplyTo::new(wait.token());
            return route::notes_step(domain, env, notes::Event::Index { reply_to, scopes, budget: bounds.bytes });
        }
        brief::Source::Item(item) => {
            forge::Read::Item { item: translate::forge_item(translate::from_brief(*item)), after: u64::MAX }
        }
        brief::Source::Comments { item, since } => {
            forge::Read::Item { item: translate::forge_item(translate::from_brief(*item)), after: *since }
        }
        brief::Source::Ci { item, head } => {
            forge::Read::Statuses { repository: item.repository, commit: head.0, page: 1 }
        }
        brief::Source::Reviews { item, .. } | brief::Source::Pull { item, .. } => {
            let item = translate::from_brief(*item);
            let Some(pull) = pull_of(domain, item) else {
                return answer_brief(domain, env, owner, brief::Read::Failed);
            };
            let pull = forge::Item { repository: item.repository, number: pull };
            match &source {
                brief::Source::Reviews { .. } => forge::Read::Reviews { item: pull, page: 1 },
                brief::Source::Item(_)
                | brief::Source::Comments { .. }
                | brief::Source::Dependencies(_)
                | brief::Source::Ci { .. }
                | brief::Source::Pull { .. }
                | brief::Source::Attempts(_)
                | brief::Source::Plan { .. }
                | brief::Source::Notes { .. }
                | brief::Source::Template(_) => forge::Read::Pull { item: pull },
            }
        }
    };
    let Ok(wait) = domain.waits.insert(Wait::Brief { owner, bounds, source }) else {
        return answer_brief(domain, env, owner, brief::Read::Failed);
    };
    route::forge_step(domain, env, forge::Event::Read { owner: wait.token(), read });
}

fn notes_item(item: Option<brief::Item>) -> Option<notes::Item> {
    let item = item?;
    Some(notes::Item { repository: item.repository, number: item.number })
}

fn pull_of(domain: &Domain, item: Item) -> Option<u64> {
    let id = items::find(domain, item)?;
    domain.items.get(id)?.relations.pull
}

fn failed(domain: &mut Domain, env: &Env<Limits>, owner: Token) {
    answer_brief(domain, env, owner, brief::Read::Failed);
}

fn answer_brief(domain: &mut Domain, env: &Env<Limits>, owner: Token, read: brief::Read) {
    route::brief_step(domain, env, brief::Event::Read { owner, read });
}

/// A brief's read of the forge ended: the parts it asked for, cut.
fn brief_got(
    domain: &mut Domain,
    env: &Env<Limits>,
    owner: Token,
    bounds: Bounds,
    source: &brief::Source,
    result: Result<api::Answer, forge::Failure>,
) {
    let Ok(answer) = result else { return answer_brief(domain, env, owner, brief::Read::Failed) };
    let engine = domain.config.forge.engine;
    let mut found: List<Box<[u8]>> = List::with_capacity(bounds.parts.max(1));
    let fits = match source {
        brief::Source::Item(_) => {
            let api::Answer::Item { item, .. } = answer else { return failed(domain, env, owner) };
            found.push(translate::concat(&[&item.title, b"\n\n", &item.body])).is_ok()
        }
        brief::Source::Comments { item, .. } => {
            let api::Answer::Item { comments, more, .. } = answer else { return failed(domain, env, owner) };
            let carried = carry(&comments, more, engine, bounds, &mut found);
            if let Some(id) = items::find(domain, translate::from_brief(*item))
                && let Some(entry) = domain.items.get_mut(id)
                && let Some(live) = entry.live.as_mut()
                && !live.started
            {
                live.comments = carried;
            }
            true
        }
        brief::Source::Ci { .. } => {
            let api::Answer::Statuses { statuses, .. } = answer else { return failed(domain, env, owner) };
            for status in &statuses {
                if status.check != api::Check::Failed {
                    continue;
                }
                let line = translate::concat(&[&status.context, b": ", &status.description, b"\n", &status.url]);
                if found.push(line).is_err() {
                    break;
                }
            }
            true
        }
        brief::Source::Reviews { head, .. } => {
            let api::Answer::Reviews { reviews, .. } = answer else { return failed(domain, env, owner) };
            for review in &reviews {
                if review.commit == head.0 && found.push(copy_of(&review.body)).is_err() {
                    break;
                }
            }
            true
        }
        brief::Source::Pull { .. } => {
            let api::Answer::Pull(pull) = answer else { return failed(domain, env, owner) };
            let merges: &[u8] = if pull.mergeable { b"merges cleanly" } else { b"conflicts with its base" };
            found.push(translate::concat(&[&pull.head, b" into ", &pull.base, b": ", merges])).is_ok()
        }
        brief::Source::Dependencies(_)
        | brief::Source::Attempts(_)
        | brief::Source::Plan { .. }
        | brief::Source::Notes { .. }
        | brief::Source::Template(_) => false,
    };
    if !fits {
        return failed(domain, env, owner);
    }
    let slices = slices(&found);
    let got = cut(slices.as_slice(), bounds);
    answer_brief(domain, env, owner, got);
}

/// People's comments in a brief, into `found`: the oldest first, whole, as
/// many as fit the read's bytes, and at least the first, cut as the brief
/// cuts it. The last comment carried, if any did not fit or were not read:
/// a run's answer takes only what its brief carried, and the rest goes to
/// the next run, so people's messages are never cut from a brief unseen
/// (the cut keeps the end, which would drop the oldest).
fn carry(
    comments: &[api::Comment],
    more: bool,
    engine: u64,
    bounds: Bounds,
    found: &mut List<Box<[u8]>>,
) -> Option<u64> {
    let room = u64::from(bounds.bytes);
    let mut used: u64 = 0;
    let mut last: Option<u64> = None;
    for comment in comments {
        let theirs = match &comment.mark {
            api::Mark::None => comment.author != engine,
            api::Mark::Key { person, .. } => person.is_some(),
            api::Mark::Record { .. } | api::Mark::Mangled => false,
        };
        if !theirs {
            last = Some(comment.id);
            continue;
        }
        let size = len(&comment.body);
        let fits = found.is_empty() || used.saturating_add(size) <= room;
        if !fits || found.push(copy_of(&comment.body)).is_err() {
            return Some(last.unwrap_or(0));
        }
        used = used.saturating_add(size);
        last = Some(comment.id);
    }
    if more { Some(last.unwrap_or(0)) } else { None }
}

fn slices(found: &List<Box<[u8]>>) -> List<&[u8]> {
    let mut slices = List::with_capacity(found.len());
    for part in found {
        slices.push(&part[..]).expect("room for each of them");
    }
    slices
}

/// The status of the plan under `goal`: its steps, each done or not.
fn plan_status(domain: &Domain, goal: Item, bounds: Bounds) -> brief::Read {
    let Some(id) = items::find(domain, goal) else { return brief::Read::Failed };
    let Some(entry) = domain.items.get(id) else { return brief::Read::Failed };
    let mut lines: List<Box<[u8]>> = List::with_capacity(u32::try_from(entry.relations.children.len()).unwrap_or(0));
    for child in &entry.relations.children {
        let state: &[u8] = if child.done.is_some() { b": done" } else { b": open" };
        lines.push(translate::concat(&[&child.name, state])).expect("room for each of them");
    }
    let slices = slices(&lines);
    cut(slices.as_slice(), bounds)
}

/// The item's earlier attempts, and its failures by class.
fn attempts(domain: &Domain, item: Item, bounds: Bounds) -> brief::Read {
    let Some(id) = items::find(domain, item) else { return brief::Read::Failed };
    let Some(entry) = domain.items.get(id) else { return brief::Read::Failed };
    let lifecycle = entry.lifecycle;
    let failures = lifecycle.failures;
    let line = translate::concat(&[
        b"attempts: ",
        &translate::decimal(lifecycle.attempts),
        b"; failed: transient ",
        &translate::decimal(u64::from(failures.of(work::Class::Transient))),
        b", permanent ",
        &translate::decimal(u64::from(failures.of(work::Class::Permanent))),
        b", run ",
        &translate::decimal(u64::from(failures.of(work::Class::Run))),
        b", agent ",
        &translate::decimal(u64::from(failures.of(work::Class::Agent))),
        b", lost ",
        &translate::decimal(u64::from(failures.of(work::Class::Lost))),
        b", invalid ",
        &translate::decimal(u64::from(failures.of(work::Class::Invalid))),
    ]);
    cut(&[&line], bounds)
}

/// `found`, cut to the read's bounds as its fit says (see
/// [`brief::Fit`]), never splitting a UTF-8 sequence: what is left out is
/// counted in the `left` of the part next to it.
pub(crate) fn cut(found: &[&[u8]], bounds: Bounds) -> brief::Read {
    let parts = match bounds.fit {
        brief::Fit::Run => run(found, bounds),
        brief::Fit::Each => each(found, bounds),
        brief::Fit::Lines => lines(found, bounds),
    };
    brief::Read::Got(parts)
}

/// One run of bytes from the end the read keeps.
fn run(found: &[&[u8]], bounds: Bounds) -> Box<[brief::Part]> {
    let mut kept: List<brief::Part> = List::with_capacity(bounds.parts);
    let mut room = u64::from(bounds.bytes);
    let mut left: u64 = 0;
    let count = found.len();
    for index in 0..count {
        let Some(part) = found.get(nearest(index, count, bounds.keep)) else { break };
        if kept.room() == 0 || room == 0 {
            left = left.saturating_add(len(part));
            continue;
        }
        let piece = kept_of(part, room, bounds.keep);
        room = room.saturating_sub(len(piece));
        let rest = len(part).saturating_sub(len(piece));
        kept.push(brief::Part { bytes: copy_of(piece), left: rest }).expect("room for each of them");
    }
    told(&mut kept, left);
    ordered(kept, bounds.keep)
}

/// Each part to an even share of the read's bytes, those nearest the end
/// the read keeps first.
fn each(found: &[&[u8]], bounds: Bounds) -> Box<[brief::Part]> {
    let mut kept: List<brief::Part> = List::with_capacity(bounds.parts);
    let count = found.len();
    let shares = u64::try_from(count.min(usize::try_from(bounds.parts).unwrap_or(usize::MAX)).max(1)).unwrap_or(1);
    let share = u64::from(bounds.bytes).checked_div(shares).unwrap_or(0);
    let mut left: u64 = 0;
    for index in 0..count {
        let Some(part) = found.get(nearest(index, count, bounds.keep)) else { break };
        if kept.room() == 0 {
            left = left.saturating_add(len(part));
            continue;
        }
        let piece = kept_of(part, share, bounds.keep);
        let rest = len(part).saturating_sub(len(piece));
        kept.push(brief::Part { bytes: copy_of(piece), left: rest }).expect("room for each of them");
    }
    told(&mut kept, left);
    ordered(kept, bounds.keep)
}

/// Whole parts from the start, and the last part, always kept.
fn lines(found: &[&[u8]], bounds: Bounds) -> Box<[brief::Part]> {
    let mut kept: List<brief::Part> = List::with_capacity(bounds.parts.max(1));
    let Some((last, before)) = found.split_last() else { return kept.into_boxed() };
    let last = kept_of(last, u64::from(bounds.bytes), brief::Keep::Start);
    let mut room = u64::from(bounds.bytes).saturating_sub(len(last));
    let mut left: u64 = 0;
    for part in before {
        let fits = len(part) <= room && kept.len().saturating_add(1) < bounds.parts && left == 0;
        if !fits {
            left = left.saturating_add(len(part));
            continue;
        }
        room = room.saturating_sub(len(part));
        kept.push(brief::Part { bytes: copy_of(part), left: 0 }).expect("room for each of them");
    }
    kept.push(brief::Part { bytes: copy_of(last), left }).expect("room for the last part");
    kept.into_boxed()
}

/// The `index`th part from the end the read keeps.
const fn nearest(index: usize, count: usize, keep: brief::Keep) -> usize {
    match keep {
        brief::Keep::Start => index,
        brief::Keep::End => count.saturating_sub(index).saturating_sub(1),
    }
}

/// As much of `part` as `room` holds, from the end `keep` names, never
/// splitting a UTF-8 sequence.
fn kept_of(part: &[u8], room: u64, keep: brief::Keep) -> &[u8] {
    let mut take = usize::try_from(room).unwrap_or(usize::MAX).min(part.len());
    match keep {
        brief::Keep::Start => {
            for _ in 0..4 {
                let continues = match part.get(take) {
                    Some(byte) => *byte & 0xC0 == 0x80,
                    None => false,
                };
                if !continues {
                    break;
                }
                take = take.saturating_sub(1);
            }
            part.get(..take).unwrap_or(&[])
        }
        brief::Keep::End => {
            let mut start = part.len().saturating_sub(take);
            for _ in 0..4 {
                let continues = match part.get(start) {
                    Some(byte) => *byte & 0xC0 == 0x80,
                    None => false,
                };
                if !continues {
                    break;
                }
                start = start.saturating_add(1);
            }
            part.get(start..).unwrap_or(&[])
        }
    }
}

fn len(bytes: &[u8]) -> u64 {
    u64::try_from(bytes.len()).unwrap_or(u64::MAX)
}

/// Counts the whole parts left out in the last part kept, which is next to
/// them.
fn told(kept: &mut List<brief::Part>, left: u64) {
    if left > 0
        && let Some(last) = kept.get_mut(kept.len().saturating_sub(1))
    {
        last.left = last.left.saturating_add(left);
    }
}

/// The parts kept, in their source's order.
fn ordered(kept: List<brief::Part>, keep: brief::Keep) -> Box<[brief::Part]> {
    let mut parts = kept.into_boxed();
    if keep == brief::Keep::End {
        parts.reverse();
    }
    parts
}

/// The notes answered an index, for a brief, or a search.
pub(crate) fn indexed(domain: &mut Domain, env: &Env<Limits>, reply_to: ReplyTo, lines: Box<[notes::Line]>, more: u32) {
    let wait = take(domain, reply_to.into_token());
    let Some((owner, bounds)) = wait.brief() else { return };
    let count = u32::try_from(lines.len()).unwrap_or(u32::MAX).saturating_add(1);
    let mut found: List<Box<[u8]>> = List::with_capacity(count);
    for line in &lines {
        found.push(translate::concat(&[&line.name, b": ", &line.description])).expect("room for each of them");
    }
    // The last part says how many entries did not fit: empty if none.
    let rest =
        if more > 0 { translate::concat(&[&translate::decimal(u64::from(more)), b" more"]) } else { Box::new([]) };
    found.push(rest).expect("room for the count of the rest");
    let slices = slices(&found);
    let got = cut(slices.as_slice(), bounds);
    answer_brief(domain, env, owner, got);
}

/// The notes answered a run's call.
pub(crate) fn relay_served(domain: &mut Domain, env: &Env<Limits>, reply_to: ReplyTo, served: Served) {
    let taken = take(domain, reply_to.into_token());
    let Some(to) = taken.relay() else { return };
    runs::serve_answer(domain, env, to, served);
}

/// The notes refused a call at their entrance.
pub(crate) fn notes_refused(domain: &mut Domain, env: &Env<Limits>, reply_to: ReplyTo, refusal: notes::Refusal) {
    match take(domain, reply_to.into_token()) {
        Wait::Brief { owner, .. } => answer_brief(domain, env, owner, brief::Read::Failed),
        Wait::Relay { to, .. } => {
            let why = match refusal {
                notes::Refusal::Busy => Unserved::Busy,
                notes::Refusal::Oversized => Unserved::Invalid,
            };
            runs::serve_answer(domain, env, to, Served::Unserved(why));
        }
        Wait::Job { .. }
        | Wait::Take { .. }
        | Wait::Record { .. }
        | Wait::Aside { .. }
        | Wait::Release { .. }
        | Wait::Wiki { .. }
        | Wait::Person { .. }
        | Wait::Views { .. }
        | Wait::Done => unreachable!("a notes refusal ends a brief or a relayed call"),
    }
}

/// A run's call, which its worker names `named`, served.
pub(crate) fn serve(
    domain: &mut Domain,
    env: &Env<Limits>,
    wait: Id<Wait>,
    item: Item,
    attempt: u64,
    named: Token,
    call: Call,
) {
    let Some(grants) = assigned(domain, item) else {
        return runs::unserved(domain, env, wait, Unserved::Ungranted);
    };
    let owner = wait.token();
    match call {
        Call::Read(read) => {
            if !grants.forge {
                return runs::unserved(domain, env, wait, Unserved::Ungranted);
            }
            route::forge_step(domain, env, forge::Event::Read { owner, read });
        }
        Call::Recall(recall) => {
            route::notes_step(domain, env, notes::Event::Recall { reply_to: ReplyTo::new(owner), recall });
        }
        Call::Note { scope, name, change } => {
            if !grants.note || !within_scope(domain, item, scope) {
                return runs::unserved(domain, env, wait, Unserved::Ungranted);
            }
            let checked =
                rules::Write::Note { repository: translate::repository(item.repository), scope: rules_scope(scope) };
            let mut findings = Queue::with_capacity(rules::max_out(&env.limits.rules));
            let decision =
                rules::check_write(&domain.config.rules, &env.limits.rules, &checked, None, &[], &mut findings);
            // A note the rules want a person to accept is refused too: a
            // call that waited on a person could outlive its run.
            if decision != rules::Decision::Allow {
                return runs::unserved(domain, env, wait, Unserved::Refused);
            }
            let reply_to = ReplyTo::new(owner);
            route::notes_step(domain, env, notes::Event::Note { reply_to, scope, name, change });
        }
        Call::Comment { text } => comment(domain, env, owner, item, attempt, named, text),
        Call::Escalate { text } => {
            if let Some(goal) = goal_of(domain, item) {
                items::notice(domain, env, goal, Inbound::Held { item }, plan::Source::Child);
            }
            comment(domain, env, owner, item, attempt, named, text);
        }
    }
}

/// The entry of the goal the item's step is under.
fn goal_of(domain: &Domain, item: Item) -> Option<Id<items::Entry>> {
    let entry = domain.items.get(items::find(domain, item)?)?;
    items::find(domain, entry.relations.goal?)
}

/// The grants of the item's attempt in flight, started or adopted.
fn assigned(domain: &Domain, item: Item) -> Option<plan::Grants> {
    domain.items.get(items::find(domain, item)?)?.grants
}

/// Whether a run of `item` may note in `scope`: the deployment's, its
/// repository's, or its goal's.
fn within_scope(domain: &Domain, item: Item, scope: notes::Scope) -> bool {
    match scope {
        notes::Scope::Deployment => true,
        notes::Scope::Repository(repository) => repository == item.repository,
        notes::Scope::Goal { repository, number } => {
            let goal = match items::find(domain, item) {
                Some(id) => match domain.items.get(id) {
                    Some(entry) => entry.relations.goal,
                    None => None,
                },
                None => None,
            };
            goal == Some(Item { repository, number }) || (repository == item.repository && number == item.number)
        }
    }
}

const fn rules_scope(scope: notes::Scope) -> rules::Scope {
    match scope {
        notes::Scope::Deployment => rules::Scope::Deployment,
        notes::Scope::Repository(_) => rules::Scope::Repository,
        notes::Scope::Goal { .. } => rules::Scope::Goal,
    }
}

/// A run's comment on its item, keyed by its attempt and its worker's name
/// for the call, so a call made again is found: one of an attempt adopted
/// after a restart is looked for first, from its claim's position.
fn comment(
    domain: &mut Domain,
    env: &Env<Limits>,
    owner: Token,
    item: Item,
    attempt: u64,
    named: Token,
    text: Box<[u8]>,
) {
    let key = translate::concat(&[b"call/", &translate::decimal(attempt), b"/", &translate::decimal(named.raw())]);
    let resumed = match items::find(domain, item) {
        Some(id) => match domain.items.get(id) {
            Some(entry) => items::Resumed::cause(entry.resumed, attempt),
            None => None,
        },
        None => None,
    };
    let write = forge::Write::Comment {
        item: translate::forge_item(item),
        key,
        person: None,
        body: forge::Content::Text(text),
    };
    route::forge_step(domain, env, forge::Event::Write { owner, write, resumed });
}

/// Where a scope's page `name` is: its repository's wiki, and its path.
fn path(domain: &Domain, scope: notes::Scope, name: &[u8]) -> (u32, Box<[u8]>) {
    match scope {
        notes::Scope::Deployment => (domain.config.home, translate::concat(&[DEPLOYMENT, name])),
        notes::Scope::Repository(repository) => (repository, copy_of(name)),
        notes::Scope::Goal { repository, number } => {
            (repository, translate::concat(&[GOALS, &translate::decimal(number), b"/", name]))
        }
    }
}

const DEPLOYMENT: &[u8] = b"deployment/";
const GOALS: &[u8] = b"goals/";

/// The name of the page at `path` in `scope`, if it is one of the scope's.
fn in_scope(scope: notes::Scope, path: &[u8]) -> Option<Box<[u8]>> {
    let prefix = match scope {
        notes::Scope::Deployment => copy_of(DEPLOYMENT),
        notes::Scope::Repository(_) => {
            if skein_lib::bytes::find(path, b"/").is_some() {
                return None;
            }
            return Some(copy_of(path));
        }
        notes::Scope::Goal { number, .. } => translate::concat(&[GOALS, &translate::decimal(number), b"/"]),
    };
    let rest = path.strip_prefix(&prefix[..])?;
    if rest.is_empty() || skein_lib::bytes::find(rest, b"/").is_some() {
        return None;
    }
    Some(copy_of(rest))
}

/// The notes list a scope's pages.
pub(crate) fn list(domain: &mut Domain, env: &Env<Limits>, owner: Token, scope: notes::Scope) {
    let found = List::with_capacity(env.limits.notes.entries);
    list_from(domain, env, owner, scope, found, None);
}

fn list_from(
    domain: &mut Domain,
    env: &Env<Limits>,
    owner: Token,
    scope: notes::Scope,
    found: List<notes::Listed>,
    after: Option<Box<[u8]>>,
) {
    let (repository, _) = path(domain, scope, b"");
    let Ok(wait) = domain.waits.insert(Wait::Wiki { owner, op: Wiki::List { scope, found } }) else {
        return route::notes_step(domain, env, notes::Event::Listed { owner, pages: None });
    };
    let read = forge::Read::Pages { repository, after };
    route::forge_step(domain, env, forge::Event::Read { owner: wait.token(), read });
}

/// The notes read a scope's page.
pub(crate) fn fetch(domain: &mut Domain, env: &Env<Limits>, owner: Token, scope: notes::Scope, name: &[u8]) {
    let (repository, name) = path(domain, scope, name);
    let Ok(wait) = domain.waits.insert(Wait::Wiki { owner, op: Wiki::Fetch }) else {
        let fetched = notes::Fetched::Failed;
        return route::notes_step(domain, env, notes::Event::Fetched { owner, fetched });
    };
    route::forge_step(
        domain,
        env,
        forge::Event::Read { owner: wait.token(), read: forge::Read::Page { repository, name } },
    );
}

/// The notes make a page, only if it is not there.
pub(crate) fn create(
    domain: &mut Domain,
    env: &Env<Limits>,
    owner: Token,
    scope: notes::Scope,
    name: &[u8],
    page: notes::Page,
) {
    let (repository, name) = path(domain, scope, name);
    let Ok(wait) = domain.waits.insert(Wait::Wiki { owner, op: Wiki::Create { page: Box::new(page) } }) else {
        return route::notes_step(domain, env, notes::Event::Wrote { owner, wrote: notes::Wrote::Failed });
    };
    let content = forge::Content::Payload(wait.token());
    let write = forge::Write::PutPage { repository, name, content, revision: None };
    route::forge_step(domain, env, forge::Event::Write { owner: wait.token(), write, resumed: None });
}

/// The notes edit a page, only if it is still at `revision`, the one their
/// check read as the run recalled it.
pub(crate) fn edit(
    domain: &mut Domain,
    env: &Env<Limits>,
    owner: Token,
    scope: notes::Scope,
    name: &[u8],
    page: notes::Page,
    revision: u64,
) {
    let (repository, name) = path(domain, scope, name);
    let Ok(wait) = domain.waits.insert(Wait::Wiki { owner, op: Wiki::Edit { page: Box::new(page) } }) else {
        return route::notes_step(domain, env, notes::Event::Wrote { owner, wrote: notes::Wrote::Failed });
    };
    let content = forge::Content::Payload(wait.token());
    let write = forge::Write::PutPage { repository, name, content, revision: Some(revision) };
    route::forge_step(domain, env, forge::Event::Write { owner: wait.token(), write, resumed: None });
}

/// The notes delete a page.
pub(crate) fn delete(domain: &mut Domain, env: &Env<Limits>, owner: Token, scope: notes::Scope, name: &[u8]) {
    let (repository, name) = path(domain, scope, name);
    let Ok(wait) = domain.waits.insert(Wait::Wiki { owner, op: Wiki::Delete }) else {
        return route::notes_step(domain, env, notes::Event::Wrote { owner, wrote: notes::Wrote::Failed });
    };
    let write = forge::Write::DeletePage { repository, name };
    route::forge_step(domain, env, forge::Event::Write { owner: wait.token(), write, resumed: None });
}

/// A wiki read for the notes ended.
fn wiki_read(
    domain: &mut Domain,
    env: &Env<Limits>,
    owner: Token,
    op: Wiki,
    result: Result<api::Answer, forge::Failure>,
) {
    match op {
        Wiki::List { scope, mut found } => {
            let Ok(api::Answer::Pages { pages, next }) = result else {
                return route::notes_step(domain, env, notes::Event::Listed { owner, pages: None });
            };
            for page in &pages {
                let Some(name) = in_scope(scope, &page.name) else { continue };
                if found.push(notes::Listed { name, revision: page.revision }).is_err() {
                    break;
                }
            }
            match next {
                Some(after) if found.room() > 0 => list_from(domain, env, owner, scope, found, Some(after)),
                Some(_) | None => {
                    let pages = Some(found.into_boxed());
                    route::notes_step(domain, env, notes::Event::Listed { owner, pages });
                }
            }
        }
        Wiki::Fetch => {
            let fetched = match result {
                Ok(api::Answer::Page(page)) => match decoded_page(domain, &page.name) {
                    Some(decoded) => notes::Fetched::Page { revision: page.revision, page: decoded },
                    None => notes::Fetched::Failed,
                },
                Err(forge::Failure::Forge(api::Error::Missing)) => notes::Fetched::Gone,
                Ok(_) | Err(_) => notes::Fetched::Failed,
            };
            route::notes_step(domain, env, notes::Event::Fetched { owner, fetched });
        }
        Wiki::Edit { .. } | Wiki::Create { .. } | Wiki::Delete => {}
    }
}

/// The note's page the protocol layer decoded from the wiki page `name`.
fn decoded_page(domain: &Domain, name: &[u8]) -> Option<notes::Page> {
    for decoded in &domain.decoded {
        match decoded {
            Decoded::Page { name: found, page } if **found == *name => return Some(notes::Page::clone(page)),
            Decoded::Page { .. } | Decoded::Record { .. } | Decoded::Outcome { .. } => {}
        }
    }
    None
}

/// A wiki write for the notes ended.
fn wiki_wrote(
    domain: &mut Domain,
    env: &Env<Limits>,
    owner: Token,
    op: Wiki,
    result: Result<forge::Written, forge::Failure>,
) {
    let wrote = match op {
        Wiki::Create { .. } => match result {
            Ok(forge::Written::Revision(revision)) => notes::Wrote::Done { revision },
            Err(forge::Failure::Revised { .. }) => notes::Wrote::Exists,
            Ok(_) | Err(_) => notes::Wrote::Failed,
        },
        Wiki::Edit { .. } => match result {
            Ok(forge::Written::Revision(revision)) => notes::Wrote::Done { revision },
            Err(forge::Failure::Revised { revision: None }) => notes::Wrote::Missing,
            Ok(_) | Err(_) => notes::Wrote::Failed,
        },
        Wiki::Delete => match result {
            Ok(_) => notes::Wrote::Done { revision: 0 },
            Err(forge::Failure::Forge(api::Error::Missing)) => notes::Wrote::Missing,
            Err(_) => notes::Wrote::Failed,
        },
        Wiki::List { .. } | Wiki::Fetch => return,
    };
    route::notes_step(domain, env, notes::Event::Wrote { owner, wrote });
}
