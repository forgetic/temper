//! Writes (seams: "Outcomes and writes"; engine-model.md, 4.4 and 12):
//! typed operations, serialised per lane, retried with a jittered backoff
//! when they fail for a while, and repeat-safe.
//!
//! A lane is an item, for the writes about one, or a repository, for the rest
//! (creating an issue or a pull request, deleting a branch, the wiki): one
//! write of a lane is in hand at a time, in the order the parent asked, so a
//! retry never lands after a later write of its lane, and labels end as the
//! last set written.
//!
//! What makes each write repeat-safe:
//!
//! - **Creations are keyed.** An issue carries its key inside it, a comment
//!   too, a pull request is keyed by its branches (the newest for them,
//!   open or not, is the one made), and a record is the one
//!   record of the engine's on its item. A creation whose attempt may have
//!   been made (it timed out) is looked for before it is tried again: among
//!   the issues the engine opened that were updated since the newest time
//!   the repository's listings had shown as the write began, a page at a
//!   time; among the item's comments after the last the working set had
//!   passed as it began; the pull request for its branches; the item's
//!   comments for a record of the engine's. So is one the parent asks for
//!   again ([`crate::Event::Write`]'s `resumed`), from after its cause, which
//!   the parent names, as nothing in the working set says where an earlier
//!   life's attempt went. A record found is the engine's own: one this write
//!   made is done, and one an earlier write made is edited, so the record
//!   says what this write carries.
//! - **Sets are written as sets,** and edits and deletions are the same
//!   whenever they land: tried again as they are. Labels are the engine's
//!   own: those wanted are added and the others it owns removed, so a label
//!   a person sets is never taken off. A deletion that finds nothing after an
//!   attempt that may have been made is done.
//! - **A merge names its head.** One whose attempt may have been made reads
//!   the pull request: merged at that head, it is done.
//! - **A record is read afresh before it is edited** (engine-model.md, 4.4),
//!   **and a wiki page before it is written:** one someone else changed or
//!   deleted since it was last read is not written over, and the parent hears
//!   what it now is. A record write aims at the record as the working set
//!   knew it when the parent asked for the write, or as the record writes
//!   before it in its lane left it, so one asked for before the parent heard
//!   of a change fails as well. Each record or page written carries a nonce
//!   of its write, so that the read after an attempt that may have been made
//!   tells the write's own landing, or that of an earlier write of its lane
//!   that gave up, from someone else's change.
//! - **What may land late is waited out.** A call that timed out may still
//!   take effect until `Limits::lifetime` after it went out: the write makes
//!   its next call no sooner, and a write that gives up holds its lane until
//!   then, so nothing of its lane lands before it.
//!
//! Its reads go out with the parent's fresh reads, ahead of the writes, and
//! the writes ahead of keeping up. A failure that may pass (unavailable, a
//! timeout) is tried again after a backoff, up to `Limits::attempts`; a
//! refusal for the rate waits for its reset, and counts no attempt; any other
//! is the answer.
//!
//! The transition table. `Due` asks for a call and `Done` answers the parent;
//! [`conclude`] settles both before the step ends.
//!
//! ```text
//! state    event                          next      requests
//! (none)   write, beyond the limits,      (none)    wrote: invalid, busy,
//!            busy, record unknown                     unknown
//!          write, its lane busy           Queued
//!          write, otherwise               Due       (its first call)
//! Queued   the lane's last write closes   Due
//! Check    a record or page as last read  Make
//!          one this write made            Done      wrote
//!          a record an earlier write of   Make
//!            its lane may have made
//!          a record or page changed       Done      wrote: edited, revised
//!            or gone
//!          a merge made at its head       Done      wrote: merged
//!          a pull request open at it      Make
//! Make     made                           Done      wrote
//!          labels added, some to remove   Unlabel
//!          timed out                      Waiting   (then Find, Check or Make)
//!          exists (a pull request)        Find
//!          missing (a deletion), after    Done      wrote: done
//!            an attempt that may be made
//! Unlabel  removed                        Done      wrote
//! Find     found, made by this write      Done      wrote
//!          a record of an earlier write   Make      (edits it)
//!          more to look at                Find
//!          not made                       Make
//! Busy     failed, rate                   Busy      (queued again)
//!          failed, may pass               Waiting   (or Done, attempts spent)
//!          failed, otherwise              Done      wrote: the error
//! Waiting  alarm                          Due
//! Done     its last call may still land   Holding   wrote
//!          otherwise                      Closed    wrote (the lane's next
//!                                                     is due)
//! Holding  alarm                          Closed    (the lane's next is due)
//! ```

use alloc::boxed::Box;
use core::mem;

use temper_lib::bytes::copy_of;
use temper_lib::{Env, Id, List, Queue, Rng, Slab, Time, Token};

use crate::api::{self, Answer, Body, Error, Kind, Mark, Op, State as Open};
use crate::boundary::{Cause, Content, Failure, Item, Position, Record, Request, Write, Written};
use crate::calls::{self, Purpose};
use crate::facts::{Fact, Priority};
use crate::items::{self, copy_labels};
use crate::limits::Limits;
use crate::model::{Alarm, Model};
use crate::scans;

/// A write in hand.
#[derive(Debug)]
pub(crate) struct Writing {
    owner: Token,
    write: Write,
    lane: Lane,
    /// The next write of its lane, which waits for this one to close.
    next: Option<Id<Writing>>,
    resumed: Option<Cause>,
    /// Whether what it created was found made by an earlier attempt.
    found: bool,
    /// Attempts that failed and may pass, whether a write call of it may
    /// have been made without its answer, and when its last call went out.
    attempts: u32,
    ambiguous: bool,
    sent: Time,
    /// Where a creation an earlier attempt made is looked for: issues updated
    /// since `since`, the forge's time, and comments after `from`.
    since: Time,
    from: u64,
    /// The nonce a record or a wiki page it writes carries.
    nonce: u64,
    /// A record's: the comment it edits and its revision as last read, or
    /// none to post one; the inbox position it carries; and the nonce of a
    /// record write before it that gave up after an attempt that may have
    /// landed.
    target: Option<Target>,
    position: Position,
    uncertain: Option<u64>,
    state: State,
}

/// Writes about one item go one at a time, and so do a repository's others.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub(crate) enum Lane {
    Item(Item),
    Repository(u32),
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
struct Target {
    comment: u64,
    revision: u64,
}

#[derive(Debug)]
enum State {
    /// Behind the writes before it in its lane.
    Queued,
    /// A call is to be queued.
    Due { phase: Phase },
    /// Its call is queued or out.
    Busy { phase: Phase },
    /// The last call failed: tried again at `until`.
    Waiting { phase: Phase, until: Time },
    /// The parent is to hear `result`, and the working set the record found.
    Done { result: Result<Written, Failure>, record: Option<Record> },
    /// Answered: it holds its lane until its last call, which timed out, can
    /// no longer land.
    Holding { until: Time },
    /// Terminal: holds nothing.
    Closed,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Phase {
    /// Reading afresh before writing: a record before it is edited, a wiki
    /// page before it is written; or the pull request after a merge that may
    /// have been made.
    Check,
    /// The write itself; for labels, adding those wanted.
    Make,
    /// Removing the labels the engine owns that are not wanted.
    Unlabel,
    /// Looking for what an earlier attempt made: issues updated at or after
    /// `since`, the `page`th page; or comments after `after`; or the pull
    /// request for its branches.
    Find { since: Time, page: u32, after: u64 },
}

/// A write from the parent: refused at the entrance, or taken, and started
/// once its lane is free.
pub(crate) fn write(
    model: &mut Model,
    env: &Env<Limits>,
    owner: Token,
    write: Write,
    resumed: Option<Cause>,
    out: &mut Queue<Request>,
) {
    if !valid(&write, &env.limits, &model.owned) {
        out.push(Request::Wrote { owner, result: Err(Failure::Invalid) });
        return;
    }
    if model.writes.is_full() {
        out.push(Request::Wrote { owner, result: Err(Failure::Busy) });
        return;
    }
    let (target, position, uncertain) = match &write {
        Write::Record { item, .. } => match items::record(model, *item) {
            Some((record, position, uncertain)) => (target_of(record), position, uncertain),
            None => {
                out.push(Request::Wrote { owner, result: Err(Failure::Unknown) });
                return;
            }
        },
        Write::CreateIssue { .. }
        | Write::Comment { .. }
        | Write::SetLabels { .. }
        | Write::OpenPull { .. }
        | Write::Merge { .. }
        | Write::Close { .. }
        | Write::DeleteBranch { .. }
        | Write::PutPage { .. }
        | Write::DeletePage { .. } => (None, Position::START, None),
    };
    let lane = lane(&write);
    let nonce = model.rng.next_u64();
    let writing = Writing {
        owner,
        write,
        lane,
        next: None,
        resumed,
        found: false,
        attempts: 0,
        ambiguous: false,
        sent: Time::ZERO,
        since: Time::ZERO,
        from: 0,
        nonce,
        target,
        position,
        uncertain,
        state: State::Queued,
    };
    let id = model.writes.insert(writing).expect("checked for room above");
    match model.lanes.get(&lane) {
        Some(&last) => {
            model.writes.get_mut(last).expect("a lane's last write lives until it closes").next = Some(id);
            model.lanes.insert(lane, id).expect("a lane per write");
        }
        None => {
            model.lanes.insert(lane, id).expect("a lane per write");
            start(model, env, id);
        }
    }
}

/// The write `id` has its lane: it reads what it needs of the working set as
/// it is now, and asks for its first call.
fn start(model: &mut Model, env: &Env<Limits>, id: Id<Writing>) {
    let writing = model.writes.get(id).expect("a write lives until it closes");
    // Where what it makes would be found: after its cause, if the parent
    // names one; otherwise after what the forge had shown before it began.
    let (since, from) = match writing.resumed {
        Some(cause) => (cause.at, cause.comment),
        None => {
            let since = scans::clock(model, repository(&writing.write));
            let from = match subject(&writing.write) {
                Some(item) => items::passed(model, item),
                None => 0,
            };
            (since, from)
        }
    };
    let known = match &writing.write {
        Write::Record { item, .. } => items::record(model, *item),
        Write::CreateIssue { .. }
        | Write::Comment { .. }
        | Write::SetLabels { .. }
        | Write::OpenPull { .. }
        | Write::Merge { .. }
        | Write::Close { .. }
        | Write::DeleteBranch { .. }
        | Write::PutPage { .. }
        | Write::DeletePage { .. } => None,
    };
    let unwanted = unwanted(&model.owned, &env.limits, &model.writes.get(id).expect("a write lives").write);
    let writing = model.writes.get_mut(id).expect("a write lives until it closes");
    writing.since = since;
    writing.from = from;
    if let Some((_, position, _)) = known {
        // It carries the position taken as it goes; it aims at the record as
        // it was when it was asked for, or as the writes before it in its lane
        // left it.
        writing.position = position;
    }
    let resumed = writing.resumed.is_some();
    let phase = match &writing.write {
        Write::Record { .. } => match writing.target {
            Some(_) => Phase::Check,
            None if resumed || writing.uncertain.is_some() => find(writing),
            None => Phase::Make,
        },
        Write::CreateIssue { .. } | Write::Comment { .. } | Write::OpenPull { .. } => {
            if resumed {
                find(writing)
            } else {
                Phase::Make
            }
        }
        Write::Merge { .. } => {
            if resumed {
                Phase::Check
            } else {
                Phase::Make
            }
        }
        Write::PutPage { .. } => Phase::Check,
        Write::SetLabels { labels, .. } => {
            if labels.is_empty() && !unwanted.is_empty() {
                Phase::Unlabel
            } else {
                Phase::Make
            }
        }
        Write::Close { .. } | Write::DeleteBranch { .. } | Write::DeletePage { .. } => Phase::Make,
    };
    writing.state = State::Due { phase };
    ask(model, id);
}

/// The first page of looking for what an earlier attempt made: a record
/// among all its item's comments, anything else after where the write
/// began.
fn find(writing: &Writing) -> Phase {
    let after = match writing.write {
        Write::Record { .. } => 0,
        Write::CreateIssue { .. }
        | Write::Comment { .. }
        | Write::SetLabels { .. }
        | Write::OpenPull { .. }
        | Write::Merge { .. }
        | Write::Close { .. }
        | Write::DeleteBranch { .. }
        | Write::PutPage { .. }
        | Write::DeletePage { .. } => writing.from,
    };
    Phase::Find { since: writing.since, page: 1, after }
}

/// The write `id`'s call goes out at `now`.
pub(crate) fn sent(model: &mut Model, id: Id<Writing>, now: Time) {
    model.writes.get_mut(id).expect("a write lives until its call is answered").sent = now;
}

/// A write's alarm: a backoff ended, and the call is tried again; or the
/// lane it held is free.
pub(crate) fn retry(model: &mut Model, env: &Env<Limits>, id: Id<Writing>) {
    let writing = model.writes.get_mut(id).expect("an alarm is cancelled as its write closes");
    match mem::replace(&mut writing.state, State::Closed) {
        State::Waiting { phase, until: _ } => {
            writing.state = State::Due { phase };
            ask(model, id);
        }
        State::Holding { until } => {
            assert!(env.now >= until, "the hold's alarm is armed for its end");
            close(model, env, id);
        }
        State::Queued | State::Due { .. } | State::Busy { .. } | State::Done { .. } | State::Closed => {
            unreachable!("a write's alarm runs while it waits or holds its lane")
        }
    }
}

/// Terminal for the write `id`'s call.
pub(crate) fn answered(
    model: &mut Model,
    env: &Env<Limits>,
    id: Id<Writing>,
    result: Result<Answer, Error>,
    out: &mut Queue<Request>,
) {
    let engine = model.config.engine;
    let unwanted = unwanted(&model.owned, &env.limits, &model.writes.get(id).expect("a write lives").write);
    let writing = model.writes.get_mut(id).expect("a write lives until its call is answered");
    let state = mem::replace(&mut writing.state, State::Closed);
    writing.state = match state {
        State::Busy { phase } => match phase {
            Phase::Check => checked(writing, engine, env, &mut model.rng, result),
            Phase::Make | Phase::Unlabel => made(writing, env, &mut model.rng, phase, !unwanted.is_empty(), result),
            Phase::Find { since, page, after } => {
                searched(writing, engine, env, &mut model.rng, since, page, after, result)
            }
        },
        State::Queued
        | State::Due { .. }
        | State::Waiting { .. }
        | State::Done { .. }
        | State::Holding { .. }
        | State::Closed => unreachable!("a write's terminal comes while its call is out"),
    };
    conclude(model, env, id, out);
}

/// What the write `id`'s call asks, as it goes out.
pub(crate) fn op(model: &Model, env: &Env<Limits>, id: Id<Writing>) -> (u32, Op) {
    let writing = model.writes.get(id).expect("a write lives until its call is answered");
    let phase = match writing.state {
        State::Busy { phase } => phase,
        State::Queued
        | State::Due { .. }
        | State::Waiting { .. }
        | State::Done { .. }
        | State::Holding { .. }
        | State::Closed => unreachable!("a write's call goes out while it is busy"),
    };
    let repository = repository(&writing.write);
    let op = match phase {
        Phase::Check => check_op(writing),
        Phase::Make => make_op(writing),
        Phase::Unlabel => Op::RemoveLabels {
            number: subject_number(&writing.write),
            labels: unwanted(&model.owned, &env.limits, &writing.write),
        },
        Phase::Find { since, page, after } => find_op(writing, model.config.engine, since, page, after),
    };
    (repository, op)
}

fn check_op(writing: &Writing) -> Op {
    match &writing.write {
        Write::Record { item, .. } => {
            let target = writing.target.expect("a record is checked when it has one");
            Op::Comment { number: item.number, id: target.comment }
        }
        Write::Merge { item, .. } => Op::Pull { number: item.number },
        Write::PutPage { name, .. } => Op::Page { name: copy_of(name) },
        Write::CreateIssue { .. }
        | Write::Comment { .. }
        | Write::SetLabels { .. }
        | Write::OpenPull { .. }
        | Write::Close { .. }
        | Write::DeleteBranch { .. }
        | Write::DeletePage { .. } => unreachable!("only a record, a page and a merge are checked"),
    }
}

fn make_op(writing: &Writing) -> Op {
    match &writing.write {
        Write::CreateIssue { repository: _, key, title, body, labels } => Op::CreateIssue {
            key: copy_of(key),
            title: copy_of(title),
            body: body_of(body),
            labels: copy_labels(labels),
        },
        Write::Comment { item, key, body } => {
            Op::Post { number: item.number, key: Some(copy_of(key)), body: body_of(body) }
        }
        Write::Record { item, payload } => {
            let body = Body::Record { payload: *payload, position: writing.position, nonce: writing.nonce };
            match writing.target {
                Some(target) => Op::EditComment { number: item.number, id: target.comment, body },
                None => Op::Post { number: item.number, key: None, body },
            }
        }
        Write::SetLabels { item, labels } => Op::AddLabels { number: item.number, labels: copy_labels(labels) },
        Write::OpenPull { repository: _, title, body, head, base } => {
            Op::OpenPull { title: copy_of(title), body: body_of(body), head: copy_of(head), base: copy_of(base) }
        }
        Write::Merge { item, head } => Op::Merge { number: item.number, head: *head },
        Write::Close { item } => Op::Close { number: item.number },
        Write::DeleteBranch { repository: _, branch } => Op::DeleteBranch { branch: copy_of(branch) },
        Write::PutPage { repository: _, name, content, revision: _ } => {
            Op::PutPage { name: copy_of(name), content: body_of(content), nonce: writing.nonce }
        }
        Write::DeletePage { repository: _, name } => Op::DeletePage { name: copy_of(name) },
    }
}

fn find_op(writing: &Writing, engine: u64, since: Time, page: u32, after: u64) -> Op {
    match &writing.write {
        Write::CreateIssue { .. } => {
            Op::Items { state: None, kind: Some(Kind::Issue), label: None, author: Some(engine), since, page }
        }
        Write::Comment { item, .. } | Write::Record { item, .. } => Op::Item { number: item.number, after },
        Write::OpenPull { head, base, .. } => Op::PullFor { head: copy_of(head), base: copy_of(base) },
        Write::SetLabels { .. }
        | Write::Merge { .. }
        | Write::Close { .. }
        | Write::DeleteBranch { .. }
        | Write::PutPage { .. }
        | Write::DeletePage { .. } => unreachable!("only creations are looked for"),
    }
}

fn body_of(content: &Content) -> Body {
    match content {
        Content::Text(text) => Body::Text(copy_of(text)),
        Content::Payload(token) => Body::Payload(*token),
    }
}

// Cell handlers: each takes the source state's data by value and returns the
// target state.

/// Check, read: a record or a page as last read is written, one this write
/// made is done, one changed by someone else is not written over; a merge
/// made at its head is done, one still open at it is tried.
fn checked(
    writing: &mut Writing,
    engine: u64,
    env: &Env<Limits>,
    rng: &mut Rng,
    result: Result<Answer, Error>,
) -> State {
    let answer = match result {
        Ok(answer) => answer,
        Err(Error::Missing) => return gone(writing),
        Err(error) => return failed(writing, env, rng, Phase::Check, error),
    };
    match &writing.write {
        Write::Record { .. } => {
            let comment = api::comment(answer);
            let target = writing.target.expect("a record is checked when it has one");
            if comment.revision == target.revision {
                return State::Due { phase: Phase::Make };
            }
            // A record of the engine's saying the nonce of a write that may
            // have landed: this one's, which is done; or an earlier one's of
            // its lane, which this one writes over.
            let nonce = match comment.mark {
                Mark::Record { nonce, .. } if comment.author == engine => Some(nonce),
                Mark::Record { .. } | Mark::None | Mark::Key(_) | Mark::Mangled => None,
            };
            if nonce == Some(writing.nonce) {
                let record =
                    Record::Found { comment: comment.id, revision: comment.revision, position: writing.position };
                return State::Done { result: Ok(Written::Done), record: Some(record) };
            }
            if nonce.is_some() && nonce == writing.uncertain {
                writing.target = Some(Target { comment: comment.id, revision: comment.revision });
                return State::Due { phase: Phase::Make };
            }
            let record = found(&comment, engine);
            State::Done { result: Err(Failure::Edited { record }), record: Some(record) }
        }
        Write::PutPage { revision, .. } => {
            let page = api::page(answer);
            if Some(page.revision) == *revision {
                return State::Due { phase: Phase::Make };
            }
            if page.nonce == Some(writing.nonce) {
                return State::Done { result: Ok(Written::Revision(page.revision)), record: None };
            }
            State::Done { result: Err(Failure::Revised { revision: Some(page.revision) }), record: None }
        }
        Write::Merge { head, .. } => {
            let pull = api::pull(answer);
            match pull.merged {
                Some(merged) if pull.commit == *head => {
                    State::Done { result: Ok(Written::Merged(merged)), record: None }
                }
                None if pull.state == Open::Closed => refused(Error::Closed),
                None if pull.commit == *head => State::Due { phase: Phase::Make },
                Some(_) | None => refused(Error::Stale),
            }
        }
        Write::CreateIssue { .. }
        | Write::Comment { .. }
        | Write::SetLabels { .. }
        | Write::OpenPull { .. }
        | Write::Close { .. }
        | Write::DeleteBranch { .. }
        | Write::DeletePage { .. } => unreachable!("only a record, a page and a merge are checked"),
    }
}

/// Check, missing: a record deleted is not posted again unasked; a page is
/// written if it was to be made, and is not if someone deleted it.
fn gone(writing: &Writing) -> State {
    match &writing.write {
        Write::Record { .. } => {
            let record = Record::Missing;
            State::Done { result: Err(Failure::Edited { record }), record: Some(record) }
        }
        Write::PutPage { revision, .. } => match revision {
            None => State::Due { phase: Phase::Make },
            Some(_) => State::Done { result: Err(Failure::Revised { revision: None }), record: None },
        },
        Write::CreateIssue { .. }
        | Write::Comment { .. }
        | Write::SetLabels { .. }
        | Write::OpenPull { .. }
        | Write::Merge { .. }
        | Write::Close { .. }
        | Write::DeleteBranch { .. }
        | Write::DeletePage { .. } => refused(Error::Missing),
    }
}

/// The record a comment of the engine's, or someone else's, holds.
fn found(comment: &api::Comment, engine: u64) -> Record {
    match comment.mark {
        Mark::Record { position, .. } if comment.author == engine => {
            Record::Found { comment: comment.id, revision: comment.revision, position }
        }
        Mark::Record { .. } | Mark::Mangled | Mark::None | Mark::Key(_) => {
            Record::Mangled { comment: comment.id, revision: comment.revision }
        }
    }
}

/// Make or Unlabel, answered: done, or the labels left to remove; or what
/// an attempt that may have been made, or a refusal, calls for.
fn made(
    writing: &mut Writing,
    env: &Env<Limits>,
    rng: &mut Rng,
    phase: Phase,
    unlabel: bool,
    result: Result<Answer, Error>,
) -> State {
    let error = match result {
        Ok(Answer::Done) if phase == Phase::Make && unlabel && is_labels(&writing.write) => {
            return State::Due { phase: Phase::Unlabel };
        }
        Ok(answer) => return done(writing, answer),
        Err(error) => error,
    };
    match error {
        Error::Timeout => {
            writing.ambiguous = true;
            let next = match &writing.write {
                Write::CreateIssue { .. } | Write::Comment { .. } | Write::OpenPull { .. } => find(writing),
                Write::Record { .. } => match writing.target {
                    Some(_) => Phase::Check,
                    None => find(writing),
                },
                Write::Merge { .. } | Write::PutPage { .. } => Phase::Check,
                Write::SetLabels { .. } => phase,
                Write::Close { .. } | Write::DeleteBranch { .. } | Write::DeletePage { .. } => Phase::Make,
            };
            failed(writing, env, rng, next, error)
        }
        Error::Exists => match writing.write {
            Write::OpenPull { .. } => State::Due { phase: find(writing) },
            Write::CreateIssue { .. }
            | Write::Comment { .. }
            | Write::Record { .. }
            | Write::SetLabels { .. }
            | Write::Merge { .. }
            | Write::Close { .. }
            | Write::DeleteBranch { .. }
            | Write::PutPage { .. }
            | Write::DeletePage { .. } => refused(error),
        },
        Error::Missing if writing.ambiguous => match writing.write {
            Write::DeleteBranch { .. } | Write::DeletePage { .. } => {
                State::Done { result: Ok(Written::Done), record: None }
            }
            Write::CreateIssue { .. }
            | Write::Comment { .. }
            | Write::Record { .. }
            | Write::SetLabels { .. }
            | Write::OpenPull { .. }
            | Write::Merge { .. }
            | Write::Close { .. }
            | Write::PutPage { .. } => refused(error),
        },
        Error::Closed | Error::Stale if writing.ambiguous => match writing.write {
            Write::Merge { .. } => State::Due { phase: Phase::Check },
            Write::CreateIssue { .. }
            | Write::Comment { .. }
            | Write::Record { .. }
            | Write::SetLabels { .. }
            | Write::OpenPull { .. }
            | Write::Close { .. }
            | Write::DeleteBranch { .. }
            | Write::PutPage { .. }
            | Write::DeletePage { .. } => refused(error),
        },
        Error::Unavailable
        | Error::RateLimited { .. }
        | Error::Forbidden
        | Error::Missing
        | Error::TooLarge
        | Error::Empty
        | Error::Full
        | Error::NothingToMerge
        | Error::Closed
        | Error::Stale
        | Error::Conflict
        | Error::Protected => failed(writing, env, rng, phase, error),
    }
}

/// Find, answered: what an earlier attempt made, or more to look at, or the
/// write tried, as nothing was made.
#[expect(clippy::too_many_arguments, reason = "a cell handler over the fields it touches")]
fn searched(
    writing: &mut Writing,
    engine: u64,
    env: &Env<Limits>,
    rng: &mut Rng,
    since: Time,
    page: u32,
    after: u64,
    result: Result<Answer, Error>,
) -> State {
    let answer = match result {
        Ok(answer) => answer,
        Err(Error::Missing) => match writing.write {
            Write::OpenPull { .. } => return State::Due { phase: Phase::Make },
            Write::CreateIssue { .. }
            | Write::Comment { .. }
            | Write::Record { .. }
            | Write::SetLabels { .. }
            | Write::Merge { .. }
            | Write::Close { .. }
            | Write::DeleteBranch { .. }
            | Write::PutPage { .. }
            | Write::DeletePage { .. } => return refused(Error::Missing),
        },
        Err(error) => return failed(writing, env, rng, Phase::Find { since, page, after }, error),
    };
    let most = page_size(&env.limits);
    match &writing.write {
        Write::CreateIssue { key, .. } => {
            let (items, more, _) = api::items(answer);
            let items = items.get(..most).unwrap_or(&items);
            for summary in items {
                let keyed = match &summary.key {
                    Some(found) => **found == **key,
                    None => false,
                };
                if keyed && summary.author == engine {
                    writing.found = true;
                    return State::Done { result: Ok(Written::Created(summary.number)), record: None };
                }
            }
            if !more {
                return State::Due { phase: Phase::Make };
            }
            let (since, page) = match items.last() {
                Some(last) if last.updated > since => (last.updated, 1),
                Some(_) | None => (since, page.saturating_add(1)),
            };
            State::Due { phase: Phase::Find { since, page, after } }
        }
        Write::Comment { key, .. } => {
            let (_, comments, more) = api::item(answer);
            let comments = comments.get(..most).unwrap_or(&comments);
            let mut last = after;
            for comment in comments {
                last = last.max(comment.id);
                let keyed = match &comment.mark {
                    Mark::Key(found) => **found == **key,
                    Mark::None | Mark::Record { .. } | Mark::Mangled => false,
                };
                if keyed && comment.author == engine {
                    writing.found = true;
                    return State::Done { result: Ok(Written::Commented(comment.id)), record: None };
                }
            }
            if more && last > after {
                State::Due { phase: Phase::Find { since, page, after: last } }
            } else {
                State::Due { phase: Phase::Make }
            }
        }
        Write::Record { .. } => {
            let (_, comments, more) = api::item(answer);
            let comments = comments.get(..most).unwrap_or(&comments);
            found_record(writing, engine, comments, more, since, page, after)
        }
        Write::OpenPull { .. } => {
            // The newest pull request for its branches is the one an earlier
            // attempt made, even if someone closed it since.
            let pull = api::pull(answer);
            writing.found = true;
            State::Done { result: Ok(Written::Created(pull.number)), record: None }
        }
        Write::SetLabels { .. }
        | Write::Merge { .. }
        | Write::Close { .. }
        | Write::DeleteBranch { .. }
        | Write::PutPage { .. }
        | Write::DeletePage { .. } => unreachable!("only creations are looked for"),
    }
}

/// Find, a page of a record's item's comments: a record of the engine's this
/// write made is done; one an earlier write made is edited; a mangled one is
/// not written over.
fn found_record(
    writing: &mut Writing,
    engine: u64,
    comments: &[api::Comment],
    more: bool,
    since: Time,
    page: u32,
    after: u64,
) -> State {
    let mut last = after;
    for comment in comments {
        last = last.max(comment.id);
        if comment.author != engine {
            continue;
        }
        match comment.mark {
            Mark::Record { nonce, position: _ } if nonce == writing.nonce => {
                // This write's own post landed.
                writing.found = true;
                let record =
                    Record::Found { comment: comment.id, revision: comment.revision, position: writing.position };
                return State::Done { result: Ok(Written::Done), record: Some(record) };
            }
            Mark::Record { .. } => {
                // An earlier write's: this one edits it, so that the
                // record says what this one carries.
                writing.target = Some(Target { comment: comment.id, revision: comment.revision });
                return State::Due { phase: Phase::Make };
            }
            Mark::Mangled => {
                let record = Record::Mangled { comment: comment.id, revision: comment.revision };
                return State::Done { result: Err(Failure::Edited { record }), record: Some(record) };
            }
            Mark::None | Mark::Key(_) => {}
        }
    }
    if more && last > after {
        State::Due { phase: Phase::Find { since, page, after: last } }
    } else {
        State::Due { phase: Phase::Make }
    }
}

/// A write made: what the parent hears, and a record's place.
fn done(writing: &Writing, answer: Answer) -> State {
    let (written, record) = match answer {
        Answer::Created(number) => (Written::Created(number), None),
        Answer::Commented { id, revision } => match writing.write {
            Write::Record { .. } => {
                (Written::Done, Some(Record::Found { comment: id, revision, position: writing.position }))
            }
            Write::CreateIssue { .. }
            | Write::Comment { .. }
            | Write::SetLabels { .. }
            | Write::OpenPull { .. }
            | Write::Merge { .. }
            | Write::Close { .. }
            | Write::DeleteBranch { .. }
            | Write::PutPage { .. }
            | Write::DeletePage { .. } => (Written::Commented(id), None),
        },
        Answer::Edited { revision } => {
            let target = writing.target.expect("only a record is edited");
            (Written::Done, Some(Record::Found { comment: target.comment, revision, position: writing.position }))
        }
        Answer::Merged(commit) => (Written::Merged(commit), None),
        Answer::Revision(revision) => (Written::Revision(revision), None),
        Answer::Done => (Written::Done, None),
        Answer::Items { .. }
        | Answer::Item { .. }
        | Answer::Comment(_)
        | Answer::Pull(_)
        | Answer::Reviews { .. }
        | Answer::Statuses { .. }
        | Answer::Permission(_)
        | Answer::Commit(_)
        | Answer::Pages { .. }
        | Answer::Page(_) => unreachable!("a write is answered as one"),
    };
    State::Done { result: Ok(written), record }
}

/// A call failed: queued again at once for the rate, whose reset holds every
/// call; tried again after a backoff if it may pass, until the attempts run
/// out, and no sooner than a write that timed out can still land; the answer
/// otherwise.
fn failed(writing: &mut Writing, env: &Env<Limits>, rng: &mut Rng, phase: Phase, error: Error) -> State {
    match error {
        Error::RateLimited { .. } => State::Due { phase },
        Error::Unavailable | Error::Timeout => {
            writing.attempts = writing.attempts.saturating_add(1);
            if writing.attempts >= env.limits.attempts {
                return refused(error);
            }
            let mut until = env.now.saturating_add(items::backoff(&env.limits, rng, writing.attempts));
            if error == Error::Timeout {
                until = until.max(writing.sent.saturating_add(env.limits.lifetime));
            }
            State::Waiting { phase, until }
        }
        Error::Forbidden
        | Error::Missing
        | Error::TooLarge
        | Error::Empty
        | Error::Full
        | Error::Exists
        | Error::NothingToMerge
        | Error::Closed
        | Error::Stale
        | Error::Conflict
        | Error::Protected => refused(error),
    }
}

fn refused(error: Error) -> State {
    State::Done { result: Err(Failure::Forge(error)), record: None }
}

/// Applied after every transition: a call due is queued; a write done
/// answers the parent, and closes or holds its lane; then the backoff's
/// alarm runs only while it waits.
fn conclude(model: &mut Model, env: &Env<Limits>, id: Id<Writing>, out: &mut Queue<Request>) {
    let writing = model.writes.get_mut(id).expect("a write lives until it closes");
    match mem::replace(&mut writing.state, State::Closed) {
        State::Due { phase } => {
            writing.state = State::Due { phase };
            ask(model, id);
        }
        State::Done { result, record } => answer(model, env, id, result, record, out),
        State::Waiting { phase, until } => {
            writing.state = State::Waiting { phase, until };
            model.facts.push(Fact::Retried { owner: writing.owner });
            model.alarms.arm(Alarm::Write(id), until).expect("an alarm per write fits");
        }
        State::Queued => writing.state = State::Queued,
        State::Busy { phase } => writing.state = State::Busy { phase },
        State::Holding { .. } | State::Closed => unreachable!("a write is concluded while it is in hand"),
    }
}

/// A write done: the working set hears what it found of its item's record,
/// and so do the record writes queued after it; the parent hears how it went;
/// and it closes, or holds its lane while its last call may still land.
fn answer(
    model: &mut Model,
    env: &Env<Limits>,
    id: Id<Writing>,
    result: Result<Written, Failure>,
    record: Option<Record>,
    out: &mut Queue<Request>,
) {
    let writing = model.writes.get(id).expect("a write lives until it closes");
    let (owner, found, next, nonce, sent) = (writing.owner, writing.found, writing.next, writing.nonce, writing.sent);
    // Given up after a write call that may have been made.
    let gave_up = match result {
        Err(Failure::Forge(_)) => writing.ambiguous,
        Ok(_)
        | Err(Failure::Busy | Failure::Invalid | Failure::Unknown | Failure::Edited { .. } | Failure::Revised { .. }) => {
            false
        }
    };
    let recording = match writing.write {
        Write::Record { item, .. } => Some(item),
        Write::CreateIssue { .. }
        | Write::Comment { .. }
        | Write::SetLabels { .. }
        | Write::OpenPull { .. }
        | Write::Merge { .. }
        | Write::Close { .. }
        | Write::DeleteBranch { .. }
        | Write::PutPage { .. }
        | Write::DeletePage { .. } => None,
    };
    // Given up as its last call timed out: it may still land.
    let pending = result == Err(Failure::Forge(Error::Timeout));
    if found {
        model.facts.push(Fact::Found { owner });
    }
    if let Some(item) = recording {
        match record {
            Some(record) => {
                items::recorded(model, item, record);
                if result.is_ok() {
                    follow_record(&mut model.writes, next, Some(record), None, env.limits.writes);
                }
            }
            None => {
                // The record may now say what this one carried.
                if gave_up {
                    items::uncertain(model, item, nonce);
                    follow_record(&mut model.writes, next, None, Some(nonce), env.limits.writes);
                }
            }
        }
        match result {
            Err(Failure::Edited { .. }) => model.facts.push(Fact::Edited { item }),
            Ok(_)
            | Err(Failure::Busy | Failure::Invalid | Failure::Unknown | Failure::Revised { .. } | Failure::Forge(_)) => {
            }
        }
    }
    match result {
        Ok(_) => model.facts.push(Fact::Wrote { owner }),
        Err(_) => model.facts.push(Fact::Unwritten { owner }),
    }
    out.push(Request::Wrote { owner, result });
    let until = sent.saturating_add(env.limits.lifetime);
    if pending && env.now < until {
        let writing = model.writes.get_mut(id).expect("a write lives until it closes");
        writing.state = State::Holding { until };
        model.alarms.arm(Alarm::Write(id), until).expect("an alarm per write fits");
        return;
    }
    close(model, env, id);
}

/// Queues the call a write is due.
fn ask(model: &mut Model, id: Id<Writing>) {
    let writing = model.writes.get_mut(id).expect("a write lives until it closes");
    let phase = match writing.state {
        State::Due { phase } => phase,
        State::Queued
        | State::Busy { .. }
        | State::Waiting { .. }
        | State::Done { .. }
        | State::Holding { .. }
        | State::Closed => unreachable!("a write asks for a call when one is due"),
    };
    let priority = match phase {
        Phase::Check | Phase::Find { .. } => Priority::Fresh,
        Phase::Make | Phase::Unlabel => Priority::Write,
    };
    calls::queue(&mut model.calls, Purpose::Write(id), priority);
    writing.state = State::Busy { phase };
    model.alarms.cancel(Alarm::Write(id));
}

/// The write `id` is done: it is retired, and the next of its lane starts.
fn close(model: &mut Model, env: &Env<Limits>, id: Id<Writing>) {
    let writing = model.writes.get_mut(id).expect("a write lives until it closes");
    let (lane, next) = (writing.lane, writing.next);
    writing.state = State::Closed;
    model.alarms.cancel(Alarm::Write(id));
    model.writes.retire(id);
    match next {
        Some(next) => start(model, env, next),
        None => {
            let last = model.lanes.remove(&lane);
            assert!(last == Some(id), "a lane's last write is the one closing with none after it");
        }
    }
}

/// A write posted or edited its item's record, which is now `record`: the
/// record writes queued after it in its lane aim at it. Or it gave up after
/// an attempt carrying the nonce `uncertain` that may have landed: they take
/// a record saying it as their own. A record found changed by someone else is
/// not passed on: the writes asked for before the parent heard of it fail as
/// this one did.
fn follow_record(
    writes: &mut Slab<Writing>,
    next: Option<Id<Writing>>,
    record: Option<Record>,
    uncertain: Option<u64>,
    most: u32,
) {
    let mut next = next;
    for _ in 0..most {
        let Some(id) = next else {
            return;
        };
        let writing = writes.get_mut(id).expect("a lane's writes live until they close");
        match writing.write {
            Write::Record { .. } => match record {
                Some(record) => {
                    writing.target = target_of(record);
                    writing.uncertain = None;
                }
                None => writing.uncertain = uncertain,
            },
            Write::CreateIssue { .. }
            | Write::Comment { .. }
            | Write::SetLabels { .. }
            | Write::OpenPull { .. }
            | Write::Merge { .. }
            | Write::Close { .. }
            | Write::DeleteBranch { .. }
            | Write::PutPage { .. }
            | Write::DeletePage { .. } => {}
        }
        next = writing.next;
    }
}

/// What a record write aims at: the comment it edits and its revision as
/// last read, or none to post one.
fn target_of(record: Record) -> Option<Target> {
    match record {
        Record::Missing => None,
        Record::Mangled { comment, revision } | Record::Found { comment, revision, .. } => {
            Some(Target { comment, revision })
        }
    }
}

fn is_labels(write: &Write) -> bool {
    match write {
        Write::SetLabels { .. } => true,
        Write::CreateIssue { .. }
        | Write::Comment { .. }
        | Write::Record { .. }
        | Write::OpenPull { .. }
        | Write::Merge { .. }
        | Write::Close { .. }
        | Write::DeleteBranch { .. }
        | Write::PutPage { .. }
        | Write::DeletePage { .. } => false,
    }
}

/// The labels the engine owns, of `owned`, that a label write does not
/// want, which it removes; none for any other write.
fn unwanted(owned: &[Box<[u8]>], limits: &Limits, write: &Write) -> Box<[Box<[u8]>]> {
    let labels = match write {
        Write::SetLabels { labels, .. } => labels,
        Write::CreateIssue { .. }
        | Write::Comment { .. }
        | Write::Record { .. }
        | Write::OpenPull { .. }
        | Write::Merge { .. }
        | Write::Close { .. }
        | Write::DeleteBranch { .. }
        | Write::PutPage { .. }
        | Write::DeletePage { .. } => return Box::new([]),
    };
    let mut unwanted = List::with_capacity(limits.labels.saturating_add(2));
    for label in owned {
        let mut wanted = false;
        for kept in labels {
            if **kept == **label {
                wanted = true;
            }
        }
        if !wanted {
            unwanted.push(copy_of(label)).expect("as many as the labels the engine owns");
        }
    }
    unwanted.into_boxed()
}

/// The number of the item a write is about, if it is about one.
fn subject_number(write: &Write) -> u64 {
    match subject(write) {
        Some(item) => item.number,
        None => unreachable!("only a write about an item names one"),
    }
}

/// The lane of a write.
fn lane(write: &Write) -> Lane {
    match subject(write) {
        Some(item) => Lane::Item(item),
        None => Lane::Repository(repository(write)),
    }
}

/// The item a write is about, if it is about one that exists.
fn subject(write: &Write) -> Option<Item> {
    match write {
        Write::Comment { item, .. }
        | Write::Record { item, .. }
        | Write::SetLabels { item, .. }
        | Write::Merge { item, .. }
        | Write::Close { item } => Some(*item),
        Write::CreateIssue { .. }
        | Write::OpenPull { .. }
        | Write::DeleteBranch { .. }
        | Write::PutPage { .. }
        | Write::DeletePage { .. } => None,
    }
}

fn repository(write: &Write) -> u32 {
    match write {
        Write::Comment { item, .. }
        | Write::Record { item, .. }
        | Write::SetLabels { item, .. }
        | Write::Merge { item, .. }
        | Write::Close { item } => item.repository,
        Write::CreateIssue { repository, .. }
        | Write::OpenPull { repository, .. }
        | Write::DeleteBranch { repository, .. }
        | Write::PutPage { repository, .. }
        | Write::DeletePage { repository, .. } => *repository,
    }
}

/// Whether a write is within the limits, about the deployment's
/// repositories, setting only labels the engine owns.
fn valid(write: &Write, limits: &Limits, owned: &[Box<[u8]>]) -> bool {
    if repository(write) >= limits.repositories {
        return false;
    }
    let name = limits.name_bytes;
    match write {
        Write::CreateIssue { repository: _, key, title, body, labels } => {
            fits(key, name) && fits(title, limits.title_bytes) && content(body, limits) && labelled(labels, limits)
        }
        Write::Comment { item: _, key, body } => fits(key, name) && content(body, limits),
        Write::Record { .. } | Write::Merge { .. } | Write::Close { .. } => true,
        Write::SetLabels { item: _, labels } => labelled(labels, limits) && owns(owned, labels),
        Write::OpenPull { repository: _, title, body, head, base } => {
            fits(title, limits.title_bytes) && content(body, limits) && fits(head, name) && fits(base, name)
        }
        Write::DeleteBranch { repository: _, branch } => fits(branch, name),
        Write::PutPage { repository: _, name: page, content: body, revision: _ } => {
            fits(page, name) && content(body, limits)
        }
        Write::DeletePage { repository: _, name: page } => fits(page, name),
    }
}

fn fits(bytes: &[u8], most: u32) -> bool {
    match u32::try_from(bytes.len()) {
        Ok(len) => len <= most,
        Err(_) => false,
    }
}

fn content(content: &Content, limits: &Limits) -> bool {
    match content {
        Content::Text(text) => fits(text, limits.body_bytes),
        Content::Payload(_) => true,
    }
}

fn labelled(labels: &[Box<[u8]>], limits: &Limits) -> bool {
    let Ok(count) = u32::try_from(labels.len()) else {
        return false;
    };
    if count > limits.labels {
        return false;
    }
    for label in labels {
        if !fits(label, limits.name_bytes) {
            return false;
        }
    }
    true
}

/// Whether the engine owns each of `labels`, as `owned` says.
fn owns(owned: &[Box<[u8]>], labels: &[Box<[u8]>]) -> bool {
    for label in labels {
        let mut ours = false;
        for kept in owned {
            if **kept == **label {
                ours = true;
            }
        }
        if !ours {
            return false;
        }
    }
    true
}

fn page_size(limits: &Limits) -> usize {
    usize::try_from(limits.page).expect("a u32 fits in a usize")
}
