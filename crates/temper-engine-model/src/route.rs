//! Routing (programming-style.md, 4.5): each of the protocol's events to the
//! sub-model, or the part of the top level, it is for, and each sub-model's
//! requests to the protocol layer or, translated, to a sibling.
//!
//! The hub is the hub, and its capabilities answer it: a hand-off goes from
//! a capability to the hub or to the top level's state, from the hub to a
//! capability or, through the plan and the rules, which are pure, straight
//! back to the hub. Every hand-off ends in a request to the protocol layer,
//! or in a state that waits for one's answer; an entry point completes them
//! all before it returns, each sub-model taking at most `limits.steps` steps
//! in it, which sizes the queues that hold their requests until they are
//! routed.
//!
//! Every match is exhaustive, so a variant added to either side's vocabulary
//! breaks the build here.

use temper_engine_model_brief as brief;
use temper_engine_model_fleet as fleet;
use temper_engine_model_forge as forge;
use temper_engine_model_notes as notes;
use temper_engine_model_plan as plan;
use temper_engine_model_views as views;
use temper_engine_model_work as work;
use temper_lib::{Env, Queue};

use crate::boundary::{Event, Request};
use crate::items;
use crate::jobs;
use crate::limits::{self, Limits};
use crate::model::Model;
use crate::people;
use crate::runs;
use crate::serve;
use crate::translate;

pub(crate) const fn work_env(env: &Env<Limits>) -> Env<work::Limits> {
    Env { now: env.now, limits: env.limits.work }
}

pub(crate) const fn plan_env(env: &Env<Limits>) -> Env<plan::Limits> {
    Env { now: env.now, limits: env.limits.plan }
}

pub(crate) const fn forge_env(env: &Env<Limits>) -> Env<forge::Limits> {
    Env { now: env.now, limits: env.limits.forge }
}

pub(crate) const fn fleet_env(env: &Env<Limits>) -> Env<fleet::Limits> {
    Env { now: env.now, limits: env.limits.fleet }
}

pub(crate) const fn brief_env(env: &Env<Limits>) -> Env<brief::Limits> {
    Env { now: env.now, limits: env.limits.brief }
}

pub(crate) const fn notes_env(env: &Env<Limits>) -> Env<notes::Limits> {
    Env { now: env.now, limits: env.limits.notes }
}

pub(crate) const fn views_env(env: &Env<Limits>) -> Env<views::Limits> {
    Env { now: env.now, limits: env.limits.views }
}

/// Hands one of the protocol's events to the sub-model, or the part of the
/// top level, it is for.
pub(crate) fn event(model: &mut Model, env: &Env<Limits>, event: Event, out: &mut Queue<Request>) {
    match event {
        Event::Answered { call, result, decoded } => {
            model.decoded = decoded;
            forge_step(model, env, forge::Event::Answered { call, result });
        }
        Event::Hint { repository, item, commit, branch } => {
            forge_step(model, env, forge::Event::Hint { repository, item, commit, branch });
        }
        Event::Hello { channel, hello } => {
            runs::hello(model, &hello);
            fleet_step(model, env, fleet::Event::Hello { channel, hello: translate::hello(hello) });
        }
        Event::Lost { channel } => fleet_step(model, env, fleet::Event::Lost { channel }),
        Event::Answer { channel, item, attempt, answer } => runs::answer(model, env, channel, item, attempt, answer),
        Event::Relay { channel, item, attempt, call, body } => {
            runs::relay(model, env, channel, item, attempt, call, body);
        }
        Event::Bounced { item, attempt, bounce } => runs::bounced(model, env, item, attempt, bounce),
        Event::Told { item, attempt, kind, content } => runs::told(model, env, item, attempt, kind, content),
        Event::Ask { reply_to, person, ask } => people::ask(model, env, reply_to, person, ask, out),
        Event::Unwatch { watcher } => views_step(model, env, views::Event::Unwatch { watcher }),
        Event::Delivered { watcher, done } => views_step(model, env, views::Event::Delivered { watcher, done }),
        Event::Stored { owner, stored } => serve::stored(model, env, owner, stored),
    }
}

/// Routes what the sub-models emitted, and what that leads to, until all of
/// them have emitted all they will in this entry point.
pub(crate) fn hand_off(model: &mut Model, env: &Env<Limits>, out: &mut Queue<Request>) {
    // Every request routed, and once the cold start is done, its end.
    let bound = limits::routed(&env.limits).saturating_add(1);
    for _ in 0..bound {
        if let Some(request) = model.views_out.pop() {
            from_views(model, env, request, out);
        } else if let Some(request) = model.notes_out.pop() {
            from_notes(model, env, request);
        } else if let Some(request) = model.brief_out.pop() {
            from_brief(model, env, request);
        } else if let Some(request) = model.fleet_out.pop() {
            from_fleet(model, env, request, out);
        } else if let Some(request) = model.forge_out.pop() {
            from_forge(model, env, request, out);
        } else if let Some(request) = model.work_out.pop() {
            from_work(model, env, request);
        } else if runs::is_loaded(model) {
            runs::loaded(model, env);
        } else {
            return;
        }
    }
    assert!(
        model.views_out.is_empty()
            && model.notes_out.is_empty()
            && model.brief_out.is_empty()
            && model.fleet_out.is_empty()
            && model.forge_out.is_empty()
            && model.work_out.is_empty(),
        "an entry point's hand-offs end within its bound"
    );
}

pub(crate) fn work_step(model: &mut Model, env: &Env<Limits>, event: work::Event) {
    count(&mut model.steps.work, &env.limits);
    assert!(model.work_out.room() >= work::max_out(&env.limits.work), "an entry point steps the hub within its bound");
    work::step(&mut model.work, &work_env(env), event, &mut model.work_out);
}

pub(crate) fn work_fire(model: &mut Model, env: &Env<Limits>) {
    count(&mut model.steps.work, &env.limits);
    work::fire(&mut model.work, &work_env(env), &mut model.work_out);
}

pub(crate) fn forge_step(model: &mut Model, env: &Env<Limits>, event: forge::Event) {
    count(&mut model.steps.forge, &env.limits);
    let room = forge::max_out(&env.limits.forge);
    assert!(model.forge_out.room() >= room, "an entry point steps the forge sub-model within its bound");
    forge::step(&mut model.forge, &forge_env(env), event, &mut model.forge_out);
}

pub(crate) fn forge_fire(model: &mut Model, env: &Env<Limits>) {
    count(&mut model.steps.forge, &env.limits);
    forge::fire(&mut model.forge, &forge_env(env), &mut model.forge_out);
}

pub(crate) fn forge_resume(model: &mut Model, env: &Env<Limits>) {
    count(&mut model.steps.forge, &env.limits);
    forge::resume(&mut model.forge, &forge_env(env), &mut model.forge_out);
}

pub(crate) fn fleet_step(model: &mut Model, env: &Env<Limits>, event: fleet::Event) {
    count(&mut model.steps.fleet, &env.limits);
    let room = fleet::max_out(&env.limits.fleet);
    assert!(model.fleet_out.room() >= room, "an entry point steps the fleet within its bound");
    fleet::step(&mut model.fleet, &fleet_env(env), event, &mut model.fleet_out);
}

pub(crate) fn fleet_fire(model: &mut Model, env: &Env<Limits>) {
    count(&mut model.steps.fleet, &env.limits);
    fleet::fire(&mut model.fleet, &fleet_env(env), &mut model.fleet_out);
}

pub(crate) fn fleet_resume(model: &mut Model, env: &Env<Limits>) {
    count(&mut model.steps.fleet, &env.limits);
    fleet::resume(&mut model.fleet, &fleet_env(env), &mut model.fleet_out);
}

pub(crate) fn brief_step(model: &mut Model, env: &Env<Limits>, event: brief::Event) {
    count(&mut model.steps.brief, &env.limits);
    let room = brief::max_out(&env.limits.brief);
    assert!(model.brief_out.room() >= room, "an entry point steps the brief within its bound");
    brief::step(&mut model.brief, &brief_env(env), event, &mut model.brief_out);
}

pub(crate) fn brief_fire(model: &mut Model, env: &Env<Limits>) {
    count(&mut model.steps.brief, &env.limits);
    brief::fire(&mut model.brief, &brief_env(env), &mut model.brief_out);
}

pub(crate) fn notes_step(model: &mut Model, env: &Env<Limits>, event: notes::Event) {
    count(&mut model.steps.notes, &env.limits);
    assert!(model.notes_out.room() >= notes::MAX_OUT, "an entry point steps the notes within their bound");
    notes::step(&mut model.notes, &notes_env(env), event, &mut model.notes_out);
}

pub(crate) fn notes_resume(model: &mut Model, env: &Env<Limits>) {
    count(&mut model.steps.notes, &env.limits);
    notes::resume(&mut model.notes, &notes_env(env), &mut model.notes_out);
}

pub(crate) fn views_step(model: &mut Model, env: &Env<Limits>, event: views::Event) {
    count(&mut model.steps.views, &env.limits);
    let room = views::max_out(&env.limits.views);
    assert!(model.views_out.room() >= room, "an entry point steps the views within their bound");
    views::step(&mut model.views, &views_env(env), event, &mut model.views_out);
}

pub(crate) fn views_fire(model: &mut Model, env: &Env<Limits>) {
    count(&mut model.steps.views, &env.limits);
    views::fire(&mut model.views, &views_env(env), &mut model.views_out);
}

/// Counts a step of a sub-model in the entry point.
fn count(steps: &mut u32, limits: &Limits) {
    *steps = steps.saturating_add(1);
    assert!(*steps <= limits.steps, "an entry point takes no more steps of a sub-model than its limits allow");
}

/// One of the hub's requests: an answer to a call the top level made, or a
/// request of the plan, the rules, the forge, the fleet or the store.
fn from_work(model: &mut Model, env: &Env<Limits>, request: work::Request) {
    match request {
        work::Request::Taken { to } | work::Request::Stopped { to } | work::Request::Released { to } => {
            people::hub_answered(model, env, to, Ok(()));
        }
        work::Request::Refused { to, refusal } => people::hub_answered(model, env, to, Err(refusal)),
        work::Request::Due { owner, item } => jobs::due(model, env, owner, item),
        work::Request::Write { owner, item, lifecycle } => jobs::write(model, env, owner, item, lifecycle),
        work::Request::Record { owner, item, attempt, outcome } => {
            jobs::record(model, env, owner, item, attempt, outcome);
        }
        work::Request::Apply { owner, item, attempt, outcome } => {
            jobs::apply(model, env, owner, item, attempt, outcome);
        }
        work::Request::Act { owner, item, action } => jobs::act(model, env, owner, item, action),
        work::Request::Start { item, attempt, run } => runs::start(model, env, item, attempt, run),
        work::Request::Adopt { item, attempt } => runs::adopt(model, env, item, attempt),
        work::Request::Cancel { item, attempt } => runs::cancel(model, env, item, attempt),
        work::Request::Relay { item, attempt, event } => runs::inbound(model, env, item, attempt, event),
        work::Request::Keep { item, attempt: _, snapshot } => runs::keep(model, item, snapshot),
        work::Request::Acknowledge { item, attempt } | work::Request::Stale { item, attempt } => {
            runs::acknowledge(model, env, item, attempt);
        }
        work::Request::Left { item } => runs::left(model, env, item),
    }
}

/// One of the forge sub-model's requests: a call to the forge, an answer to
/// what the top level asked, or news of the working set.
fn from_forge(model: &mut Model, env: &Env<Limits>, request: forge::Request, out: &mut Queue<Request>) {
    match request {
        forge::Request::Call { call, repository, op } => serve::call(model, call, repository, op, out),
        forge::Request::Read { owner, result } => serve::read(model, env, owner, result),
        forge::Request::Wrote { owner, result } => serve::wrote(model, env, owner, result),
        forge::Request::Full { item } => people::full(model, item),
        forge::Request::Room => people::room(model, env),
        forge::Request::Announced { item, view } => items::announced(model, env, item, view),
        forge::Request::Offered { item } => people::offered(model, env, item),
        forge::Request::Inbox { item, seq, news } => runs::news(model, env, item, seq, news),
        // The labels a plan declares as inputs are not read yet: nothing
        // the engine decides follows labels.
        forge::Request::Changed { item: _, labels: _ } | forge::Request::Forbidden { item: _ } => {}
        forge::Request::Left { item, why: _ } => items::left(model, env, item),
        forge::Request::Loaded => runs::read(model),
    }
}

/// One of the fleet's requests: to a worker, through the protocol layer; or
/// to the hub, translated.
fn from_fleet(model: &mut Model, env: &Env<Limits>, request: fleet::Request, out: &mut Queue<Request>) {
    match request {
        fleet::Request::Assign { channel, run, attempt } => runs::assign(model, channel, run, attempt, out),
        fleet::Request::Inbound { channel, run, attempt, event } => {
            runs::deliver(model, channel, run, attempt, event, out);
        }
        fleet::Request::Cancel { channel, run, attempt } => {
            let item = translate::item(run);
            out.push(Request::Cancel { channel, item, attempt: attempt.raw() });
        }
        fleet::Request::Relayed { channel, run, attempt, call, answer } => {
            runs::relayed(model, channel, run, attempt, call, answer, out);
        }
        fleet::Request::Acknowledge { channel, run, attempt } => {
            let item = translate::item(run);
            out.push(Request::Acknowledge { channel, item, attempt: attempt.raw() });
        }
        fleet::Request::Refuse { channel } => out.push(Request::Refuse { channel }),
        fleet::Request::Placed { run, attempt } => runs::placed(model, env, run, attempt),
        fleet::Request::Listed { run, attempt } => {
            let item = translate::item(run);
            work_step(model, env, work::Event::Listed { item, attempt: attempt.raw() });
        }
        fleet::Request::Answered { to, run, attempt, answer: _, payload } => {
            runs::answered(model, env, to, run, attempt, payload);
        }
        fleet::Request::Lost { to, run, attempt } => {
            runs::ended(model, env, to, run, attempt, work::Answer::Lost);
        }
        fleet::Request::Withdrawn { to, run, attempt, withdrawal } => {
            assert!(
                withdrawal == fleet::Withdrawal::Cancelled,
                "an item has one live run: no attempt replaces another the fleet holds"
            );
            runs::ended(model, env, to, run, attempt, work::Answer::Refused);
        }
        fleet::Request::Refused { to, run, attempt, refusal: _ } => {
            runs::ended(model, env, to, run, attempt, work::Answer::Refused);
        }
        fleet::Request::Relay { reply_to, run, attempt, body } => runs::call(model, env, reply_to, run, attempt, body),
        fleet::Request::Bounced { run, attempt, bounce: _ } => runs::bounce(model, run, attempt),
        fleet::Request::Undelivered { run, attempt, event, undelivered } => match undelivered {
            fleet::Undelivered::Unplaced | fleet::Undelivered::Adrift => {
                let item = translate::item(run);
                work_step(model, env, work::Event::Undelivered { item, attempt: attempt.raw(), event });
            }
            // It stays in the item's inbox, for the next run.
            fleet::Undelivered::Gone => {}
        },
        fleet::Request::Told { run, attempt: _, fact } => runs::report(model, env, run, fact),
        fleet::Request::Drop { payload } => runs::forget(model, payload),
    }
}

/// One of the brief's requests: its answer to a render, or a read of a
/// section's source.
fn from_brief(model: &mut Model, env: &Env<Limits>, request: brief::Request) {
    match request {
        brief::Request::Rendered { reply_to, sections } => runs::rendered(model, env, reply_to, Ok(sections)),
        // A brief without a section its run is for: the run failed for a
        // while, and is tried again within its class's retries.
        brief::Request::Failed { reply_to, missing: _, why: _ } => {
            let failed = work::Answer::Failed(work::Class::Transient);
            runs::rendered(model, env, reply_to, Err(failed));
        }
        brief::Request::Refused { reply_to, refusal } => {
            let answer = match refusal {
                // No room for the brief: nothing ran, and the hub claims again
                // after a backoff.
                brief::Refusal::Busy => work::Answer::Refused,
                brief::Refusal::Oversized => work::Answer::Failed(work::Class::Permanent),
            };
            runs::rendered(model, env, reply_to, Err(answer));
        }
        // The hub claims again after its own backoff: room in the brief needs
        // no notice.
        brief::Request::Room => {}
        brief::Request::Read { owner, source, keep, fit, parts, bytes } => {
            serve::brief_read(model, env, owner, source, crate::waits::Bounds { keep, fit, parts, bytes });
        }
    }
}

/// One of the notes' requests: an answer to a call, or a wiki operation.
fn from_notes(model: &mut Model, env: &Env<Limits>, request: notes::Request) {
    match request {
        notes::Request::Indexed { reply_to, lines, more, unread: _ }
        | notes::Request::Found { reply_to, lines, more, unread: _ } => {
            serve::indexed(model, env, reply_to, lines, more);
        }
        notes::Request::Recalled { reply_to, entries, failed } => {
            serve::relay_served(model, env, reply_to, crate::boundary::Served::Recalled { entries, failed });
        }
        notes::Request::Noted { reply_to, noted } => {
            serve::relay_served(model, env, reply_to, crate::boundary::Served::Noted(noted));
        }
        notes::Request::Refused { reply_to, refusal } => serve::notes_refused(model, env, reply_to, refusal),
        notes::Request::List { owner, scope } => serve::list(model, env, owner, scope),
        notes::Request::Fetch { owner, scope, name } => serve::fetch(model, env, owner, scope, &name),
        notes::Request::Create { owner, scope, name, page } => serve::create(model, env, owner, scope, &name, page),
        notes::Request::Edit { owner, scope, name, page, revision } => {
            serve::edit(model, env, owner, scope, &name, page, revision);
        }
        notes::Request::Delete { owner, scope, name } => serve::delete(model, env, owner, scope, &name),
    }
}

/// One of the views' requests: to a person's stream, to the store, or the
/// answer to a watch.
fn from_views(model: &mut Model, env: &Env<Limits>, request: views::Request, out: &mut Queue<Request>) {
    match request {
        views::Request::Watching { watcher } => people::watching(model, watcher, None, out),
        views::Request::Refused { watcher, refusal } => people::watching(model, watcher, Some(refusal), out),
        views::Request::Deliver { watcher, missed, chunks } => {
            out.push(Request::Deliver { watcher, missed, chunks: translate::chunks(chunks) });
        }
        views::Request::Ended { watcher, end } => out.push(Request::Ended { watcher, end }),
        views::Request::Append { owner, records } => serve::append(model, env, owner, records, out),
        views::Request::Expire { owner, before } => serve::expire(model, owner, before, out),
    }
}
