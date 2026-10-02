//! The work items, and the attempts the engine makes at them
//! (engine-model.md, 4.2, simplified: no claims, no plans; one run per item).
//!
//! An item's transition table:
//!
//! ```text
//! state    event or alarm              next     emits
//! Waiting  its alarm                   Due      (on the due queue)
//!                                      Placed   assign (overbooked: no worker has a free slot, and
//!                                                 none holds an attempt of its workstream)
//! Due      resume, a free slot, no     Placed   assign (a stale event for the attempt it replaces, later)
//!            attempt of its workstream
//!            held
//! Placed   answered: busy              Waiting  (a retry) or Closed (no attempts left)
//!          answered: invalid snapshot  Waiting  (a retry, fresh) or Closed (no attempts left)
//!          answered: invalid           Closed
//!          answered: ended             Closed
//!          answered: parked            Waiting  (a wake) or Closed (no wakes or attempts left)
//!          answered: failed            Waiting  (a retry, by chance) or Closed (held)
//!          answered, after a cancel    Closed   (cancelled, unless the run ended or was invalid)
//!          its attempt lost            Waiting  (a retry, by chance) or Closed, as after a cancel
//! ```
//!
//! An attempt's:
//!
//! ```text
//! state      event or alarm                 next       emits
//! Live       inbound alarm                  Live       inbound (the alarm again, while events last)
//!            cancel alarm                   Cancelled  cancel
//!            hello: listed, not kept        Cancelled  cancel (from the ready list)
//!            hello: listed, kept or held    Live
//!            hello: not listed, or grace    Lost
//!            answered                       (gone)     (a late cancel, by chance)
//! Cancelled  hello: listed                  Cancelled  cancel again, unless held (the first may have been lost)
//!            hello: not listed, or grace    Lost
//!            answered                       (gone)     (a late cancel, by chance)
//! Lost       hello: listed                  Lost       cancel, unless held (its item has moved on)
//!            answered                       (gone)     (counted late)
//! ```
//!
//! "Held" is a run whose answer its worker holds, which follows the hello.
//! While its worker is out of contact, an attempt's alarms fire again later.
//! Calls, bounces and listings for an attempt that has answered, was
//! cancelled or was presumed lost are counted and dropped (attempts are
//! fenced); an answer to an attempt that has answered, or for one never made,
//! is a bug.
//!
//! A failure is retried by the `transient` chance when a retry may get past
//! it, and by the `permanent` chance when it may not; a busy refusal, and a
//! refusal of the snapshot, always are. Either way only while the item has
//! attempts left, and after a backoff.
//!
//! A parked run's snapshot is kept, by the `resumes` chance, for the attempt
//! that wakes it, and for the next ones until a run starts from it: an
//! attempt refused, unprepared or whose agent did not start leaves it for the
//! next. One that started spends it, and so does one that may have (it was
//! cancelled, or lost with its worker's contact): starting fresh always works
//! (engine-model.md, section 6). A snapshot the worker refuses is dropped.
//! A run's saved work is where its item's next attempt starts, in each
//! repository whose save landed.

use alloc::boxed::Box;
use core::mem;

use temper_lib::{Duration, Env, Id, Queue, Time, Token};

use crate::api::{
    Access, AgentFailure, Answer, Assignment, Cause, Failure, Invalid, Landing, Preparation, Refusal, RunFailure,
    Start, Work, Workspace,
};
use crate::fleet;
use crate::model::{Alarm, Config, Model, Request};

/// A work item.
#[derive(Debug)]
pub(crate) struct Item {
    workspace: Workspace,
    /// The saved-work branch, if it saves unfinished work.
    save: Option<Box<[u8]>>,
    /// Its charter, encoded.
    charter: Box<[u8]>,
    /// The snapshot of its last park, to resume from at the next wake.
    snapshot: Option<Box<[u8]>>,
    /// Assignments made for it, and wakes, so far.
    attempts: u32,
    wakes: u32,
    /// Its last attempt, and the worker that got it.
    last: Option<Last>,
    /// Its workstream, by its place among the configured ones. A workstream
    /// is one item's work at a time (engine-model.md, 4.1): no attempt of the
    /// item is assigned while a worker holds one of another of its
    /// workstream's.
    stream: u32,
    state: State,
}

#[derive(Clone, Copy, Debug)]
struct Last {
    worker: Token,
    attempt: Token,
}

#[derive(Debug)]
enum State {
    /// Its alarm is armed: its first start, a retry or a wake.
    Waiting,
    /// On the due queue, waiting for a free slot, and for no worker to hold
    /// an attempt of its workstream.
    Due,
    /// Its attempt `attempt` is out, and has not answered.
    Placed { attempt: Token },
    /// Terminal: holds nothing.
    Closed,
}

/// An attempt not answered yet.
#[derive(Debug)]
pub(crate) struct Attempt {
    pub(crate) item: Id<Item>,
    /// Its item's workstream.
    pub(crate) stream: u32,
    pub(crate) worker: Token,
    /// Inbound events sent to it.
    pub(crate) sent: u32,
    pub(crate) state: Attempted,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum Attempted {
    /// Out on its worker.
    Live,
    /// The engine cancelled it.
    Cancelled,
    /// Presumed lost with its worker's contact: its item has moved on, and its
    /// answer, if one comes, changes nothing.
    Lost,
}

/// How an item closed.
#[derive(Clone, Copy, Debug)]
enum Ending {
    Finished,
    Rejected,
    Held,
    Parked,
    Cancelled,
}

impl Item {
    pub(crate) fn new(workspace: Workspace, save: Option<Box<[u8]>>, charter: Box<[u8]>, stream: u32) -> Item {
        Item {
            workspace,
            save,
            charter,
            snapshot: None,
            attempts: 0,
            wakes: 0,
            last: None,
            stream,
            state: State::Waiting,
        }
    }
}

/// Whether `workspace` has a repository a run may write.
pub(crate) fn writable(workspace: &Workspace) -> bool {
    for repository in &workspace.repositories {
        match repository.access {
            Access::Writable { .. } => return true,
            Access::ReadOnly => {}
        }
    }
    false
}

/// Asserts the engine made `attempt`: attempts are numbered from one.
pub(crate) fn check_made(made: u64, attempt: Token) {
    assert!(attempt.raw() > 0 && attempt.raw() <= made, "the engine hears only of attempts it made");
}

/// The state of `attempt`, which traffic from `worker` names as the run
/// `run`'s, or `None` if it has answered. Asserts the engine made it, for that
/// run, on that worker.
pub(crate) fn state(model: &Model, worker: Token, run: Token, attempt: Token) -> Option<Attempted> {
    check_made(model.made, attempt);
    let record = model.attempts.get(&attempt)?;
    assert!(record.item == Id::from_token(run), "traffic names its attempt's run");
    assert!(record.worker == worker, "an attempt's traffic comes from the worker it was assigned to");
    Some(record.state)
}

/// Whether a worker holds an attempt of the workstream `stream`, as far as
/// the engine knows: out on it and not answered, its contact lost or not, or
/// presumed lost and listed by it since.
pub(crate) fn held(model: &Model, stream: u32) -> bool {
    for (_, worker) in &model.workers {
        for attempt in &worker.placed {
            let record = model.attempts.get(attempt).expect("an attempt held has not answered");
            if record.stream == stream {
                return true;
            }
        }
    }
    false
}

/// Whether a due item's workstream is held by no worker.
pub(crate) fn placeable(model: &Model) -> bool {
    for id in &model.due {
        let item = model.items.get(*id).expect("an item due lives");
        if !held(model, item.stream) {
            return true;
        }
    }
    false
}

/// Takes the oldest due item whose workstream no worker holds an attempt of,
/// if there is one, from the due queue, leaving the others in their order.
pub(crate) fn next_due(model: &mut Model) -> Option<Id<Item>> {
    let mut next = None;
    for _ in 0..model.due.len() {
        let id = model.due.pop().expect("as many as the queue holds");
        let stream = model.items.get(id).expect("an item due lives").stream;
        if next.is_none() && !held(model, stream) {
            next = Some(id);
        } else {
            model.due.push(id);
        }
    }
    next
}

/// Waiting, its alarm: the item is due. It waits for a free slot, unless no
/// worker has one and it is overbooked on a worker in contact; either way, it
/// waits while a worker holds an attempt of its workstream.
pub(crate) fn due(model: &mut Model, env: &Env<Config>, id: Id<Item>, out: &mut Queue<Request>) {
    let config = &env.limits;
    let stream = model.items.get(id).expect("an item lives until it closes").stream;
    let free = !held(model, stream);
    let item = model.items.get_mut(id).expect("an item lives until it closes");
    let full = fleet::free(&model.workers).is_none();
    let overbooked = if free && full && model.rng.chance(config.overbook) { fleet::up(&model.workers) } else { None };
    match overbooked {
        Some(worker) => {
            model.tally.overbooked = model.tally.overbooked.saturating_add(1);
            place(model, env, id, worker, out);
        }
        None => {
            item.state = match mem::replace(&mut item.state, State::Closed) {
                State::Waiting => State::Due,
                State::Due | State::Placed { .. } | State::Closed => {
                    unreachable!("an item's alarm runs while it waits")
                }
            };
            model.due.try_push(id).expect("room on the due queue for every item");
        }
    }
}

/// Waiting or Due: assigns the item's next attempt to `worker`.
pub(crate) fn place(model: &mut Model, env: &Env<Config>, id: Id<Item>, worker: Token, out: &mut Queue<Request>) {
    let config = &env.limits;
    let stream = model.items.get(id).expect("an item lives until it closes").stream;
    assert!(!held(model, stream), "a workstream is one item's work at a time");
    let Model { items, attempts, workers, timers, rng, tally, made, .. } = model;
    *made = made.checked_add(1).expect("attempts fit a u64");
    let attempt = Token::new(*made);
    let run = id.token();
    let item = items.get_mut(id).expect("an item lives until it closes");
    item.state = match mem::replace(&mut item.state, State::Closed) {
        State::Waiting | State::Due => State::Placed { attempt },
        State::Placed { .. } | State::Closed => unreachable!("an item is placed once it is due"),
    };
    item.attempts = item.attempts.saturating_add(1);
    if let Some(last) = item.last
        && rng.chance(config.stale)
    {
        let stale = Alarm::Stale { worker: last.worker, run, attempt: last.attempt };
        timers
            .arm(stale, after(env, rng.between(config.cancel_min.as_nanos(), config.cancel_max.as_nanos())))
            .expect("a timer per attempt");
    }
    item.last = Some(Last { worker, attempt });
    let assignment = Assignment {
        run,
        attempt,
        workspace: item.workspace.clone(),
        save: item.save.clone(),
        charter: item.charter.clone(),
        snapshot: item.snapshot.clone(),
    };
    let fresh = attempts.insert(attempt, Attempt { item: id, stream, worker, sent: 0, state: Attempted::Live });
    assert!(fresh.expect("room for every attempt").is_none(), "attempts are named apart");
    let placed = workers.get_mut(&worker).expect("placed on a worker that said hello").placed.insert(attempt);
    assert!(placed.expect("room for an attempt per item"), "attempts are named apart");
    if config.inbound > 0 {
        let at = after(env, rng.between(config.inbound_min.as_nanos(), config.inbound_max.as_nanos()));
        timers.arm(Alarm::Inbound(attempt), at).expect("a timer per attempt");
    }
    if rng.chance(config.cancels) {
        let at = after(env, rng.between(config.cancel_min.as_nanos(), config.cancel_max.as_nanos()));
        timers.arm(Alarm::Cancel(attempt), at).expect("a timer per attempt");
    }
    tally.assigned = tally.assigned.saturating_add(1);
    out.push(Request::Assign { worker, assignment });
}

/// Placed, answered: what the answer leads to, once it is acknowledged. An
/// answer for an attempt that has answered already is a duplicate, sent
/// again by a worker that did not hear the acknowledgement: acknowledged
/// again, counted and dropped.
pub(crate) fn answered(
    model: &mut Model,
    env: &Env<Config>,
    worker: Token,
    run: Token,
    attempt: Token,
    answer: Answer,
    out: &mut Queue<Request>,
) {
    let config = &env.limits;
    let known = state(model, worker, run, attempt);
    out.push(Request::Acknowledge { worker, run, attempt });
    if known.is_none() {
        model.tally.duplicates = model.tally.duplicates.saturating_add(1);
        return;
    }
    let record = model.attempts.remove(&attempt).expect("an attempt not answered has a record");
    model.timers.cancel(Alarm::Inbound(attempt));
    model.timers.cancel(Alarm::Cancel(attempt));
    let held = model.workers.get_mut(&worker).expect("a worker is known once it said hello").placed.remove(&attempt);
    let cancelled = match record.state {
        Attempted::Live => false,
        Attempted::Cancelled => true,
        Attempted::Lost => {
            model.tally.late = model.tally.late.saturating_add(1);
            late(model, record.item, answer);
            return;
        }
    };
    assert!(held, "an attempt out is held by its worker");
    if model.rng.chance(config.late_cancels) {
        let at = after(env, model.rng.between(config.cancel_min.as_nanos(), config.cancel_max.as_nanos()));
        let late = Alarm::Late { worker: record.worker, run, attempt };
        model.timers.arm(late, at).expect("a timer per attempt");
    }
    let id = record.item;
    let item = model.items.get_mut(id).expect("an item lives while its attempt is out");
    match item.state {
        State::Placed { attempt: placed } => assert!(placed == attempt, "an item waits on its last attempt"),
        State::Waiting | State::Due | State::Closed => unreachable!("an item waits on its attempt"),
    }
    if started(&answer) {
        item.snapshot = None;
    }
    let tally = &mut model.tally;
    match answer {
        Answer::Refused(Refusal::Busy) => {
            tally.busy = tally.busy.saturating_add(1);
            if cancelled { close(model, id, Ending::Cancelled) } else { retry(model, env, id) }
        }
        Answer::Refused(Refusal::Invalid(invalid)) => {
            tally.invalid = tally.invalid.saturating_add(1);
            match invalid {
                Invalid::Snapshot => {
                    item.snapshot = None;
                    if cancelled { close(model, id, Ending::Cancelled) } else { retry(model, env, id) }
                }
                Invalid::Repositories | Invalid::Duplicate | Invalid::Name | Invalid::Charter => {
                    close(model, id, Ending::Rejected);
                }
            }
        }
        Answer::Ended { outcome: _, work } => {
            tally.ended = tally.ended.saturating_add(1);
            record_work(model, id, work);
            close(model, id, Ending::Finished);
        }
        Answer::Parked { snapshot, work } => {
            tally.parked = tally.parked.saturating_add(1);
            record_work(model, id, work);
            if cancelled { close(model, id, Ending::Cancelled) } else { wake(model, env, id, snapshot) }
        }
        Answer::Failed { failure, work } => {
            tally.failed = tally.failed.saturating_add(1);
            record_work(model, id, work);
            if cancelled {
                close(model, id, Ending::Cancelled);
            } else if model.rng.chance(if transient(failure) { config.transient } else { config.permanent }) {
                retry(model, env, id);
            } else {
                close(model, id, Ending::Held);
            }
        }
    }
}

/// The attempt `attempt` is presumed lost with its worker's contact: its item
/// is retried as after a failure a retry may get past, unless the engine had
/// cancelled it. One lost already stays so. The caller no longer counts it
/// against its worker's slots.
pub(crate) fn lose(model: &mut Model, env: &Env<Config>, attempt: Token) {
    let config = &env.limits;
    let record = model.attempts.get_mut(&attempt).expect("an attempt held has not answered");
    let cancelled = match record.state {
        Attempted::Live => false,
        Attempted::Cancelled => true,
        Attempted::Lost => return,
    };
    record.state = Attempted::Lost;
    let id = record.item;
    model.items.get_mut(id).expect("an item lives while its attempt is out").snapshot = None;
    model.timers.cancel(Alarm::Inbound(attempt));
    model.timers.cancel(Alarm::Cancel(attempt));
    model.tally.lost = model.tally.lost.saturating_add(1);
    if cancelled {
        close(model, id, Ending::Cancelled);
    } else if model.rng.chance(config.transient) {
        retry(model, env, id);
    } else {
        close(model, id, Ending::Held);
    }
}

/// The answer of an attempt presumed lost, its item having moved on: what it
/// saved is where the item's next attempt starts, while the item is open.
fn late(model: &mut Model, id: Id<Item>, answer: Answer) {
    let work = match answer {
        Answer::Refused(_) => return,
        Answer::Ended { outcome: _, work }
        | Answer::Parked { snapshot: _, work }
        | Answer::Failed { failure: _, work } => work,
    };
    let Some(item) = model.items.get(id) else {
        return;
    };
    match item.state {
        State::Waiting | State::Due | State::Placed { .. } => record_work(model, id, work),
        State::Closed => {}
    }
}

/// Whether the attempt's run started, or may have, as its answer shows.
fn started(answer: &Answer) -> bool {
    match answer {
        Answer::Refused(_) => false,
        Answer::Ended { .. } | Answer::Parked { .. } => true,
        Answer::Failed { failure, work: _ } => match failure {
            Failure::Unprepared(_) | Failure::Agent(AgentFailure::Unstarted) => false,
            Failure::Run(_)
            | Failure::Agent(
                AgentFailure::Exited | AgentFailure::Rules | AgentFailure::NoProgress | AgentFailure::WallTime,
            )
            | Failure::Cancelled(_) => true,
        },
    }
}

/// Whether a retry may get past `failure`: the forge was out of reach, the
/// branch moved, the worker cancelled the run on its own, or its agent did
/// not start, exited or stalled. A run that failed of itself, broke the
/// channel's rules or ran out of time would fail the same way again.
fn transient(failure: Failure) -> bool {
    match failure {
        Failure::Unprepared(preparation) => match preparation {
            Preparation::Transient => true,
            Preparation::Permanent => false,
        },
        Failure::Run(run) => match run {
            RunFailure::Cancelled | RunFailure::Stale => true,
            RunFailure::Model | RunFailure::Budget | RunFailure::Policy => false,
        },
        Failure::Agent(agent) => match agent {
            AgentFailure::Unstarted | AgentFailure::Exited | AgentFailure::NoProgress => true,
            AgentFailure::Rules | AgentFailure::WallTime => false,
        },
        Failure::Cancelled(cause) => match cause {
            Cause::Contact | Cause::Shutdown => true,
            Cause::Engine => unreachable!("only an attempt the engine cancelled is cancelled by it"),
        },
    }
}

/// Checks what the run says it left on the forge against its workspace, and
/// starts the item's next attempt from its saved work where its save landed.
fn record_work(model: &mut Model, id: Id<Item>, work: Work) {
    let Work { landed, saved } = work;
    let item = model.items.get_mut(id).expect("an item lives while its attempt is out");
    let mut previous: Option<u32> = None;
    for &place in &landed {
        assert!(previous < Some(place), "landings are listed by place, ascending");
        previous = Some(place);
        let repository = item.workspace.repositories.get(usize::try_from(place).expect("a u32 fits in a usize"));
        match repository.expect("a landing is in a repository of the workspace").access {
            Access::Writable { .. } => {}
            Access::ReadOnly => unreachable!("nothing lands in a read-only repository"),
        }
    }
    let count = u32::try_from(landed.len()).expect("a workspace's repositories fit a u32");
    model.tally.landed = model.tally.landed.saturating_add(count);
    let Some(saved) = saved else {
        return;
    };
    let branch = item.save.as_ref().expect("work is saved only when the assignment asks");
    assert!(saved.len() == item.workspace.repositories.len(), "a save says what became of every repository");
    for (repository, &landing) in item.workspace.repositories.iter_mut().zip(&saved) {
        match landing {
            Landing::Landed => {
                match repository.access {
                    Access::Writable { .. } => {}
                    Access::ReadOnly => unreachable!("nothing is saved from a read-only repository"),
                }
                repository.start = Start::Saved { branch: branch.clone() };
                model.tally.saved = model.tally.saved.saturating_add(1);
            }
            Landing::Moved | Landing::Failed | Landing::Unchanged => {}
        }
    }
}

/// The item is due again after a backoff, if it has attempts left; else it is
/// held for a person.
fn retry(model: &mut Model, env: &Env<Config>, id: Id<Item>) {
    let config = &env.limits;
    let item = model.items.get_mut(id).expect("an item lives until it closes");
    if item.attempts >= config.attempts {
        close(model, id, Ending::Held);
        return;
    }
    item.state = waiting(&mut item.state);
    let at = after(env, model.rng.between(config.backoff_min.as_nanos(), config.backoff_max.as_nanos()));
    model.timers.arm(Alarm::Item(id), at).expect("a timer per item");
    model.tally.retries = model.tally.retries.saturating_add(1);
}

/// The item parked, with `snapshot`: it is woken later, if it has wakes and
/// attempts left, resumed from the snapshot or started fresh, as drawn now.
fn wake(model: &mut Model, env: &Env<Config>, id: Id<Item>, snapshot: Option<Box<[u8]>>) {
    let config = &env.limits;
    let item = model.items.get_mut(id).expect("an item lives until it closes");
    if item.wakes >= config.wakes || item.attempts >= config.attempts {
        close(model, id, Ending::Parked);
        return;
    }
    item.wakes = item.wakes.saturating_add(1);
    item.snapshot = match snapshot {
        Some(snapshot) if model.rng.chance(config.resumes) => {
            model.tally.resumed = model.tally.resumed.saturating_add(1);
            Some(snapshot)
        }
        Some(_) | None => None,
    };
    item.state = waiting(&mut item.state);
    let at = after(env, model.rng.between(config.wake_min.as_nanos(), config.wake_max.as_nanos()));
    model.timers.arm(Alarm::Item(id), at).expect("a timer per item");
    model.tally.wakes = model.tally.wakes.saturating_add(1);
}

/// Placed: waiting again.
fn waiting(state: &mut State) -> State {
    match mem::replace(state, State::Closed) {
        State::Placed { attempt: _ } => State::Waiting,
        State::Waiting | State::Due | State::Closed => unreachable!("an item waits again once its attempt is over"),
    }
}

/// Placed: the item is closed, and retired.
fn close(model: &mut Model, id: Id<Item>, ending: Ending) {
    let item = model.items.get_mut(id).expect("an item lives until it closes");
    item.state = match mem::replace(&mut item.state, State::Closed) {
        State::Placed { attempt: _ } => State::Closed,
        State::Waiting | State::Due | State::Closed => unreachable!("an item closes once its attempt is over"),
    };
    model.items.retire(id);
    let endings = &mut model.tally.endings;
    let count = match ending {
        Ending::Finished => &mut endings.finished,
        Ending::Rejected => &mut endings.rejected,
        Ending::Held => &mut endings.held,
        Ending::Parked => &mut endings.parked,
        Ending::Cancelled => &mut endings.cancelled,
    };
    *count = count.saturating_add(1);
}

/// `nanos` after now.
pub(crate) fn after(env: &Env<Config>, nanos: u64) -> Time {
    env.now.saturating_add(Duration::from_nanos(nanos))
}
