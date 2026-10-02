//! The working set (engine-model.md, section 12): the items the engine
//! tracks that are not done, and what its decisions need of each. An item
//! enters when the parent takes it in or a listing finds it carrying the
//! tracking label, and leaves when it closes or the parent stops tracking it;
//! a full working set refuses it at the entrance, and it waits on the forge
//! until there is room ([`crate::scans`]).
//!
//! Entering, an item is read from its first comment until its record is
//! found: the engine's own comment carrying a record block, or one that does
//! not decode (then the item is held for a person), or none. It is then
//! announced to the parent, and its inbox (4.3) is derived from the inbox
//! position its record names: the comments after it that are not the
//! engine's own, the reviews of its pull request after those taken, and its
//! pull request's head, CI and state when they differ from what was taken.
//! The inbox holds `Limits::inbox` news; what does not fit waits on the forge
//! until the parent takes some ([`took`]), as nothing of it is queued but
//! where to read from.
//!
//! An item is read again when a listing shows it changed: its comments for a
//! new updated time, and, for a pull request, the pull request too; its
//! linked pull request when a listing shows that changed, or a status webhook
//! names its head, or its CI has not settled when its repository is polled.
//! A listing that shows an item at the updated time it last showed, the
//! newest the listing saw, means it again only once, in a later pass: the
//! forge's times are a second apart at best, so a change may have come in the
//! same second as the read before ([`changed`]).
//!
//! The transition table. A read is one call; `Due` is the state that asks
//! for one, which [`conclude`] queues at once, so it never outlasts a step.
//!
//! ```text
//! state    event                          next     requests
//! (none)   track, room                    Finding  (a read queued)
//!          track, full                    (none)   full
//! Finding  read, no record yet, more      Finding
//!          read, record found or mangled  Reading  announced, the page's news
//!            or not, its comments here       or Idle
//!          read, its comments before it   Reading  announced
//!          read, closed; missing          (gone)   left
//! Reading  read, more and room            Reading  news, changed
//!          read, otherwise                Idle     news, changed
//!          read, closed; missing          (gone)   left
//! Pulling  read                           Idle     news
//! Busy     failed, rate                   Busy     (queued again)
//!          failed, otherwise              Waiting
//!          untracked, listed closed       (left)   left, when listed closed
//! Waiting  alarm                          Busy
//! Idle     stale, room for news           Busy     (a read queued)
//! (left)   read                           Closed
//! ```
//!
//! "(gone)" and "(left)": the item leaves the working set at once (its name is
//! free to enter again), and its entry is retired once nothing it asked for
//! is out: at once from Idle, Due and Waiting, and from Busy once its read
//! answers, which is then dropped.

use alloc::boxed::Box;
use core::mem;

use temper_lib::bytes::copy_of;
use temper_lib::{Duration, Env, Id, List, Queue, Rng, Slab, Time};

use crate::api::{self, Answer, Check, Comment, Error, Kind, Mark, Op, Pull, State as Open, Status, Summary};
use crate::boundary::{Ci, Item, Level, News, Position, Record, Request, View};
use crate::calls::{self, Calls, Purpose};
use crate::facts::{Fact, Priority};
use crate::limits::Limits;
use crate::model::{Alarm, Model};
use crate::scans;

#[derive(Debug)]
pub(crate) struct Entry {
    item: Item,
    /// Known from its first read.
    kind: Option<Kind>,
    labels: Box<[Box<[u8]>]>,
    /// Its updated time as listings show it, and its linked pull request's.
    seen: Seen,
    pull_seen: Seen,
    /// The pull request carrying its change, or itself if it is one; and
    /// that pull request as last read, and as last told.
    pull: Option<u64>,
    level: Option<Level>,
    told: Told,
    /// Known from its first read; and the nonce of a record write of the
    /// engine's that may or may not have landed, whose revision is not known.
    record: Option<Record>,
    uncertain: Option<u64>,
    /// The inbox position the parent took, which the next record written
    /// carries, and the one the news held reach.
    taken: Position,
    announced: Position,
    inbox: Queue<Held>,
    /// The last news told.
    seq: u64,
    stale: Stale,
    /// It has left the working set, and is retired once nothing is out.
    left: bool,
    state: State,
}

/// News held until the parent takes it, and the position past it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) struct Held {
    seq: u64,
    news: News,
    position: Position,
}

/// What listings showed of an item: its updated time, whether a later pass
/// confirmed it, and the pass that saw it change.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) struct Seen {
    updated: Time,
    settled: bool,
    pass: u64,
}

/// The state of a pull request last told, besides its head and CI, which the
/// position says.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
struct Told {
    open: bool,
    merged: Option<[u8; 32]>,
    mergeable: bool,
}

/// An item and a pull request as they are when nothing is said of them.
const QUIET: Told = Told { open: true, merged: None, mergeable: true };

/// What is owed a read.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
struct Stale {
    comments: bool,
    pull: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum State {
    /// A read is to be queued.
    Due {
        phase: Phase,
        attempt: u32,
    },
    /// A read is queued or out.
    Busy {
        phase: Phase,
        call: Id<calls::Call>,
        attempt: u32,
    },
    /// The last read failed: tried again at `until`.
    Waiting {
        phase: Phase,
        attempt: u32,
        until: Time,
    },
    Idle,
    /// It is not on the forge as the working set holds items: closed, or
    /// gone. It leaves.
    Gone,
    /// Terminal: holds nothing.
    Closed,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Phase {
    /// Its comments after `after`, for its record.
    Finding { after: u64 },
    /// Its comments after the last one passed.
    Reading,
    /// The pull request `number`.
    Pulling { number: u64 },
}

/// The pull request of `entry`, as last read.
pub(crate) fn level(entry: &Entry) -> Option<Level> {
    entry.level
}

/// Takes `item` in, if there is room; refuses it otherwise, and it waits on
/// the forge.
pub(crate) fn track(model: &mut Model, env: &Env<Limits>, item: Item, out: &mut Queue<Request>) {
    assert!(item.repository < env.limits.repositories, "the parent names the deployment's repositories");
    if model.index.contains_key(&item) {
        return;
    }
    if admit(model, env, item, Time::ZERO).is_none() {
        model.facts.push(Fact::Refused { item });
        scans::wait(model, item.repository);
        out.push(Request::Full { item });
    }
}

/// Admits `item` into the working set, to be read for its record, unless it
/// is full; returns its entry. `updated` is its updated time as the listing
/// that found it showed it, if one did.
pub(crate) fn admit(model: &mut Model, env: &Env<Limits>, item: Item, updated: Time) -> Option<Id<Entry>> {
    if let Some(id) = model.index.get(&item) {
        return Some(*id);
    }
    if model.entries.is_full() {
        return None;
    }
    let pass = scans::passes(model, item.repository);
    let seen = Seen { updated, settled: false, pass };
    let entry = Entry {
        item,
        kind: None,
        labels: Box::new([]),
        seen,
        pull_seen: Seen { updated: Time::ZERO, settled: false, pass },
        pull: None,
        level: None,
        told: QUIET,
        record: None,
        uncertain: None,
        taken: Position::START,
        announced: Position::START,
        inbox: Queue::with_capacity(env.limits.inbox),
        seq: 0,
        stale: Stale { comments: false, pull: false },
        left: false,
        state: State::Due { phase: Phase::Finding { after: 0 }, attempt: 0 },
    };
    let id = model.entries.insert(entry).expect("checked for room above");
    model.index.insert(item, id).expect("an index as large as the working set");
    model.loading.finding = model.loading.finding.saturating_add(1);
    model.facts.push(Fact::Admitted { item });
    kick(&mut model.entries, &mut model.calls, id);
    Some(id)
}

/// The parent stops tracking `item`: it leaves, and nothing is told of it.
pub(crate) fn untrack(model: &mut Model, env: &Env<Limits>, item: Item) {
    let Some(&id) = model.index.get(&item) else {
        return;
    };
    leave(model, id);
    follow(model, env, id);
}

/// `item`'s change is carried by the pull request `pull`, or none.
pub(crate) fn link(model: &mut Model, env: &Env<Limits>, item: Item, pull: Option<u64>) {
    let Some(&id) = model.index.get(&item) else {
        return;
    };
    let pass = scans::passes(model, item.repository);
    let entry = model.entries.get_mut(id).expect("the index names live entries");
    match entry.kind {
        Some(Kind::Pull) => return,
        Some(Kind::Issue) | None => {}
    }
    let old = entry.pull;
    if old != pull {
        if let Some(number) = old {
            let name = Item { repository: item.repository, number };
            if model.pulls.get(&name) == Some(&id) {
                model.pulls.remove(&name);
            }
            // Another pull request: what was taken of the old one says
            // nothing of it.
            entry.level = None;
            entry.told = QUIET;
            let unlinked = Position { reviews: 0, head: None, ci: Ci::None, ..entry.taken };
            entry.taken = unlinked;
            entry.announced = Position { comment: entry.announced.comment, ..unlinked };
        }
        if let Some(number) = pull {
            let name = Item { repository: item.repository, number };
            model.pulls.insert(name, id).expect("a pull request per item held");
        }
        entry.pull = pull;
        entry.pull_seen = Seen { updated: Time::ZERO, settled: false, pass };
    }
    entry.stale.pull = pull.is_some();
    kick(&mut model.entries, &mut model.calls, id);
    follow(model, env, id);
}

/// The parent took `item`'s news through `through`: the position moves past
/// them, and room frees for more.
pub(crate) fn took(model: &mut Model, env: &Env<Limits>, item: Item, through: u64) {
    let Some(&id) = model.index.get(&item) else {
        return;
    };
    let entry = model.entries.get_mut(id).expect("the index names live entries");
    for _ in 0..entry.inbox.len() {
        let Some(held) = entry.inbox.iter().next() else {
            break;
        };
        if held.seq > through {
            break;
        }
        let held = entry.inbox.pop().expect("looked at above");
        entry.taken = held.position;
    }
    kick(&mut model.entries, &mut model.calls, id);
    follow(model, env, id);
}

/// A listing shows `summary`, an item of the working set, in the `pass`th
/// pass of its repository.
pub(crate) fn listed(
    model: &mut Model,
    env: &Env<Limits>,
    id: Id<Entry>,
    summary: &Summary,
    pass: u64,
    out: &mut Queue<Request>,
) {
    let entry = model.entries.get_mut(id).expect("the index names live entries");
    if summary.state == Open::Closed {
        let item = entry.item;
        leave(model, id);
        out.push(Request::Left { item });
    } else {
        absorb(entry, &env.limits, summary, out);
        if changed(&mut entry.seen, summary.updated, pass) {
            entry.stale.comments = true;
            if entry.kind == Some(Kind::Pull) {
                entry.stale.pull = true;
            }
        }
        kick(&mut model.entries, &mut model.calls, id);
    }
    follow(model, env, id);
}

/// A listing shows `summary`, the pull request linked to the item `id`.
pub(crate) fn linked(model: &mut Model, env: &Env<Limits>, id: Id<Entry>, summary: &Summary, pass: u64) {
    let entry = model.entries.get_mut(id).expect("the index names live entries");
    if changed(&mut entry.pull_seen, summary.updated, pass) {
        entry.stale.pull = true;
    }
    kick(&mut model.entries, &mut model.calls, id);
    follow(model, env, id);
}

/// Its repository is polled: the pull requests of its items whose CI has not
/// settled are read again, as statuses move no updated time. Or a status
/// webhook named `commit`: the pull requests on that head are.
pub(crate) fn poll_ci(model: &mut Model, repository: u32, commit: Option<[u8; 32]>) {
    for (item, id) in &model.index {
        if item.repository != repository {
            continue;
        }
        let entry = model.entries.get_mut(*id).expect("the index names live entries");
        if entry.pull.is_none() {
            continue;
        }
        let wanted = match commit {
            Some(commit) => match entry.level {
                Some(level) => level.commit == commit,
                None => false,
            },
            None => match entry.level {
                Some(level) => level.open && unsettled(level.ci),
                None => true,
            },
        };
        if wanted {
            entry.stale.pull = true;
            kick(&mut model.entries, &mut model.calls, *id);
        }
    }
}

fn unsettled(ci: Ci) -> bool {
    match ci {
        Ci::None | Ci::Pending => true,
        Ci::Passed | Ci::Failed => false,
    }
}

/// A backoff ended: the read is tried again.
pub(crate) fn retry(model: &mut Model, env: &Env<Limits>, id: Id<Entry>) {
    let entry = model.entries.get_mut(id).expect("an alarm is cancelled as its entry is retired");
    entry.state = match entry.state {
        State::Waiting { phase, attempt, until: _ } => State::Due { phase, attempt },
        State::Due { .. } | State::Busy { .. } | State::Idle | State::Gone | State::Closed => {
            unreachable!("the backoff's alarm runs while it waits")
        }
    };
    kick(&mut model.entries, &mut model.calls, id);
    follow(model, env, id);
}

/// Terminal for the read of the item `id`.
pub(crate) fn answered(
    model: &mut Model,
    env: &Env<Limits>,
    id: Id<Entry>,
    result: Result<Answer, Error>,
    out: &mut Queue<Request>,
) {
    let engine = model.config.engine;
    let entry = model.entries.get_mut(id).expect("an entry lives until its read is answered");
    let finding = entry.record.is_none();
    let state = mem::replace(&mut entry.state, State::Closed);
    entry.state = match state {
        State::Busy { phase, call: _, attempt } => {
            if entry.left {
                State::Closed
            } else {
                match phase {
                    Phase::Finding { after } => found(entry, engine, env, &mut model.rng, after, attempt, result, out),
                    Phase::Reading => read(entry, engine, env, &mut model.rng, attempt, result, out),
                    Phase::Pulling { number } => pulled(entry, env, &mut model.rng, number, attempt, result, out),
                }
            }
        }
        State::Due { .. } | State::Waiting { .. } | State::Idle | State::Gone | State::Closed => {
            unreachable!("a read's terminal comes while it is out")
        }
    };
    if finding && entry.record.is_some() {
        model.loading.finding = model.loading.finding.saturating_sub(1);
    }
    conclude(model, env, id, out);
}

/// What a call for the item `id` asks, as it goes out.
pub(crate) fn op(model: &Model, id: Id<Entry>) -> (u32, Op) {
    let entry = model.entries.get(id).expect("an entry lives until its read is answered");
    let number = entry.item.number;
    let op = match entry.state {
        State::Busy { phase, .. } => match phase {
            Phase::Finding { after } => Op::Item { number, after },
            Phase::Reading => Op::Item { number, after: entry.announced.comment },
            Phase::Pulling { number } => Op::Pull { number, reviews: entry.announced.reviews },
        },
        State::Due { .. } | State::Waiting { .. } | State::Idle | State::Gone | State::Closed => {
            unreachable!("an entry's call goes out while it is busy")
        }
    };
    (entry.item.repository, op)
}

// Cell handlers: each takes the source state's data by value and returns the
// target state.

/// Finding, read: the record, if this page has it.
#[expect(clippy::too_many_arguments, reason = "a cell handler over the fields it touches")]
fn found(
    entry: &mut Entry,
    engine: u64,
    env: &Env<Limits>,
    rng: &mut Rng,
    after: u64,
    attempt: u32,
    result: Result<Answer, Error>,
    out: &mut Queue<Request>,
) -> State {
    let (summary, comments, more) = match result {
        Ok(answer) => api::item(answer),
        Err(error) => return failed(env, rng, Phase::Finding { after }, attempt, error),
    };
    assert!(summary.number == entry.item.number, "an item's read is answered with that item");
    assert!(comments.len() <= page(&env.limits), "the protocol layer brings a page at most");
    if summary.state == Open::Closed {
        return State::Gone;
    }
    entry.kind = Some(summary.kind);
    absorb(entry, &env.limits, &summary, out);
    let mut record = None;
    let mut last = after;
    for comment in &comments {
        last = comment.id;
        if comment.author != engine {
            continue;
        }
        match comment.mark {
            Mark::Record { position, nonce: _ } => {
                record = Some(Record::Found { comment: comment.id, revision: comment.revision, position });
                break;
            }
            Mark::Mangled => {
                record = Some(Record::Mangled { comment: comment.id, revision: comment.revision });
                break;
            }
            Mark::None | Mark::Key(_) => {}
        }
    }
    let record = match record {
        Some(record) => record,
        None if more => return State::Due { phase: Phase::Finding { after: last }, attempt: 0 },
        None => Record::Missing,
    };
    let position = match record {
        Record::Found { position, .. } => position,
        Record::Mangled { comment, .. } => Position { comment, ..Position::START },
        Record::Missing => Position::START,
    };
    entry.record = Some(record);
    entry.taken = position;
    entry.announced = position;
    if summary.kind == Kind::Pull {
        entry.pull = Some(entry.item.number);
        entry.stale.pull = true;
    }
    let view = View { kind: summary.kind, labels: copy_labels(&entry.labels), record };
    out.push(Request::Announced { item: entry.item, view });
    if position.comment < after {
        // Comments before this page come after the position: read them.
        return State::Due { phase: Phase::Reading, attempt: 0 };
    }
    after_comments(entry, engine, &comments, more, out)
}

/// Reading, read: the page's news.
fn read(
    entry: &mut Entry,
    engine: u64,
    env: &Env<Limits>,
    rng: &mut Rng,
    attempt: u32,
    result: Result<Answer, Error>,
    out: &mut Queue<Request>,
) -> State {
    let (summary, comments, more) = match result {
        Ok(answer) => api::item(answer),
        Err(error) => return failed(env, rng, Phase::Reading, attempt, error),
    };
    assert!(summary.number == entry.item.number, "an item's read is answered with that item");
    assert!(comments.len() <= page(&env.limits), "the protocol layer brings a page at most");
    if summary.state == Open::Closed {
        return State::Gone;
    }
    absorb(entry, &env.limits, &summary, out);
    after_comments(entry, engine, &comments, more, out)
}

/// The news of a page of comments, and what follows it: the next page, or
/// nothing until there is room.
fn after_comments(entry: &mut Entry, engine: u64, comments: &[Comment], more: bool, out: &mut Queue<Request>) -> State {
    let stopped = comment_news(entry, engine, comments, out);
    if stopped || (more && entry.inbox.room() == 0) {
        entry.stale.comments = true;
        return State::Idle;
    }
    if more {
        return State::Due { phase: Phase::Reading, attempt: 0 };
    }
    State::Idle
}

/// Pulling, read: the reviews after those taken, and the pull request's head,
/// CI and state if they moved.
fn pulled(
    entry: &mut Entry,
    env: &Env<Limits>,
    rng: &mut Rng,
    number: u64,
    attempt: u32,
    result: Result<Answer, Error>,
    out: &mut Queue<Request>,
) -> State {
    let pull = match result {
        Ok(answer) => api::pull(answer),
        Err(error) => {
            let state = failed(env, rng, Phase::Pulling { number }, attempt, error);
            if state == State::Gone {
                // No such pull request: nothing to tell of it.
                if entry.pull == Some(number) {
                    entry.level = None;
                }
                return State::Idle;
            }
            return state;
        }
    };
    if entry.pull != Some(number) {
        // Linked to another since: the read says nothing of it.
        return State::Idle;
    }
    assert!(pull.reviews.len() <= page(&env.limits), "the protocol layer brings a page at most");
    let level = Level {
        number,
        commit: pull.commit,
        ci: ci(&pull.statuses),
        open: pull.state == Open::Open,
        merged: pull.merged,
        mergeable: pull.mergeable,
    };
    entry.level = Some(level);
    if review_news(entry, &pull, out) {
        entry.stale.pull = true;
        return State::Idle;
    }
    if pull.more {
        // More reviews than a page: read on, the state told with the last.
        return match entry.inbox.room() {
            0 => {
                entry.stale.pull = true;
                State::Idle
            }
            _ => State::Due { phase: Phase::Pulling { number }, attempt: 0 },
        };
    }
    let told = Told { open: level.open, merged: level.merged, mergeable: level.mergeable };
    let quiet = entry.announced.head == Some(level.commit) && entry.announced.ci == level.ci && entry.told == told;
    if quiet {
        return State::Idle;
    }
    if entry.inbox.room() == 0 {
        entry.stale.pull = true;
        return State::Idle;
    }
    entry.told = told;
    let position = Position { head: Some(level.commit), ci: level.ci, ..entry.announced };
    let news = News::Pull {
        commit: level.commit,
        ci: level.ci,
        open: level.open,
        merged: level.merged,
        mergeable: level.mergeable,
    };
    tell(entry, news, position, out);
    State::Idle
}

/// A read failed: queued again at once for the rate, whose reset holds every
/// call; the item gone if the forge has it no more; otherwise tried again
/// after a backoff.
fn failed(env: &Env<Limits>, rng: &mut Rng, phase: Phase, attempt: u32, error: Error) -> State {
    match error {
        Error::RateLimited { .. } => State::Due { phase, attempt },
        Error::Missing | Error::Forbidden => State::Gone,
        Error::Unavailable
        | Error::Timeout
        | Error::TooLarge
        | Error::Empty
        | Error::Full
        | Error::Exists
        | Error::NothingToMerge
        | Error::Closed
        | Error::Stale
        | Error::Conflict
        | Error::Protected => {
            let attempt = attempt.saturating_add(1);
            let until = env.now.saturating_add(backoff(&env.limits, rng, attempt));
            State::Waiting { phase, attempt, until }
        }
    }
}

/// The comments of a page that are news, told while the inbox has room; the
/// engine's own are passed over. Says whether it stopped for room.
fn comment_news(entry: &mut Entry, engine: u64, comments: &[Comment], out: &mut Queue<Request>) -> bool {
    for comment in comments {
        if comment.id <= entry.announced.comment {
            continue;
        }
        if comment.author == engine {
            entry.announced.comment = comment.id;
            continue;
        }
        if entry.inbox.room() == 0 {
            return true;
        }
        let position = Position { comment: comment.id, ..entry.announced };
        tell(entry, News::Comment { id: comment.id, author: comment.author }, position, out);
    }
    false
}

/// The reviews of `pull`, a page of those after the ones it was read after,
/// told while the inbox has room. Says whether it stopped for room.
fn review_news(entry: &mut Entry, pull: &Pull, out: &mut Queue<Request>) -> bool {
    for review in &pull.reviews {
        let index = entry.announced.reviews.saturating_add(1);
        if entry.inbox.room() == 0 {
            return true;
        }
        let position = Position { reviews: index, ..entry.announced };
        let news = News::Review { author: review.author, verdict: review.verdict, commit: review.commit };
        tell(entry, news, position, out);
    }
    false
}

/// Holds `news`, which takes the inbox to `position`, and tells it.
fn tell(entry: &mut Entry, news: News, position: Position, out: &mut Queue<Request>) {
    entry.seq = entry.seq.saturating_add(1);
    let held = Held { seq: entry.seq, news, position };
    entry.inbox.try_push(held).expect("news is told only while there is room");
    entry.announced = position;
    out.push(Request::Inbox { item: entry.item, seq: entry.seq, news });
}

/// Takes what `summary` says of the entry's labels, telling the parent of a
/// change once the item is announced.
fn absorb(entry: &mut Entry, limits: &Limits, summary: &Summary, out: &mut Queue<Request>) {
    if *entry.labels == *summary.labels {
        return;
    }
    let most = usize::try_from(limits.labels).expect("a u32 fits in a usize");
    assert!(summary.labels.len() <= most, "the protocol layer brings the labels the limits hold");
    // The old labels go before the new are kept.
    entry.labels = Box::new([]);
    entry.labels = copy_labels(&summary.labels);
    if entry.record.is_some() {
        out.push(Request::Changed { item: entry.item, labels: copy_labels(&summary.labels) });
    }
}

/// Whether a listing that shows `updated` in the `pass`th pass means news:
/// a newer time does; the same time does once more, in a later pass than the
/// one that found it, as the forge's times are coarse and a change may have
/// come after the read in the same second.
pub(crate) fn changed(seen: &mut Seen, updated: Time, pass: u64) -> bool {
    if updated > seen.updated {
        *seen = Seen { updated, settled: false, pass };
        return true;
    }
    if updated == seen.updated && !seen.settled && pass > seen.pass {
        seen.settled = true;
        return true;
    }
    false
}

/// CI on a head, over its contexts.
fn ci(statuses: &[Status]) -> Ci {
    let mut ci = Ci::None;
    for status in statuses {
        ci = match status.check {
            Check::Failed => Ci::Failed,
            Check::Pending => match ci {
                Ci::Failed => Ci::Failed,
                Ci::None | Ci::Pending | Ci::Passed => Ci::Pending,
            },
            Check::Passed => match ci {
                Ci::None | Ci::Passed => Ci::Passed,
                Ci::Pending => Ci::Pending,
                Ci::Failed => Ci::Failed,
            },
        };
    }
    ci
}

fn page(limits: &Limits) -> usize {
    usize::try_from(limits.page).expect("a u32 fits in a usize")
}

pub(crate) fn copy_labels(labels: &[Box<[u8]>]) -> Box<[Box<[u8]>]> {
    let count = u32::try_from(labels.len()).expect("labels within the limits");
    let mut copy = List::with_capacity(count);
    for label in labels {
        copy.push(copy_of(label)).expect("a list as long as the labels");
    }
    copy.into_boxed()
}

/// A backoff for the `attempt`th retry: drawn between `Limits::backoff`
/// doubled for each attempt before it and twice that, at most
/// `Limits::backoff_max`.
pub(crate) fn backoff(limits: &Limits, rng: &mut Rng, attempt: u32) -> Duration {
    let doublings = attempt.saturating_sub(1).min(32);
    let base = limits.backoff.saturating_mul(2_u64.checked_pow(doublings).unwrap_or(u64::MAX));
    let max = limits.backoff_max.as_nanos();
    let low = base.as_nanos().min(max);
    let high = low.saturating_mul(2).min(max);
    Duration::from_nanos(rng.between(low, high))
}

/// The item `id` leaves the working set: its name is free, and it is retired
/// once nothing it asked for is out.
fn leave(model: &mut Model, id: Id<Entry>) {
    let entry = model.entries.get_mut(id).expect("an entry leaves once");
    let item = entry.item;
    if model.index.get(&item) == Some(&id) {
        model.index.remove(&item);
    }
    if let Some(number) = entry.pull {
        let name = Item { repository: item.repository, number };
        if model.pulls.get(&name) == Some(&id) {
            model.pulls.remove(&name);
        }
    }
    if entry.record.is_none() {
        model.loading.finding = model.loading.finding.saturating_sub(1);
    }
    entry.left = true;
    entry.state = match entry.state {
        State::Busy { phase, call, attempt } => State::Busy { phase, call, attempt },
        State::Due { .. } | State::Waiting { .. } | State::Idle | State::Gone => State::Closed,
        State::Closed => unreachable!("an entry leaves once"),
    };
    model.facts.push(Fact::Left { item });
}

/// Applied after every transition: an item gone leaves, and the parent is
/// told; an idle item owed a read gets one, if its inbox has room for what it
/// may find; then what the state implies ([`follow`]).
fn conclude(model: &mut Model, env: &Env<Limits>, id: Id<Entry>, out: &mut Queue<Request>) {
    let entry = model.entries.get(id).expect("an entry lives until it is retired");
    let item = entry.item;
    match entry.state {
        State::Gone => {
            leave(model, id);
            out.push(Request::Left { item });
        }
        State::Due { .. } | State::Busy { .. } | State::Waiting { .. } | State::Idle | State::Closed => {}
    }
    kick(&mut model.entries, &mut model.calls, id);
    follow(model, env, id);
}

/// Queues the read an entry is due, or owed while it is idle.
fn kick(entries: &mut Slab<Entry>, calls: &mut Calls, id: Id<Entry>) {
    let entry = entries.get_mut(id).expect("an entry lives until it is retired");
    let room = entry.inbox.room() > 0;
    let (phase, attempt) = match entry.state {
        State::Due { phase, attempt } => (phase, attempt),
        State::Idle => {
            if entry.stale.comments && room {
                (Phase::Reading, 0)
            } else if entry.stale.pull && room {
                match entry.pull {
                    Some(number) => (Phase::Pulling { number }, 0),
                    None => {
                        entry.stale.pull = false;
                        return;
                    }
                }
            } else {
                return;
            }
        }
        State::Busy { .. } | State::Waiting { .. } | State::Gone | State::Closed => return,
    };
    match phase {
        Phase::Reading => entry.stale.comments = false,
        Phase::Pulling { .. } => entry.stale.pull = false,
        Phase::Finding { .. } => {}
    }
    let call = calls::queue(calls, Purpose::Item(id), Priority::Keep);
    entry.state = State::Busy { phase, call, attempt };
}

/// What an entry's state implies, applied after every transition: whether
/// its backoff's alarm runs, and whether it is retired, which frees room.
fn follow(model: &mut Model, env: &Env<Limits>, id: Id<Entry>) {
    let entry = model.entries.get(id).expect("an entry lives until it is retired");
    match entry.state {
        State::Waiting { until, .. } => {
            model.alarms.arm(Alarm::Item(id), until).expect("an alarm per entry fits");
        }
        State::Busy { .. } | State::Idle => model.alarms.cancel(Alarm::Item(id)),
        State::Closed => {
            model.alarms.cancel(Alarm::Item(id));
            model.entries.retire(id);
            scans::room(model, env);
        }
        State::Due { .. } | State::Gone => unreachable!("a step settles these before it ends"),
    }
}

/// The record of `item` as the working set knows it, if it is held and read;
/// the position taken; and the nonce of a record write that may have landed.
pub(crate) fn record(model: &Model, item: Item) -> Option<(Record, Position, Option<u64>)> {
    let id = model.index.get(&item)?;
    let entry = model.entries.get(*id).expect("the index names live entries");
    Some((entry.record?, entry.taken, entry.uncertain))
}

/// A write found `item`'s record as `record`: posted, edited, or changed by
/// someone else.
pub(crate) fn recorded(model: &mut Model, item: Item, record: Record) {
    let Some(id) = model.index.get(&item) else {
        return;
    };
    let entry = model.entries.get_mut(*id).expect("the index names live entries");
    if entry.record.is_some() {
        entry.record = Some(record);
        entry.uncertain = None;
    }
}

/// A record write of `item` carrying `nonce` gave up after an attempt that
/// may have landed: what the record now is, is not known.
pub(crate) fn uncertain(model: &mut Model, item: Item, nonce: u64) {
    let Some(id) = model.index.get(&item) else {
        return;
    };
    let entry = model.entries.get_mut(*id).expect("the index names live entries");
    if entry.record.is_some() {
        entry.uncertain = Some(nonce);
    }
}

/// Where comments of `item` posted from now are looked for: after the last
/// one the working set passed, or from the first.
pub(crate) fn passed(model: &Model, item: Item) -> u64 {
    match model.index.get(&item) {
        Some(id) => model.entries.get(*id).expect("the index names live entries").announced.comment,
        None => 0,
    }
}
