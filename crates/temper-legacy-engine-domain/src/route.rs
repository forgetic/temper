//! Routing (programming-model.md, 4.5): each of the protocol's events to the
//! child domain, or the part of the top level, it is for, and each child domain's
//! requests to the protocol layer or, translated, to a sibling.
//!
//! The hub is the hub, and its capabilities answer it: a hand-off goes from
//! a capability to the hub or to the top level's state, from the hub to a
//! capability or, through the plan and the rules, which are pure, straight
//! back to the hub. Every hand-off ends in a request to the protocol layer,
//! or in a state that waits for one's answer; an entry point completes them
//! all before it returns, each child domain taking at most `limits.steps` steps
//! in it, which sizes the queues that hold their requests until they are
//! routed.
//!
//! Every match is exhaustive, so a variant added to either side's vocabulary
//! breaks the build here.

use skein_lib::{Env, Queue};
use temper_engine_domain_brief as brief;
use temper_engine_domain_fleet as fleet;
use temper_engine_domain_notes as notes;
use temper_engine_domain_views as views;
use temper_legacy_engine_domain_forge as forge;
use temper_legacy_engine_domain_plan as plan;
use temper_legacy_engine_domain_work as work;

use crate::boundary::{Event, Request};
use crate::domain::Domain;
use crate::items;
use crate::jobs;
use crate::limits::{self, Limits};
use crate::people;
use crate::runs;
use crate::serve;
use crate::translate;

pub(crate) const fn work_env(env: &Env<Limits>) -> Env<work::Limits> {
    Env { now: env.now, wall: env.wall, limits: env.limits.work }
}

pub(crate) const fn plan_env(env: &Env<Limits>) -> Env<plan::Limits> {
    Env { now: env.now, wall: env.wall, limits: env.limits.plan }
}

pub(crate) const fn forge_env(env: &Env<Limits>) -> Env<forge::Limits> {
    Env { now: env.now, wall: env.wall, limits: env.limits.forge }
}

pub(crate) const fn fleet_env(env: &Env<Limits>) -> Env<fleet::Limits> {
    Env { now: env.now, wall: env.wall, limits: env.limits.fleet }
}

pub(crate) const fn brief_env(env: &Env<Limits>) -> Env<brief::Limits> {
    Env { now: env.now, wall: env.wall, limits: env.limits.brief }
}

pub(crate) const fn notes_env(env: &Env<Limits>) -> Env<notes::Limits> {
    Env { now: env.now, wall: env.wall, limits: env.limits.notes }
}

pub(crate) const fn views_env(env: &Env<Limits>) -> Env<views::Limits> {
    Env { now: env.now, wall: env.wall, limits: env.limits.views }
}

/// Hands one of the protocol's events to the child domain, or the part of the
/// top level, it is for.
pub(crate) fn event(domain: &mut Domain, env: &Env<Limits>, event: Event, out: &mut Queue<Request>) {
    match event {
        Event::Refreshed { account, generation, valid } => {
            crate::credentials::step(domain, env, crate::accounts::Event::Refreshed { account, generation, valid });
        }
        Event::RefreshFailed { account, generation, failure } => {
            crate::credentials::step(domain, env, crate::accounts::Event::Failed { account, generation, failure });
        }
        Event::Rejected { channel, item, attempt, account, generation } => fleet_step(
            domain,
            env,
            fleet::Event::Rejected {
                channel,
                run: translate::run_of(item),
                attempt: skein_lib::Token::new(attempt),
                account,
                generation,
            },
        ),
        Event::Exhausted { channel, item, attempt, account, retry_after } => fleet_step(
            domain,
            env,
            fleet::Event::Exhausted {
                channel,
                run: translate::run_of(item),
                attempt: skein_lib::Token::new(attempt),
                account,
                retry_after,
            },
        ),
        Event::Answered { call, cost, result, decoded } => {
            domain.decoded = decoded;
            forge_step(domain, env, forge::Event::Answered { call, cost, result });
        }
        Event::Hint { repository, item, commit, branch, by, wiki } => {
            forge_step(domain, env, forge::Event::Hint { repository, item, commit, branch, by, wiki });
        }
        Event::Hello { channel, hello } => {
            runs::hello(domain, &hello);
            fleet_step(domain, env, fleet::Event::Hello { channel, hello: translate::hello(hello) });
        }
        Event::Lost { channel } => fleet_step(domain, env, fleet::Event::Lost { channel }),
        Event::Answer { channel, item, attempt, answer } => runs::answer(domain, env, channel, item, attempt, answer),
        Event::Relay { channel, item, attempt, call, body } => {
            runs::relay(domain, env, channel, item, attempt, call, body);
        }
        Event::Bounced { channel, item, attempt, name, bounce } => {
            runs::bounced(domain, env, channel, item, attempt, name, bounce);
        }
        Event::Told { channel, item, attempt, kind, content } => {
            runs::told(domain, env, channel, item, attempt, kind, content);
        }
        Event::Ask { reply_to, person, ask } => people::ask(domain, env, reply_to, person, ask, out),
        Event::Unwatch { watcher } => views_step(domain, env, views::Event::Unwatch { watcher }),
        Event::Delivered { watcher, done } => views_step(domain, env, views::Event::Delivered { watcher, done }),
        Event::Stored { owner, stored } => serve::stored(domain, env, owner, stored),
    }
}

/// Routes what the child domains emitted, and what that leads to, until all of
/// them have emitted all they will in this entry point.
pub(crate) fn hand_off(domain: &mut Domain, env: &Env<Limits>, out: &mut Queue<Request>) {
    // Every request routed, and once the cold start is done, its end.
    let bound = limits::routed(&env.limits).saturating_add(1);
    for _ in 0..bound {
        if let Some(request) = domain.account_out.pop() {
            crate::credentials::route(domain, env, request, out);
        } else if let Some(request) = domain.views_out.pop() {
            from_views(domain, env, request, out);
        } else if let Some(request) = domain.notes_out.pop() {
            from_notes(domain, env, request);
        } else if let Some(request) = domain.brief_out.pop() {
            from_brief(domain, env, request);
        } else if let Some(request) = domain.fleet_out.pop() {
            from_fleet(domain, env, request, out);
        } else if let Some(request) = domain.forge_out.pop() {
            from_forge(domain, env, request, out);
        } else if let Some(request) = domain.work_out.pop() {
            from_work(domain, env, request);
        } else if runs::is_loaded(domain) {
            runs::loaded(domain, env);
        } else {
            return;
        }
    }
    assert!(
        domain.account_out.is_empty()
            && domain.views_out.is_empty()
            && domain.notes_out.is_empty()
            && domain.brief_out.is_empty()
            && domain.fleet_out.is_empty()
            && domain.forge_out.is_empty()
            && domain.work_out.is_empty(),
        "an entry point's hand-offs end within its bound"
    );
}

pub(crate) fn work_step(domain: &mut Domain, env: &Env<Limits>, event: work::Event) {
    count(&mut domain.steps.work, &env.limits);
    assert!(domain.work_out.room() >= work::max_out(&env.limits.work), "an entry point steps the hub within its bound");
    work::step(&mut domain.work, &work_env(env), event, &mut domain.work_out);
}

pub(crate) fn work_fire(domain: &mut Domain, env: &Env<Limits>) {
    count(&mut domain.steps.work, &env.limits);
    work::fire(&mut domain.work, &work_env(env), &mut domain.work_out);
}

pub(crate) fn forge_step(domain: &mut Domain, env: &Env<Limits>, event: forge::Event) {
    count(&mut domain.steps.forge, &env.limits);
    let room = forge::max_out(&env.limits.forge);
    assert!(domain.forge_out.room() >= room, "an entry point steps the forge child domain within its bound");
    forge::step(&mut domain.forge, &forge_env(env), event, &mut domain.forge_out);
}

pub(crate) fn forge_fire(domain: &mut Domain, env: &Env<Limits>) {
    count(&mut domain.steps.forge, &env.limits);
    forge::fire(&mut domain.forge, &forge_env(env), &mut domain.forge_out);
}

pub(crate) fn forge_resume(domain: &mut Domain, env: &Env<Limits>) {
    count(&mut domain.steps.forge, &env.limits);
    forge::resume(&mut domain.forge, &forge_env(env), &mut domain.forge_out);
}

pub(crate) fn fleet_step(domain: &mut Domain, env: &Env<Limits>, event: fleet::Event) {
    count(&mut domain.steps.fleet, &env.limits);
    let room = fleet::max_out(&env.limits.fleet);
    assert!(domain.fleet_out.room() >= room, "an entry point steps the fleet within its bound");
    fleet::step(&mut domain.fleet, &fleet_env(env), event, &mut domain.fleet_out);
}

pub(crate) fn fleet_fire(domain: &mut Domain, env: &Env<Limits>) {
    count(&mut domain.steps.fleet, &env.limits);
    fleet::fire(&mut domain.fleet, &fleet_env(env), &mut domain.fleet_out);
}

pub(crate) fn fleet_resume(domain: &mut Domain, env: &Env<Limits>) {
    count(&mut domain.steps.fleet, &env.limits);
    fleet::resume(&mut domain.fleet, &fleet_env(env), &mut domain.fleet_out);
}

pub(crate) fn brief_step(domain: &mut Domain, env: &Env<Limits>, event: brief::Event) {
    count(&mut domain.steps.brief, &env.limits);
    let room = brief::max_out(&env.limits.brief);
    assert!(domain.brief_out.room() >= room, "an entry point steps the brief within its bound");
    brief::step(&mut domain.brief, &brief_env(env), event, &mut domain.brief_out);
}

pub(crate) fn brief_fire(domain: &mut Domain, env: &Env<Limits>) {
    count(&mut domain.steps.brief, &env.limits);
    brief::fire(&mut domain.brief, &brief_env(env), &mut domain.brief_out);
}

pub(crate) fn notes_step(domain: &mut Domain, env: &Env<Limits>, event: notes::Event) {
    count(&mut domain.steps.notes, &env.limits);
    assert!(domain.notes_out.room() >= notes::MAX_OUT, "an entry point steps the notes within their bound");
    notes::step(&mut domain.notes, &notes_env(env), event, &mut domain.notes_out);
}

pub(crate) fn notes_resume(domain: &mut Domain, env: &Env<Limits>) {
    count(&mut domain.steps.notes, &env.limits);
    notes::resume(&mut domain.notes, &notes_env(env), &mut domain.notes_out);
}

/// Coalesces a wiki hint over the scopes the notes already hold. The
/// deployment's scope belongs only to its configured home repository.
fn wiki_changed(domain: &mut Domain, env: &Env<Limits>, repository: u32) {
    for index in 0..env.limits.notes.scopes {
        let Some(scope) = domain.notes.scope(index) else { break };
        let matches = match scope {
            notes::Scope::Deployment => repository == domain.config.home,
            notes::Scope::Repository(held) | notes::Scope::Goal { repository: held, number: _ } => held == repository,
        };
        if matches && domain.wiki_pending.insert(scope).is_err() {
            // A hint is advisory. Old queued scopes may still occupy the
            // bounded set after an eviction; periodic reads catch up.
            break;
        }
    }
}

pub(crate) fn wiki_resume(domain: &mut Domain, env: &Env<Limits>) {
    let Some(scope) = domain.wiki_pending.pop_first() else { return };
    if domain.notes.holds(scope) {
        notes_step(domain, env, notes::Event::Refresh { scope });
    }
}

pub(crate) fn views_step(domain: &mut Domain, env: &Env<Limits>, event: views::Event) {
    count(&mut domain.steps.views, &env.limits);
    let room = views::max_out(&env.limits.views);
    assert!(domain.views_out.room() >= room, "an entry point steps the views within their bound");
    views::step(&mut domain.views, &views_env(env), event, &mut domain.views_out);
}

pub(crate) fn views_fire(domain: &mut Domain, env: &Env<Limits>) {
    count(&mut domain.steps.views, &env.limits);
    views::fire(&mut domain.views, &views_env(env), &mut domain.views_out);
}

/// Counts a step of a child domain in the entry point.
fn count(steps: &mut u32, limits: &Limits) {
    *steps = steps.saturating_add(1);
    assert!(*steps <= limits.steps, "an entry point takes no more steps of a child domain than its limits allow");
}

/// One of the hub's requests: an answer to a call the top level made, or a
/// request of the plan, the rules, the forge, the fleet or the store.
fn from_work(domain: &mut Domain, env: &Env<Limits>, request: work::Request) {
    match request {
        work::Request::Taken { to } | work::Request::Stopped { to } | work::Request::Released { to } => {
            people::hub_answered(domain, env, to, Ok(()));
        }
        work::Request::Refused { to, refusal } => people::hub_answered(domain, env, to, Err(refusal)),
        work::Request::Due { owner, item } => jobs::due(domain, env, owner, item),
        work::Request::Write { owner, item, lifecycle } => jobs::write(domain, env, owner, item, lifecycle),
        work::Request::Record { owner, item, attempt, outcome } => {
            jobs::record(domain, env, owner, item, attempt, outcome);
        }
        work::Request::Apply { owner, item, attempt, outcome } => {
            jobs::apply(domain, env, owner, item, attempt, outcome);
        }
        work::Request::Act { owner, item, action } => jobs::act(domain, env, owner, item, action),
        work::Request::Start { item, attempt, run } => runs::start(domain, env, item, attempt, run),
        work::Request::Adopt { item, attempt } => runs::adopt(domain, env, item, attempt),
        work::Request::Cancel { item, attempt } => runs::cancel(domain, env, item, attempt),
        work::Request::Relay { item, attempt, event } => runs::inbound(domain, env, item, attempt, event),
        work::Request::Keep { item, attempt: _, snapshot } => runs::keep(domain, item, snapshot),
        work::Request::Acknowledge { item, attempt } | work::Request::Stale { item, attempt } => {
            runs::acknowledge(domain, env, item, attempt);
        }
        work::Request::Left { item } => runs::left(domain, env, item),
    }
}

/// One of the forge child domain's requests: a call to the forge, an answer to
/// what the top level asked, or news of the working set.
fn from_forge(domain: &mut Domain, env: &Env<Limits>, request: forge::Request, out: &mut Queue<Request>) {
    match request {
        forge::Request::Call { call, repository, op } => serve::call(domain, call, repository, op, out),
        forge::Request::Read { owner, result } => serve::read(domain, env, owner, result),
        forge::Request::Wrote { owner, result } => serve::wrote(domain, env, owner, result),
        forge::Request::Full { item } => people::full(domain, item),
        forge::Request::Room => people::room(domain, env),
        forge::Request::Announced { item, view } => items::announced(domain, env, item, view),
        forge::Request::Offered { item } => people::offered(domain, env, item),
        forge::Request::Inbox { item, seq, news } => runs::news(domain, env, item, seq, news),
        // The labels a plan declares as inputs are not read yet: nothing
        // the engine decides follows labels.
        forge::Request::Changed { item: _, labels: _ } | forge::Request::Forbidden { item: _ } => {}
        forge::Request::Left { item, why: _ } => items::left(domain, env, item),
        forge::Request::Loaded => runs::read(domain),
        forge::Request::Wiki { repository } => wiki_changed(domain, env, repository),
    }
}

/// One of the fleet's requests: to a worker, through the protocol layer; or
/// to the hub, translated.
fn from_fleet(domain: &mut Domain, env: &Env<Limits>, request: fleet::Request, out: &mut Queue<Request>) {
    match request {
        fleet::Request::Grant { channel, run, attempt, grant } => out.push(Request::Grant {
            channel,
            item: translate::item(run),
            attempt: attempt.raw(),
            grant: crate::accounts::Grant { account: grant.account, generation: grant.generation, valid: grant.valid },
        }),
        fleet::Request::Rejected { run, attempt, account, generation } => {
            if crate::credentials::uses(domain, translate::item(run), attempt.raw(), account) {
                crate::credentials::step(domain, env, crate::accounts::Event::Rejected { account, generation });
            }
        }
        fleet::Request::Exhausted { run, attempt, account, retry_after } => {
            if crate::credentials::uses(domain, translate::item(run), attempt.raw(), account) {
                crate::credentials::step(domain, env, crate::accounts::Event::Exhausted { account, retry_after });
            }
        }
        fleet::Request::Assign { channel, run, attempt } => runs::assign(domain, channel, run, attempt, out),
        fleet::Request::Inbound { channel, run, attempt, event } => {
            runs::deliver(domain, channel, run, attempt, event, out);
        }
        fleet::Request::Cancel { channel, run, attempt } => {
            let item = translate::item(run);
            out.push(Request::Cancel { channel, item, attempt: attempt.raw() });
        }
        fleet::Request::Relayed { channel, run, attempt, call, answer } => {
            runs::relayed(domain, channel, run, attempt, call, answer, out);
        }
        fleet::Request::Acknowledge { channel, run, attempt } => {
            let item = translate::item(run);
            out.push(Request::Acknowledge { channel, item, attempt: attempt.raw() });
        }
        fleet::Request::Turned { body, .. } => runs::dropped(domain, env, body),
        fleet::Request::AcknowledgeTurn { .. } | fleet::Request::TurnBusy { .. } => {}
        fleet::Request::Refuse { channel } => out.push(Request::Refuse { channel }),
        fleet::Request::Placed { run, attempt } => runs::placed(domain, env, run, attempt),
        fleet::Request::Listed { run, attempt } => {
            let item = translate::item(run);
            work_step(domain, env, work::Event::Listed { item, attempt: attempt.raw() });
        }
        fleet::Request::Answered { to, run, attempt, answer: _, payload } => {
            runs::answered(domain, env, to, run, attempt, payload);
        }
        fleet::Request::Lost { to, run, attempt } => {
            runs::ended(domain, env, to, run, attempt, work::Answer::Lost);
        }
        fleet::Request::Withdrawn { to, run, attempt, withdrawal } => {
            assert!(
                withdrawal == fleet::Withdrawal::Cancelled,
                "an item has one live run: no attempt replaces another the fleet holds"
            );
            runs::ended(domain, env, to, run, attempt, work::Answer::Refused);
        }
        fleet::Request::Refused { to, run, attempt, refusal: _ } => {
            runs::ended(domain, env, to, run, attempt, work::Answer::Refused);
        }
        fleet::Request::Relay { reply_to, run, attempt, body } => runs::call(domain, env, reply_to, run, attempt, body),
        fleet::Request::Bounced { run, attempt, name, bounce: _ } => runs::bounce(domain, run, attempt, name),
        fleet::Request::Undelivered { run, attempt, event, undelivered } => match undelivered {
            fleet::Undelivered::Unplaced | fleet::Undelivered::Adrift => {
                let item = translate::item(run);
                work_step(domain, env, work::Event::Undelivered { item, attempt: attempt.raw(), event });
            }
            // It stays in the item's inbox, for the next run.
            fleet::Undelivered::Gone => {}
        },
        fleet::Request::Told { run, attempt: _, fact } => runs::report(domain, env, run, fact),
        fleet::Request::Drop { payload } => runs::dropped(domain, env, payload),
    }
}

/// One of the brief's requests: its answer to a render, or a read of a
/// section's source.
fn from_brief(domain: &mut Domain, env: &Env<Limits>, request: brief::Request) {
    match request {
        brief::Request::Rendered { reply_to, sections } => runs::rendered(domain, env, reply_to, Ok(sections)),
        // A brief without a section its run is for: the run failed for a
        // while, and is tried again within its class's retries.
        brief::Request::Failed { reply_to, missing: _, why: _ } => {
            let failed = work::Answer::Failed(work::Class::Transient);
            runs::rendered(domain, env, reply_to, Err(failed));
        }
        brief::Request::Refused { reply_to, refusal } => {
            let answer = match refusal {
                // No room for the brief: nothing ran, and the hub claims again
                // after a backoff.
                brief::Refusal::Busy => work::Answer::Refused,
                brief::Refusal::Oversized => work::Answer::Failed(work::Class::Permanent),
            };
            runs::rendered(domain, env, reply_to, Err(answer));
        }
        // The hub claims again after its own backoff: room in the brief needs
        // no notice.
        brief::Request::Room => {}
        brief::Request::Read { owner, source, keep, fit, parts, bytes } => {
            serve::brief_read(domain, env, owner, source, crate::waits::Bounds { keep, fit, parts, bytes });
        }
    }
}

/// One of the notes' requests: an answer to a call, or a wiki operation.
fn from_notes(domain: &mut Domain, env: &Env<Limits>, request: notes::Request) {
    match request {
        notes::Request::Indexed { reply_to, lines, more, unread: _ }
        | notes::Request::Found { reply_to, lines, more, unread: _ } => {
            serve::indexed(domain, env, reply_to, lines, more);
        }
        notes::Request::Recalled { reply_to, entries, failed } => {
            serve::relay_served(domain, env, reply_to, crate::boundary::Served::Recalled { entries, failed });
        }
        notes::Request::Noted { reply_to, noted } => {
            serve::relay_served(domain, env, reply_to, crate::boundary::Served::Noted(noted));
        }
        notes::Request::Refused { reply_to, refusal } => serve::notes_refused(domain, env, reply_to, refusal),
        notes::Request::List { owner, scope } => serve::list(domain, env, owner, scope),
        notes::Request::Fetch { owner, scope, name } => serve::fetch(domain, env, owner, scope, &name),
        notes::Request::Create { owner, scope, name, page } => serve::create(domain, env, owner, scope, &name, page),
        notes::Request::Edit { owner, scope, name, page, revision } => {
            serve::edit(domain, env, owner, scope, &name, page, revision);
        }
        notes::Request::Delete { owner, scope, name } => serve::delete(domain, env, owner, scope, &name),
    }
}

/// One of the views' requests: to a person's stream, to the store, or the
/// answer to a watch.
fn from_views(domain: &mut Domain, env: &Env<Limits>, request: views::Request, out: &mut Queue<Request>) {
    match request {
        views::Request::Watching { watcher } => people::watching(domain, watcher, None, out),
        views::Request::Refused { watcher, refusal } => people::watching(domain, watcher, Some(refusal), out),
        views::Request::Deliver { watcher, missed, chunks } => {
            out.push(Request::Deliver { watcher, missed, chunks: translate::chunks(chunks) });
        }
        views::Request::Ended { watcher, end } => out.push(Request::Ended { watcher, end }),
        views::Request::Append { owner, records } => serve::append(domain, env, owner, records, out),
        views::Request::Expire { owner, before } => serve::expire(domain, owner, before, out),
    }
}
