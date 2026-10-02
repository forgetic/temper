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
//!   too, a pull request is keyed by its branches, and a record is the one
//!   record of the engine's on its item. A creation whose attempt may have
//!   been made (it timed out, or its answer was lost) is looked for before it
//!   is tried again: issues updated since the first attempt went out, a
//!   page at a time; the item's comments after the last the working set had
//!   passed; the pull request for its branches. So is one the parent asks for
//!   again after a restart ([`crate::Event::Write`]'s `resumed`).
//! - **Sets are written as sets,** and edits and deletions are the same
//!   whenever they land: tried again as they are. A deletion that finds
//!   nothing after an attempt that may have been made is done.
//! - **A merge names its head.** One whose attempt may have been made reads
//!   the pull request: merged at that head, it is done.
//! - **A record is read afresh before it is edited** (engine-model.md, 4.4):
//!   one someone else changed or deleted since it was last read is not
//!   written over, and the parent hears what it now is. A record write aims at
//!   the record as the working set knew it when the parent asked for the
//!   write, or as the record writes before it in its lane left it, so one
//!   asked for before the parent heard of a change fails as well. After an
//!   edit that may have been made, it is edited again without the check,
//!   which would find the engine's own edit.
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
//! Check    a record as last read          Make
//!          a record changed or gone       Done      wrote: edited
//!          a merge made at its head       Done      wrote: merged
//!          a pull request open at it      Make
//! Make     made                           Done      wrote
//!          timed out                      Waiting   (then Find, Check or Make)
//!          exists (a pull request)        Find
//!          missing (a deletion), after    Done      wrote: done
//!            an attempt that may be made
//! Find     found                          Done      wrote
//!          more to look at                Find
//!          not made                       Make
//! Busy     failed, rate                   Busy      (queued again)
//!          failed, may pass               Waiting   (or Done, attempts spent)
//!          failed, otherwise              Done      wrote: the error
//! Waiting  alarm                          Due
//! Done     (settled)                      Closed    (the lane's next is due)
//! ```

use alloc::boxed::Box;
use core::mem;

use temper_lib::bytes::copy_of;
use temper_lib::{Env, Id, Queue, Rng, Slab, Time, Token};

use crate::api::{self, Answer, Body, Error, Kind, Mark, Op, State as Open};
use crate::boundary::{Content, Failure, Item, Position, Record, Request, Write, Written};
use crate::calls::{self, Purpose};
use crate::facts::{Fact, Priority};
use crate::items::{self, copy_labels};
use crate::limits::Limits;
use crate::model::{Alarm, Model};

/// A write in hand.
#[derive(Debug)]
pub(crate) struct Writing {
    owner: Token,
    write: Write,
    lane: Lane,
    /// The next write of its lane, which waits for this one to close.
    next: Option<Id<Writing>>,
    resumed: bool,
    /// Whether what it created was found made by an earlier attempt.
    found: bool,
    /// Attempts that failed and may pass, and whether the last may have been
    /// made.
    attempts: u32,
    ambiguous: bool,
    /// Where a creation an earlier attempt made is looked for: issues updated
    /// since `since`, comments after `from`.
    since: Time,
    from: u64,
    /// A record's: the comment it edits and its revision as last read, or
    /// none to post one; and the inbox position it carries.
    target: Option<Target>,
    position: Position,
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
    /// Terminal: holds nothing.
    Closed,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Phase {
    /// Reading the record afresh before it is edited; or the pull request
    /// after a merge that may have been made.
    Check,
    /// The write itself.
    Make,
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
    resumed: bool,
    out: &mut Queue<Request>,
) {
    if !valid(&write, &env.limits) {
        out.push(Request::Wrote { owner, result: Err(Failure::Invalid) });
        return;
    }
    if model.writes.is_full() {
        out.push(Request::Wrote { owner, result: Err(Failure::Busy) });
        return;
    }
    let (target, position) = match &write {
        Write::Record { item, .. } => match items::record(model, *item) {
            Some((record, position, _)) => (target_of(record), position),
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
        | Write::DeletePage { .. } => (None, Position::START),
    };
    let lane = lane(&write);
    let writing = Writing {
        owner,
        write,
        lane,
        next: None,
        resumed,
        found: false,
        attempts: 0,
        ambiguous: false,
        since: env.now,
        from: 0,
        target,
        position,
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
    let item = subject(&writing.write);
    let from = match item {
        Some(item) => items::passed(model, item),
        None => 0,
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
    let writing = model.writes.get_mut(id).expect("a write lives until it closes");
    writing.since = env.now;
    writing.from = from;
    if let Some((_, position, _)) = known {
        // It carries the position taken as it goes; it aims at the record as
        // it was when it was asked for, or as the writes before it in its lane
        // left it.
        writing.position = position;
    }
    let phase = match &writing.write {
        Write::Record { .. } => match writing.target {
            Some(_) => Phase::Check,
            None if writing.resumed => find(writing),
            None => Phase::Make,
        },
        Write::CreateIssue { .. } | Write::Comment { .. } | Write::OpenPull { .. } => {
            if writing.resumed {
                find(writing)
            } else {
                Phase::Make
            }
        }
        Write::Merge { .. } => {
            if writing.resumed {
                Phase::Check
            } else {
                Phase::Make
            }
        }
        Write::SetLabels { .. }
        | Write::Close { .. }
        | Write::DeleteBranch { .. }
        | Write::PutPage { .. }
        | Write::DeletePage { .. } => Phase::Make,
    };
    writing.state = State::Due { phase };
    ask(model, id);
}

/// The first page of looking for what an earlier attempt made.
fn find(writing: &Writing) -> Phase {
    Phase::Find { since: writing.since, page: 1, after: writing.from }
}

/// A backoff ended: the call is tried again.
pub(crate) fn retry(model: &mut Model, id: Id<Writing>) {
    let writing = model.writes.get_mut(id).expect("an alarm is cancelled as its write closes");
    writing.state = match mem::replace(&mut writing.state, State::Closed) {
        State::Waiting { phase, until: _ } => State::Due { phase },
        State::Queued | State::Due { .. } | State::Busy { .. } | State::Done { .. } | State::Closed => {
            unreachable!("the backoff's alarm runs while it waits")
        }
    };
    ask(model, id);
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
    let writing = model.writes.get_mut(id).expect("a write lives until its call is answered");
    let state = mem::replace(&mut writing.state, State::Closed);
    writing.state = match state {
        State::Busy { phase } => match phase {
            Phase::Check => checked(writing, engine, env, &mut model.rng, result),
            Phase::Make => made(writing, env, &mut model.rng, result),
            Phase::Find { since, page, after } => {
                searched(writing, engine, env, &mut model.rng, since, page, after, result)
            }
        },
        State::Queued | State::Due { .. } | State::Waiting { .. } | State::Done { .. } | State::Closed => {
            unreachable!("a write's terminal comes while its call is out")
        }
    };
    conclude(model, env, id, out);
}

/// What the write `id`'s call asks, as it goes out.
pub(crate) fn op(model: &Model, id: Id<Writing>) -> (u32, Op) {
    let writing = model.writes.get(id).expect("a write lives until its call is answered");
    let phase = match writing.state {
        State::Busy { phase } => phase,
        State::Queued | State::Due { .. } | State::Waiting { .. } | State::Done { .. } | State::Closed => {
            unreachable!("a write's call goes out while it is busy")
        }
    };
    let repository = repository(&writing.write);
    let op = match phase {
        Phase::Check => check_op(writing),
        Phase::Make => make_op(writing),
        Phase::Find { since, page, after } => find_op(writing, since, page, after),
    };
    (repository, op)
}

fn check_op(writing: &Writing) -> Op {
    match &writing.write {
        Write::Record { item, .. } => {
            let target = writing.target.expect("a record is checked when it has one");
            Op::Comment { number: item.number, id: target.comment }
        }
        Write::Merge { item, .. } => Op::Pull { number: item.number, reviews: 0 },
        Write::CreateIssue { .. }
        | Write::Comment { .. }
        | Write::SetLabels { .. }
        | Write::OpenPull { .. }
        | Write::Close { .. }
        | Write::DeleteBranch { .. }
        | Write::PutPage { .. }
        | Write::DeletePage { .. } => unreachable!("only a record and a merge are checked"),
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
            let body = Body::Record { payload: *payload, position: writing.position };
            match writing.target {
                Some(target) => Op::EditComment { number: item.number, id: target.comment, body },
                None => Op::Post { number: item.number, key: None, body },
            }
        }
        Write::SetLabels { item, labels } => Op::SetLabels { number: item.number, labels: copy_labels(labels) },
        Write::OpenPull { repository: _, title, body, head, base } => {
            Op::OpenPull { title: copy_of(title), body: body_of(body), head: copy_of(head), base: copy_of(base) }
        }
        Write::Merge { item, head } => Op::Merge { number: item.number, head: *head },
        Write::Close { item } => Op::Close { number: item.number },
        Write::DeleteBranch { repository: _, branch } => Op::DeleteBranch { branch: copy_of(branch) },
        Write::PutPage { repository: _, name, content } => {
            Op::PutPage { name: copy_of(name), content: body_of(content) }
        }
        Write::DeletePage { repository: _, name } => Op::DeletePage { name: copy_of(name) },
    }
}

fn find_op(writing: &Writing, since: Time, page: u32, after: u64) -> Op {
    match &writing.write {
        Write::CreateIssue { .. } => Op::Items { state: None, kind: Some(Kind::Issue), label: None, since, page },
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

/// Check, read: a record as last read is edited, one changed is not; a merge
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
        Err(Error::Missing) => match writing.write {
            Write::Record { .. } => {
                let record = Record::Missing;
                return State::Done { result: Err(Failure::Edited { record }), record: Some(record) };
            }
            Write::CreateIssue { .. }
            | Write::Comment { .. }
            | Write::SetLabels { .. }
            | Write::OpenPull { .. }
            | Write::Merge { .. }
            | Write::Close { .. }
            | Write::DeleteBranch { .. }
            | Write::PutPage { .. }
            | Write::DeletePage { .. } => return refused(Error::Missing),
        },
        Err(error) => return failed(writing, env, rng, Phase::Check, error),
    };
    match &writing.write {
        Write::Record { .. } => {
            let comment = api::comment(answer);
            let target = writing.target.expect("a record is checked when it has one");
            if comment.revision == target.revision {
                return State::Due { phase: Phase::Make };
            }
            let record = match comment.mark {
                Mark::Record(position) if comment.author == engine => {
                    Record::Found { comment: comment.id, revision: comment.revision, position }
                }
                Mark::Record(_) | Mark::Mangled | Mark::None | Mark::Key(_) => {
                    Record::Mangled { comment: comment.id, revision: comment.revision }
                }
            };
            State::Done { result: Err(Failure::Edited { record }), record: Some(record) }
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
        | Write::PutPage { .. }
        | Write::DeletePage { .. } => unreachable!("only a record and a merge are checked"),
    }
}

/// Make, answered: done; or what an attempt that may have been made, or a
/// refusal, calls for.
fn made(writing: &mut Writing, env: &Env<Limits>, rng: &mut Rng, result: Result<Answer, Error>) -> State {
    let error = match result {
        Ok(answer) => return done(writing, answer),
        Err(error) => error,
    };
    match error {
        Error::Timeout => {
            writing.ambiguous = true;
            let phase = match &writing.write {
                Write::CreateIssue { .. } | Write::Comment { .. } | Write::OpenPull { .. } => find(writing),
                Write::Record { .. } => match writing.target {
                    Some(_) => Phase::Make,
                    None => find(writing),
                },
                Write::Merge { .. } => Phase::Check,
                Write::SetLabels { .. }
                | Write::Close { .. }
                | Write::DeleteBranch { .. }
                | Write::PutPage { .. }
                | Write::DeletePage { .. } => Phase::Make,
            };
            failed(writing, env, rng, phase, error)
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
        | Error::Protected => failed(writing, env, rng, Phase::Make, error),
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
    match &writing.write {
        Write::CreateIssue { key, .. } => {
            let (items, more) = api::items(answer);
            for summary in &items {
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
            let mut last = after;
            for comment in &comments {
                last = comment.id;
                let keyed = match &comment.mark {
                    Mark::Key(found) => **found == **key,
                    Mark::None | Mark::Record(_) | Mark::Mangled => false,
                };
                if keyed && comment.author == engine {
                    writing.found = true;
                    return State::Done { result: Ok(Written::Commented(comment.id)), record: None };
                }
            }
            if more {
                State::Due { phase: Phase::Find { since, page, after: last } }
            } else {
                State::Due { phase: Phase::Make }
            }
        }
        Write::Record { .. } => {
            let (_, comments, more) = api::item(answer);
            let mut last = after;
            for comment in &comments {
                last = comment.id;
                if comment.author != engine {
                    continue;
                }
                let record = match comment.mark {
                    Mark::Record(position) => {
                        Record::Found { comment: comment.id, revision: comment.revision, position }
                    }
                    Mark::Mangled => Record::Mangled { comment: comment.id, revision: comment.revision },
                    Mark::None | Mark::Key(_) => continue,
                };
                writing.found = true;
                return State::Done { result: Ok(Written::Done), record: Some(record) };
            }
            if more {
                State::Due { phase: Phase::Find { since, page, after: last } }
            } else {
                State::Due { phase: Phase::Make }
            }
        }
        Write::OpenPull { .. } => {
            let pull = api::pull(answer);
            if pull.state == Open::Open {
                writing.found = true;
                State::Done { result: Ok(Written::Created(pull.number)), record: None }
            } else {
                State::Due { phase: Phase::Make }
            }
        }
        Write::SetLabels { .. }
        | Write::Merge { .. }
        | Write::Close { .. }
        | Write::DeleteBranch { .. }
        | Write::PutPage { .. }
        | Write::DeletePage { .. } => unreachable!("only creations are looked for"),
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
        | Answer::Statuses(_)
        | Answer::Permission(_)
        | Answer::Commit(_)
        | Answer::Pages { .. }
        | Answer::Page(_) => unreachable!("a write is answered as one"),
    };
    State::Done { result: Ok(written), record }
}

/// A call failed: queued again at once for the rate, whose reset holds every
/// call; tried again after a backoff if it may pass, until the attempts run
/// out; the answer otherwise.
fn failed(writing: &mut Writing, env: &Env<Limits>, rng: &mut Rng, phase: Phase, error: Error) -> State {
    match error {
        Error::RateLimited { .. } => State::Due { phase },
        Error::Unavailable | Error::Timeout => {
            writing.attempts = writing.attempts.saturating_add(1);
            if writing.attempts >= env.limits.attempts {
                return refused(error);
            }
            let until = env.now.saturating_add(items::backoff(&env.limits, rng, writing.attempts));
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
/// answers the parent, and closes, and the next of its lane starts; then the
/// backoff's alarm runs only while it waits.
fn conclude(model: &mut Model, env: &Env<Limits>, id: Id<Writing>, out: &mut Queue<Request>) {
    let writing = model.writes.get_mut(id).expect("a write lives until it closes");
    match mem::replace(&mut writing.state, State::Closed) {
        State::Due { phase } => {
            writing.state = State::Due { phase };
            ask(model, id);
        }
        State::Done { result, record } => {
            let owner = writing.owner;
            if writing.found {
                model.facts.push(Fact::Found { owner });
            }
            if let Some(record) = record {
                let item = subject(&writing.write).expect("a record is about an item");
                let next = writing.next;
                items::recorded(model, item, record);
                if result.is_ok() {
                    follow_record(&mut model.writes, next, record, env.limits.writes);
                }
                match result {
                    Err(Failure::Edited { .. }) => model.facts.push(Fact::Edited { item }),
                    Ok(_) | Err(Failure::Busy | Failure::Invalid | Failure::Unknown | Failure::Forge(_)) => {}
                }
            }
            match result {
                Ok(_) => model.facts.push(Fact::Wrote { owner }),
                Err(_) => model.facts.push(Fact::Unwritten { owner }),
            }
            out.push(Request::Wrote { owner, result });
            close(model, env, id);
        }
        State::Waiting { phase, until } => {
            writing.state = State::Waiting { phase, until };
            model.facts.push(Fact::Retried { owner: writing.owner });
            model.alarms.arm(Alarm::Write(id), until).expect("an alarm per write fits");
        }
        State::Queued => writing.state = State::Queued,
        State::Busy { phase } => writing.state = State::Busy { phase },
        State::Closed => unreachable!("a write is concluded while it lives"),
    }
}

/// Queues the call a write is due.
fn ask(model: &mut Model, id: Id<Writing>) {
    let writing = model.writes.get_mut(id).expect("a write lives until it closes");
    let phase = match writing.state {
        State::Due { phase } => phase,
        State::Queued | State::Busy { .. } | State::Waiting { .. } | State::Done { .. } | State::Closed => {
            unreachable!("a write asks for a call when one is due")
        }
    };
    let priority = match phase {
        Phase::Check | Phase::Find { .. } => Priority::Fresh,
        Phase::Make => Priority::Write,
    };
    calls::queue(&mut model.calls, Purpose::Write(id), priority);
    writing.state = State::Busy { phase };
    model.alarms.cancel(Alarm::Write(id));
}

/// The write `id` is done: it is retired, and the next of its lane starts.
fn close(model: &mut Model, env: &Env<Limits>, id: Id<Writing>) {
    let writing = model.writes.get_mut(id).expect("a write lives until it closes");
    let (lane, next) = (writing.lane, writing.next);
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
/// record writes queued after it in its lane aim at it. A record found
/// changed by someone else is not passed on: the writes asked for before the
/// parent heard of it fail as this one did.
fn follow_record(writes: &mut Slab<Writing>, next: Option<Id<Writing>>, record: Record, most: u32) {
    let mut next = next;
    for _ in 0..most {
        let Some(id) = next else {
            return;
        };
        let writing = writes.get_mut(id).expect("a lane's writes live until they close");
        match writing.write {
            Write::Record { .. } => writing.target = target_of(record),
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
/// repositories.
fn valid(write: &Write, limits: &Limits) -> bool {
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
        Write::SetLabels { item: _, labels } => labelled(labels, limits),
        Write::OpenPull { repository: _, title, body, head, base } => {
            fits(title, limits.title_bytes) && content(body, limits) && fits(head, name) && fits(base, name)
        }
        Write::DeleteBranch { repository: _, branch } => fits(branch, name),
        Write::PutPage { repository: _, name: page, content: body } => fits(page, name) && content(body, limits),
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
