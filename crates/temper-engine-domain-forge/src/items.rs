//! The working set (engine-domain.md, section 12): the items the engine
//! tracks that are not done, and what its decisions need of each. An item
//! enters when the parent takes it in or a listing finds it carrying the
//! tracking label, and leaves when it closes, when the forge no longer has
//! it, or when the parent stops tracking it; a full working set refuses it at
//! the entrance, and it waits on the forge until there is room
//! ([`crate::scans`]).
//!
//! Entering, an item is read from its first comment until its record is
//! found: the engine's own comment carrying a record block, or one that does
//! not decode (then the item is held for a person), or none. It is then
//! announced to the parent, and its inbox (4.3) is derived from the inbox
//! position its record names: the comments after it that are not the
//! engine's own, on the item and on its pull request, and its pull request's
//! state and verdicts when they differ from what was taken. The inbox holds
//! `Limits::inbox` news, its last place kept for its pull request's state and
//! verdicts; what does not fit waits on the forge until the parent takes some
//! ([`took`]), as nothing of it is queued but where to read from. News the
//! parent had no room for is told again when it asks ([`retell`]).
//!
//! **Level state.** An item's pull request is read whatever room its inbox
//! has, so [`crate::Domain::pull`] is as fresh as the forge allows: its head,
//! where its base is, CI on its head, whether it is open, merged, merges
//! cleanly; and the verdicts on its head, the latest of each reviewer save
//! the engine, read a page at a time ([`crate::Domain::reviews`]). What the
//! inbox is told of them is the level against the position taken: a news
//! when the head, CI or state moved, and one when the verdicts did (their
//! digest), whenever there is room.
//!
//! An item is read again when a listing shows it changed: its comments for a
//! new updated time, and, for a pull request, its state and verdicts too; its
//! linked pull request, its verdicts and its comments, when a listing shows
//! that changed. A pull request's state moves no updated time when CI
//! reports or its base moves, so it is also read when a webhook names its
//! head's commit or its base branch, and on a backoff of its own whatever
//! its state: every poll at first, twice as long each time it is found
//! unchanged, up to the slow pass's interval. A listing that shows an item
//! at the updated time it last showed means it again only once, and only if
//! that time had passed by when the listing was made, the forge's times
//! being a second apart at best ([`changed`]).
//!
//! A read that fails for a while is tried again after a backoff. One the
//! forge refuses as forbidden is too, and the parent is told
//! ([`Request::Forbidden`]) once it has been refused `Limits::attempts`
//! times in a row; one it answers missing makes the item leave.
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
//!          read, closed; missing          (gone)   left: closed, missing
//! Reading  read, more and room            Reading  news, changed
//!          read, otherwise                Idle     news, changed
//!          read, closed; missing          (gone)   left: closed, missing
//! Pulling  read                           Idle     news (level)
//!          missing                        Idle
//! Reviewing read, more                    Reviewing
//!          read, the last page            Idle     news (level)
//! Remarking read, more and room           Remarking news
//!          read, otherwise                Idle     news
//! Busy     failed, rate                   Busy     (queued again)
//!          failed, forbidden, attempts    Waiting  forbidden (once)
//!            spent
//!          failed, otherwise              Waiting
//!          untracked, listed closed       (left)   left, when listed closed
//! Waiting  alarm                          Busy
//! Idle     stale                          Busy     (a read queued)
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

use crate::api::{self, Answer, Comment, Error, Kind, Mark, Op, State as Open, Summary, Verdict};
use crate::boundary::{Ci, Item, Level, News, Position, Record, Request, Reviewed, View, Why};
use crate::calls::{self, Calls, Purpose};
use crate::domain::{Alarm, Config, Domain};
use crate::facts::{Fact, Priority};
use crate::limits::Limits;
use crate::scans;

#[derive(Debug)]
#[expect(clippy::struct_excessive_bools, reason = "what is known and told of an item, each on its own")]
pub(crate) struct Entry {
    item: Item,
    /// Known from its first read.
    kind: Option<Kind>,
    labels: Box<[Box<[u8]>]>,
    /// Its updated time as listings show it, and its linked pull request's.
    seen: Seen,
    pull_seen: Seen,
    /// The pull request carrying its change, or itself if it is one; that
    /// pull request as last read, its base branch, and as last told.
    pull: Option<u64>,
    level: Option<Level>,
    base: Box<[u8]>,
    told: Told,
    /// The verdicts on its pull request's head, once read for that head; and
    /// those of the read under way, page by page.
    reviews: List<Reviewed>,
    reviewed: bool,
    scratch: List<Reviewed>,
    /// When its pull request is next read whatever else happens, and how long
    /// after that the next is, unless it changes.
    poll_at: Time,
    poll_every: Duration,
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
    /// Whether it counts in the cold start's reads, and whether the parent
    /// was told the forge forbids it.
    counted: bool,
    forbidden: bool,
    /// The slow pass's cycle that last listed it.
    cycle: u64,
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

/// What listings showed of an item: its updated time, the forge's, and
/// whether a listing made once that time had passed showed it, after which
/// a read finds all it says.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) struct Seen {
    updated: Time,
    settled: bool,
}

impl Seen {
    /// What a listing made at `now` showing `updated` says.
    pub(crate) fn listed(updated: Time, now: Time, resolution: Duration) -> Seen {
        Seen { updated, settled: now >= updated.saturating_add(resolution) }
    }

    /// Nothing listed.
    pub(crate) const NONE: Seen = Seen { updated: Time::ZERO, settled: false };
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

/// What is owed a read: its comments, its pull request's state, verdicts and
/// comments.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[expect(clippy::struct_excessive_bools, reason = "each read is owed on its own: a set of flags")]
struct Stale {
    comments: bool,
    pull: bool,
    reviews: bool,
    remarks: bool,
}

const FRESH: Stale = Stale { comments: false, pull: false, reviews: false, remarks: false };

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
    /// The last read failed, refused as forbidden or not: tried again at
    /// `until`.
    Waiting {
        phase: Phase,
        attempt: u32,
        until: Time,
        forbidden: bool,
    },
    Idle,
    /// It is not on the forge as the working set holds items: closed, or
    /// gone. It leaves.
    Gone {
        why: Why,
    },
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
    /// The `page`th page of the pull request `number`'s reviews.
    Reviewing { number: u64, page: u32 },
    /// The comments on the pull request `number` after the last passed.
    Remarking { number: u64 },
}

/// The pull request of `entry`, as last read.
pub(crate) fn level(entry: &Entry) -> Option<Level> {
    entry.level
}

/// The verdicts on the head of `entry`'s pull request, once read for it.
pub(crate) fn verdicts(entry: &Entry) -> Option<&[Reviewed]> {
    if entry.reviewed { Some(entry.reviews.as_slice()) } else { None }
}

/// Takes `item` in, if there is room; refuses it otherwise, and it waits on
/// the forge.
pub(crate) fn track(domain: &mut Domain, env: &Env<Limits>, item: Item, out: &mut Queue<Request>) {
    assert!(item.repository < env.limits.repositories, "the parent names the deployment's repositories");
    if domain.index.contains_key(&item) {
        return;
    }
    if admit(domain, env, item, Seen::NONE).is_none() {
        domain.facts.push(Fact::Refused { item });
        domain.refused = true;
        scans::wait(domain, item.repository);
        out.push(Request::Full { item });
    }
}

/// Admits `item` into the working set, to be read for its record, unless it
/// is full; returns its entry. `seen` is what the listing that found it
/// showed, if one did.
pub(crate) fn admit(domain: &mut Domain, env: &Env<Limits>, item: Item, seen: Seen) -> Option<Id<Entry>> {
    if let Some(id) = domain.index.get(&item) {
        return Some(*id);
    }
    if domain.entries.is_full() {
        return None;
    }
    let limits = &env.limits;
    let entry = Entry {
        item,
        kind: None,
        labels: Box::new([]),
        seen,
        pull_seen: Seen::NONE,
        pull: None,
        level: None,
        base: Box::new([]),
        told: QUIET,
        reviews: List::with_capacity(limits.reviewers),
        reviewed: false,
        scratch: List::with_capacity(limits.reviewers),
        poll_at: env.now,
        poll_every: limits.poll,
        record: None,
        uncertain: None,
        taken: Position::START,
        announced: Position::START,
        inbox: Queue::with_capacity(limits.inbox),
        seq: 0,
        stale: FRESH,
        counted: true,
        forbidden: false,
        cycle: scans::cycle(domain, item.repository),
        left: false,
        state: State::Due { phase: Phase::Finding { after: 0 }, attempt: 0 },
    };
    let id = domain.entries.insert(entry).expect("checked for room above");
    domain.index.insert(item, id).expect("an index as large as the working set");
    domain.loading.finding = domain.loading.finding.saturating_add(1);
    domain.facts.push(Fact::Admitted { item });
    kick(&mut domain.entries, &mut domain.calls, id);
    Some(id)
}

/// The parent stops tracking `item`: it leaves, and nothing is told of it.
pub(crate) fn untrack(domain: &mut Domain, env: &Env<Limits>, item: Item) {
    let Some(&id) = domain.index.get(&item) else {
        return;
    };
    leave(domain, id);
    follow(domain, env, id);
}

/// `item`'s change is carried by the pull request `pull`, or none. What was
/// taken or held of another pull request says nothing of this one.
pub(crate) fn link(domain: &mut Domain, env: &Env<Limits>, item: Item, pull: Option<u64>) {
    let Some(&id) = domain.index.get(&item) else {
        return;
    };
    let entry = domain.entries.get_mut(id).expect("the index names live entries");
    match entry.kind {
        Some(Kind::Pull) => return,
        Some(Kind::Issue) | None => {}
    }
    let old = entry.pull;
    if old != pull {
        if let Some(number) = old {
            let name = Item { repository: item.repository, number };
            if domain.pulls.get(&name) == Some(&id) {
                domain.pulls.remove(&name);
            }
            entry.taken = unlinked(entry.taken);
            entry.announced = unlinked(entry.announced);
            for _ in 0..entry.inbox.len() {
                let Some(held) = entry.inbox.pop() else {
                    break;
                };
                entry.inbox.push(Held { position: unlinked(held.position), ..held });
            }
        }
        if let Some(number) = pull {
            let name = Item { repository: item.repository, number };
            domain.pulls.insert(name, id).expect("a pull request per item held");
        }
        entry.pull = pull;
        entry.level = None;
        entry.told = QUIET;
        entry.reviews.clear();
        entry.reviewed = false;
        entry.pull_seen = Seen::NONE;
        entry.poll_every = env.limits.poll;
    }
    let linked = pull.is_some();
    entry.stale = Stale { pull: linked, reviews: linked, remarks: linked, ..entry.stale };
    kick(&mut domain.entries, &mut domain.calls, id);
    follow(domain, env, id);
}

/// A position with nothing taken of a pull request.
fn unlinked(position: Position) -> Position {
    Position { pull_comment: 0, reviews: 0, head: None, ci: Ci::None, ..position }
}

/// The parent took `item`'s news through `through`: the position moves past
/// them, and room frees for more, the level's first.
pub(crate) fn took(domain: &mut Domain, env: &Env<Limits>, item: Item, through: u64, out: &mut Queue<Request>) {
    let Some(&id) = domain.index.get(&item) else {
        return;
    };
    let entry = domain.entries.get_mut(id).expect("the index names live entries");
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
    if entry.record.is_some() {
        tell_level(entry, out);
    }
    kick(&mut domain.entries, &mut domain.calls, id);
    follow(domain, env, id);
}

/// The parent had no room for `item`'s news from the `from`th on: the news
/// still held from it is told again, unchanged.
pub(crate) fn retell(domain: &Domain, item: Item, from: u64, out: &mut Queue<Request>) {
    let Some(&id) = domain.index.get(&item) else {
        return;
    };
    let entry = domain.entries.get(id).expect("the index names live entries");
    for held in &entry.inbox {
        if held.seq >= from {
            out.push(Request::Inbox { item: entry.item, seq: held.seq, news: held.news });
        }
    }
}

/// A listing made at `now` shows `summary`, an item of the working set.
pub(crate) fn listed(
    domain: &mut Domain,
    env: &Env<Limits>,
    id: Id<Entry>,
    summary: &Summary,
    now: Time,
    out: &mut Queue<Request>,
) {
    let config = &domain.config;
    let entry = domain.entries.get_mut(id).expect("the index names live entries");
    if summary.state == Open::Closed {
        let item = entry.item;
        leave(domain, id);
        out.push(Request::Left { item, why: Why::Closed });
    } else {
        absorb(entry, &env.limits, config, summary, out);
        if changed(&mut entry.seen, summary.updated, now, env.limits.resolution) {
            entry.stale.comments = true;
            if entry.kind == Some(Kind::Pull) {
                entry.stale.pull = true;
                entry.stale.reviews = true;
            }
        }
        kick(&mut domain.entries, &mut domain.calls, id);
    }
    follow(domain, env, id);
}

/// A listing made at `now` shows `summary`, the pull request linked to the
/// item `id`: its state, verdicts and comments are read again if it moved.
pub(crate) fn linked(domain: &mut Domain, env: &Env<Limits>, id: Id<Entry>, summary: &Summary, now: Time) {
    let entry = domain.entries.get_mut(id).expect("the index names live entries");
    if changed(&mut entry.pull_seen, summary.updated, now, env.limits.resolution) {
        entry.stale = Stale { pull: true, reviews: true, remarks: true, ..entry.stale };
    }
    kick(&mut domain.entries, &mut domain.calls, id);
    follow(domain, env, id);
}

/// Its repository is polled: the open pull requests of its items whose
/// backoff has passed are read again, as CI and a moving base move no
/// updated time.
pub(crate) fn poll(domain: &mut Domain, env: &Env<Limits>, repository: u32) {
    for (item, id) in &domain.index {
        if item.repository != repository {
            continue;
        }
        let entry = domain.entries.get_mut(*id).expect("the index names live entries");
        let open = match entry.level {
            Some(level) => level.open,
            None => true,
        };
        if entry.pull.is_none() || !open || env.now < entry.poll_at {
            continue;
        }
        entry.poll_at = env.now.saturating_add(entry.poll_every);
        entry.poll_every = entry.poll_every.saturating_mul(2).min(env.limits.slow.max(env.limits.poll));
        entry.stale.pull = true;
        kick(&mut domain.entries, &mut domain.calls, *id);
    }
}

/// A webhook named `commit`, a status's or a push's, or `branch`, a push's:
/// the pull requests on that head, or into that base, are read again.
pub(crate) fn hinted(domain: &mut Domain, repository: u32, commit: Option<[u8; 32]>, branch: Option<&[u8]>) {
    for (item, id) in &domain.index {
        if item.repository != repository {
            continue;
        }
        let entry = domain.entries.get_mut(*id).expect("the index names live entries");
        if entry.pull.is_none() {
            continue;
        }
        let head = match commit {
            Some(commit) => match entry.level {
                Some(level) => level.commit == commit,
                None => false,
            },
            None => false,
        };
        let base = match branch {
            Some(branch) => !entry.base.is_empty() && *entry.base == *branch,
            None => false,
        };
        if head || base {
            entry.stale.pull = true;
            kick(&mut domain.entries, &mut domain.calls, *id);
        }
    }
}

/// The slow pass's listing of its `cycle`th cycle showed the item `id`.
pub(crate) fn slow_listed(domain: &mut Domain, id: Id<Entry>, cycle: u64) {
    domain.entries.get_mut(id).expect("the index names live entries").cycle = cycle;
}

/// The slow pass's `cycle`th cycle over `repository` ended: the items it
/// held throughout that no page of it showed open are read, to find whether
/// they are still there.
pub(crate) fn unlisted(domain: &mut Domain, repository: u32, cycle: u64) {
    for (item, id) in &domain.index {
        if item.repository != repository {
            continue;
        }
        let entry = domain.entries.get_mut(*id).expect("the index names live entries");
        if entry.cycle < cycle && entry.record.is_some() {
            entry.cycle = cycle;
            entry.stale.comments = true;
            kick(&mut domain.entries, &mut domain.calls, *id);
        }
    }
}

/// A backoff ended: the read is tried again.
pub(crate) fn retry(domain: &mut Domain, env: &Env<Limits>, id: Id<Entry>) {
    let entry = domain.entries.get_mut(id).expect("an alarm is cancelled as its entry is retired");
    entry.state = match entry.state {
        State::Waiting { phase, attempt, until: _, forbidden: _ } => State::Due { phase, attempt },
        State::Due { .. } | State::Busy { .. } | State::Idle | State::Gone { .. } | State::Closed => {
            unreachable!("the backoff's alarm runs while it waits")
        }
    };
    kick(&mut domain.entries, &mut domain.calls, id);
    follow(domain, env, id);
}

/// Terminal for the read of the item `id`.
pub(crate) fn answered(
    domain: &mut Domain,
    env: &Env<Limits>,
    id: Id<Entry>,
    result: Result<Answer, Error>,
    out: &mut Queue<Request>,
) {
    let config = &domain.config;
    let entry = domain.entries.get_mut(id).expect("an entry lives until its read is answered");
    let state = mem::replace(&mut entry.state, State::Closed);
    entry.state = match state {
        State::Busy { phase, call: _, attempt } => {
            if entry.left {
                State::Closed
            } else {
                let rng = &mut domain.rng;
                match phase {
                    Phase::Finding { after } => found(entry, config, env, rng, after, attempt, result, out),
                    Phase::Reading => read(entry, config, env, rng, attempt, result, out),
                    Phase::Pulling { number } => pulled(entry, env, rng, number, attempt, result, out),
                    Phase::Reviewing { number, page } => {
                        reviewed(entry, config.engine, env, rng, number, page, attempt, result, out)
                    }
                    Phase::Remarking { number } => {
                        remarked(entry, config.engine, env, rng, number, attempt, result, out)
                    }
                }
            }
        }
        State::Due { .. } | State::Waiting { .. } | State::Idle | State::Gone { .. } | State::Closed => {
            unreachable!("a read's terminal comes while it is out")
        }
    };
    conclude(domain, env, id, out);
}

/// What a call for the item `id` asks, as it goes out.
pub(crate) fn op(domain: &Domain, id: Id<Entry>) -> (u32, Op) {
    let entry = domain.entries.get(id).expect("an entry lives until its read is answered");
    let number = entry.item.number;
    let op = match entry.state {
        State::Busy { phase, .. } => match phase {
            Phase::Finding { after } => Op::Item { number, after },
            Phase::Reading => Op::Item { number, after: entry.announced.comment },
            Phase::Pulling { number } => Op::Pull { number },
            Phase::Reviewing { number, page } => Op::Reviews { number, page },
            Phase::Remarking { number } => Op::Item { number, after: entry.announced.pull_comment },
        },
        State::Due { .. } | State::Waiting { .. } | State::Idle | State::Gone { .. } | State::Closed => {
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
    config: &Config,
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
    entry.forbidden = false;
    if summary.state == Open::Closed {
        return State::Gone { why: Why::Closed };
    }
    let comments = comments.get(..page(&env.limits)).unwrap_or(&comments);
    entry.kind = Some(summary.kind);
    absorb(entry, &env.limits, config, &summary, out);
    let mut record = None;
    let mut last = after;
    for comment in comments {
        last = last.max(comment.id);
        if comment.author != config.engine {
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
            Mark::None | Mark::Key { .. } => {}
        }
    }
    let record = match record {
        Some(record) => record,
        None if more && last > after => return State::Due { phase: Phase::Finding { after: last }, attempt: 0 },
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
        entry.stale = Stale { pull: true, reviews: true, ..entry.stale };
    }
    let view = View { kind: summary.kind, labels: copy_labels(&entry.labels), record };
    out.push(Request::Announced { item: entry.item, view });
    if position.comment < after {
        // Comments before this page come after the position: read them.
        return State::Due { phase: Phase::Reading, attempt: 0 };
    }
    after_comments(entry, config.engine, comments, more, Phase::Reading, out)
}

/// Reading, read: the page's news.
fn read(
    entry: &mut Entry,
    config: &Config,
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
    entry.forbidden = false;
    if summary.state == Open::Closed {
        return State::Gone { why: Why::Closed };
    }
    let comments = comments.get(..page(&env.limits)).unwrap_or(&comments);
    absorb(entry, &env.limits, config, &summary, out);
    after_comments(entry, config.engine, comments, more, Phase::Reading, out)
}

/// Remarking, read: the news of a page of comments on the item's pull
/// request.
#[expect(clippy::too_many_arguments, reason = "a cell handler over the fields it touches")]
fn remarked(
    entry: &mut Entry,
    engine: u64,
    env: &Env<Limits>,
    rng: &mut Rng,
    number: u64,
    attempt: u32,
    result: Result<Answer, Error>,
    out: &mut Queue<Request>,
) -> State {
    let (_, comments, more) = match result {
        Ok(answer) => api::item(answer),
        Err(Error::Missing) => return State::Idle,
        Err(error) => return failed(env, rng, Phase::Remarking { number }, attempt, error),
    };
    if entry.pull != Some(number) {
        // Linked to another since: the read says nothing of it.
        return State::Idle;
    }
    let comments = comments.get(..page(&env.limits)).unwrap_or(&comments);
    after_comments(entry, engine, comments, more, Phase::Remarking { number }, out)
}

/// The news of a page of comments, on the item or on its pull request as
/// `phase` reads them, and what follows it: the next page, or nothing until
/// there is room.
fn after_comments(
    entry: &mut Entry,
    engine: u64,
    comments: &[Comment],
    more: bool,
    phase: Phase,
    out: &mut Queue<Request>,
) -> State {
    let on = match phase {
        Phase::Remarking { number } => number,
        Phase::Finding { .. } | Phase::Reading | Phase::Pulling { .. } | Phase::Reviewing { .. } => entry.item.number,
    };
    let stopped = comment_news(entry, engine, on, comments, out);
    let owed = stopped || (more && !has_room_for_comments(entry));
    if owed {
        match phase {
            Phase::Remarking { .. } => entry.stale.remarks = true,
            Phase::Finding { .. } | Phase::Reading | Phase::Pulling { .. } | Phase::Reviewing { .. } => {
                entry.stale.comments = true;
            }
        }
        return State::Idle;
    }
    if more {
        return State::Due { phase, attempt: 0 };
    }
    State::Idle
}

/// Pulling, read: the pull request's state, kept whatever room the inbox
/// has, and told if it moved.
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
        Err(Error::Missing) => {
            // No such pull request: nothing to tell of it.
            if entry.pull == Some(number) {
                entry.level = None;
            }
            return State::Idle;
        }
        Err(error) => return failed(env, rng, Phase::Pulling { number }, attempt, error),
    };
    if entry.pull != Some(number) {
        // Linked to another since: the read says nothing of it.
        return State::Idle;
    }
    let level = Level {
        number,
        commit: pull.commit,
        base: pull.base_commit,
        ci: pull.ci,
        open: pull.state == Open::Open,
        merged: pull.merged,
        mergeable: pull.mergeable,
    };
    let was = entry.level;
    if was != Some(level) {
        entry.poll_every = env.limits.poll;
        entry.poll_at = env.now.saturating_add(env.limits.poll);
    }
    let head = match was {
        Some(was) => was.commit != level.commit,
        None => true,
    };
    if head {
        // The verdicts are the new head's, read afresh.
        entry.reviews.clear();
        entry.reviewed = false;
        entry.stale.reviews = true;
    }
    entry.level = Some(level);
    if *entry.base != *pull.base {
        // Kept to the limits: a base beyond them is never named by a push.
        entry.base = Box::new([]);
        if fits(&pull.base, env.limits.name_bytes) {
            entry.base = copy_of(&pull.base);
        }
    }
    if entry.record.is_some() {
        tell_level(entry, out);
    }
    State::Idle
}

/// Reviewing, read: a page of the verdicts on the head; once the last is
/// read, they are the level, told if they moved.
#[expect(clippy::too_many_arguments, reason = "a cell handler over the fields it touches")]
fn reviewed(
    entry: &mut Entry,
    engine: u64,
    env: &Env<Limits>,
    rng: &mut Rng,
    number: u64,
    page: u32,
    attempt: u32,
    result: Result<Answer, Error>,
    out: &mut Queue<Request>,
) -> State {
    let (reviews, more) = match result {
        Ok(answer) => api::reviews(answer),
        Err(Error::Missing) => return State::Idle,
        Err(error) => return failed(env, rng, Phase::Reviewing { number, page }, attempt, error),
    };
    if entry.pull != Some(number) {
        return State::Idle;
    }
    let Some(level) = entry.level else {
        // The head is read first: the verdicts are on it.
        entry.stale.reviews = true;
        return State::Idle;
    };
    if page == 1 {
        entry.scratch.clear();
    }
    let reviews = reviews.get(..self::page(&env.limits)).unwrap_or(&reviews);
    for review in reviews {
        let verdict = match review.verdict {
            Verdict::Approve | Verdict::RequestChanges => review.verdict,
            Verdict::Comment => continue,
        };
        if review.commit != level.commit || review.author == engine {
            continue;
        }
        verdict_of(&mut entry.scratch, review.author, verdict);
    }
    if more && !reviews.is_empty() {
        return State::Due { phase: Phase::Reviewing { number, page: page.saturating_add(1) }, attempt: 0 };
    }
    mem::swap(&mut entry.reviews, &mut entry.scratch);
    entry.scratch.clear();
    entry.reviewed = true;
    if entry.record.is_some() {
        tell_level(entry, out);
    }
    State::Idle
}

/// Keeps `verdict` as `author`'s latest, among as many reviewers as the
/// limits hold.
fn verdict_of(verdicts: &mut List<Reviewed>, author: u64, verdict: Verdict) {
    for index in 0..verdicts.len() {
        let kept = verdicts.get_mut(index).expect("within its length");
        if kept.author == author {
            kept.verdict = verdict;
            return;
        }
    }
    // Beyond the limits, a reviewer is not counted.
    let _kept = verdicts.push(Reviewed { author, verdict });
}

/// A read failed: queued again at once for the rate, whose reset holds every
/// call; the item gone if the forge has it no more; otherwise tried again
/// after a backoff, a refusal as forbidden among them.
fn failed(env: &Env<Limits>, rng: &mut Rng, phase: Phase, attempt: u32, error: Error) -> State {
    match error {
        Error::RateLimited { .. } => State::Due { phase, attempt },
        Error::Missing => State::Gone { why: Why::Missing },
        Error::Forbidden
        | Error::Unavailable
        | Error::Timeout
        | Error::TooLarge
        | Error::Empty
        | Error::Full
        | Error::Exists
        | Error::NothingToMerge
        | Error::Closed
        | Error::Stale
        | Error::Conflict
        | Error::Protected
        | Error::Circular => {
            let attempt = attempt.saturating_add(1);
            let until = env.now.saturating_add(backoff(&env.limits, rng, attempt));
            State::Waiting { phase, attempt, until, forbidden: error == Error::Forbidden }
        }
    }
}

/// Whether the inbox has room for a comment: its last place is kept for
/// the level.
fn has_room_for_comments(entry: &Entry) -> bool {
    entry.inbox.room() > 1
}

/// The comments of a page, on the item `on`, that are news, told while the
/// inbox has room: people's, and those the engine wrote for a person, as
/// theirs; the engine's own are passed over. Says whether it stopped for
/// room.
fn comment_news(entry: &mut Entry, engine: u64, on: u64, comments: &[Comment], out: &mut Queue<Request>) -> bool {
    let own = on == entry.item.number;
    for comment in comments {
        let last = if own { entry.announced.comment } else { entry.announced.pull_comment };
        if comment.id <= last {
            continue;
        }
        let position = if own {
            Position { comment: comment.id, ..entry.announced }
        } else {
            Position { pull_comment: comment.id, ..entry.announced }
        };
        let author = match &comment.mark {
            Mark::Key { key: _, person: Some(person) } if comment.author == engine => *person,
            Mark::Key { .. } | Mark::None | Mark::Record { .. } | Mark::Mangled => comment.author,
        };
        if author == engine {
            entry.announced = position;
            continue;
        }
        if !has_room_for_comments(entry) {
            return true;
        }
        tell(entry, News::Comment { on, id: comment.id, author }, position, out);
    }
    false
}

/// Tells what the level of the item's pull request says against the
/// position: its state if it moved, and its verdicts if they did, while
/// there is room.
fn tell_level(entry: &mut Entry, out: &mut Queue<Request>) {
    let Some(level) = entry.level else {
        return;
    };
    let told = Told { open: level.open, merged: level.merged, mergeable: level.mergeable };
    let quiet = entry.announced.head == Some(level.commit) && entry.announced.ci == level.ci && entry.told == told;
    if !quiet && entry.inbox.room() > 0 {
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
    }
    if !entry.reviewed {
        return;
    }
    let digest = digest(level.commit, entry.reviews.as_slice());
    if entry.announced.reviews != digest && entry.inbox.room() > 0 {
        let position = Position { reviews: digest, ..entry.announced };
        tell(entry, News::Reviews { commit: level.commit }, position, out);
    }
}

/// A digest of the verdicts on the head `commit`, whatever their order: the
/// position of what was taken of them. None is none, on any head; the same
/// verdicts on another head are others.
fn digest(commit: [u8; 32], verdicts: &[Reviewed]) -> u64 {
    if verdicts.is_empty() {
        return 0;
    }
    let mut digest: u64 = fnv(&commit);
    for kept in verdicts {
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        let verdict: u8 = match kept.verdict {
            Verdict::Approve => 1,
            Verdict::RequestChanges => 2,
            Verdict::Comment => 3,
        };
        for byte in kept.author.to_be_bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
        hash ^= u64::from(verdict);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
        digest = digest.wrapping_add(hash);
    }
    digest
}

/// The FNV-1a hash of `bytes`.
fn fnv(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
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
/// change once the item is announced. Labels beyond the limits are not
/// kept, the tracking and hand-in labels first among those that are.
fn absorb(entry: &mut Entry, limits: &Limits, config: &Config, summary: &Summary, out: &mut Queue<Request>) {
    let labels = kept_labels(limits, config, &summary.labels);
    if *entry.labels == *labels {
        return;
    }
    if entry.record.is_some() {
        out.push(Request::Changed { item: entry.item, labels: copy_labels(&labels) });
    }
    entry.labels = labels;
}

/// The labels of `labels` the working set keeps: those within the limits'
/// `name_bytes`, as many as its `labels`, the tracking and hand-in labels
/// first, then the rest in their order.
fn kept_labels(limits: &Limits, config: &Config, labels: &[Box<[u8]>]) -> Box<[Box<[u8]>]> {
    let mut kept = List::with_capacity(limits.labels);
    for label in labels {
        let ours = **label == *config.tracking || **label == *config.hand_in;
        if ours && fits(label, limits.name_bytes) {
            let _kept = kept.push(copy_of(label));
        }
    }
    for label in labels {
        let ours = **label == *config.tracking || **label == *config.hand_in;
        if !ours && fits(label, limits.name_bytes) {
            let _kept = kept.push(copy_of(label));
        }
    }
    kept.into_boxed()
}

fn fits(bytes: &[u8], most: u32) -> bool {
    match u32::try_from(bytes.len()) {
        Ok(len) => len <= most,
        Err(_) => false,
    }
}

/// Whether a listing made at `now` that shows `updated` means news: a newer
/// time does; the same time does once more, if the listing that showed it
/// first was made within that time, and this one after it: the forge's times
/// are coarse, and a change may have come after the read in the same
/// second. Forge times both, compared only with one another.
pub(crate) fn changed(seen: &mut Seen, updated: Time, now: Time, resolution: Duration) -> bool {
    let listed = Seen::listed(updated, now, resolution);
    if updated > seen.updated {
        *seen = listed;
        return true;
    }
    if updated == seen.updated && !seen.settled && listed.settled {
        seen.settled = true;
        return true;
    }
    false
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
fn leave(domain: &mut Domain, id: Id<Entry>) {
    let entry = domain.entries.get_mut(id).expect("an entry leaves once");
    let item = entry.item;
    if domain.index.get(&item) == Some(&id) {
        domain.index.remove(&item);
    }
    if let Some(number) = entry.pull {
        let name = Item { repository: item.repository, number };
        if domain.pulls.get(&name) == Some(&id) {
            domain.pulls.remove(&name);
        }
    }
    if entry.counted {
        entry.counted = false;
        domain.loading.finding = domain.loading.finding.saturating_sub(1);
    }
    entry.left = true;
    entry.state = match entry.state {
        State::Busy { phase, call, attempt } => State::Busy { phase, call, attempt },
        State::Due { .. } | State::Waiting { .. } | State::Idle | State::Gone { .. } => State::Closed,
        State::Closed => unreachable!("an entry leaves once"),
    };
    domain.facts.push(Fact::Left { item });
}

/// Applied after every transition: an item gone leaves, and the parent is
/// told; one the forge keeps forbidding is told once; one whose record is
/// known no longer counts in the cold start; an idle item owed a read gets
/// one; then what the state implies ([`follow`]).
fn conclude(domain: &mut Domain, env: &Env<Limits>, id: Id<Entry>, out: &mut Queue<Request>) {
    let entry = domain.entries.get_mut(id).expect("an entry lives until it is retired");
    let item = entry.item;
    match entry.state {
        State::Gone { why } => {
            leave(domain, id);
            out.push(Request::Left { item, why });
        }
        State::Waiting { forbidden: true, attempt, .. } if attempt >= env.limits.attempts => {
            if !entry.forbidden {
                entry.forbidden = true;
                out.push(Request::Forbidden { item });
            }
        }
        State::Due { .. } | State::Busy { .. } | State::Waiting { .. } | State::Idle | State::Closed => {}
    }
    let entry = domain.entries.get_mut(id).expect("an entry lives until it is retired");
    if entry.counted && (entry.record.is_some() || entry.forbidden) {
        // Read, or refused for good: the cold start does not wait for it.
        entry.counted = false;
        domain.loading.finding = domain.loading.finding.saturating_sub(1);
    }
    kick(&mut domain.entries, &mut domain.calls, id);
    follow(domain, env, id);
}

/// Queues the read an entry is due, or owed while it is idle: its comments
/// and its pull request's while its inbox has room for them, its pull
/// request's state and verdicts whatever room it has; each owed read in turn,
/// so that none waits on another made owed again and again.
fn kick(entries: &mut Slab<Entry>, calls: &mut Calls, id: Id<Entry>) {
    let entry = entries.get_mut(id).expect("an entry lives until it is retired");
    let room = has_room_for_comments(entry);
    let (phase, attempt) = match entry.state {
        State::Due { phase, attempt } => (phase, attempt),
        State::Idle => {
            let Some(phase) = owed(entry, room) else {
                return;
            };
            (phase, 0)
        }
        State::Busy { .. } | State::Waiting { .. } | State::Gone { .. } | State::Closed => return,
    };
    match phase {
        Phase::Reading => entry.stale.comments = false,
        Phase::Pulling { .. } => entry.stale.pull = false,
        Phase::Reviewing { .. } => entry.stale.reviews = false,
        Phase::Remarking { .. } => entry.stale.remarks = false,
        Phase::Finding { .. } => {}
    }
    let call = calls::queue(calls, Purpose::Item(id), Priority::Keep);
    entry.state = State::Busy { phase, call, attempt };
}

/// The read an idle entry is owed, if any.
fn owed(entry: &mut Entry, room: bool) -> Option<Phase> {
    if entry.stale.comments && room {
        return Some(Phase::Reading);
    }
    let Some(number) = entry.pull else {
        entry.stale = Stale { pull: false, reviews: false, remarks: false, ..entry.stale };
        return None;
    };
    // Its state first, if it is not known; then its verdicts and comments,
    // before its state is read again: webhooks can make that owed again and
    // again, and nothing else of it must wait for them.
    if entry.level.is_none() && (entry.stale.pull || entry.stale.reviews) {
        return Some(Phase::Pulling { number });
    }
    if entry.stale.reviews {
        return Some(Phase::Reviewing { number, page: 1 });
    }
    if entry.stale.remarks && room && number != entry.item.number {
        return Some(Phase::Remarking { number });
    }
    if entry.stale.pull {
        return Some(Phase::Pulling { number });
    }
    entry.stale.remarks = entry.stale.remarks && number != entry.item.number;
    None
}

/// What an entry's state implies, applied after every transition: whether
/// its backoff's alarm runs, and whether it is retired, which frees room.
fn follow(domain: &mut Domain, env: &Env<Limits>, id: Id<Entry>) {
    let entry = domain.entries.get(id).expect("an entry lives until it is retired");
    match entry.state {
        State::Waiting { until, .. } => {
            domain.alarms.arm(Alarm::Item(id), until).expect("an alarm per entry fits");
        }
        State::Busy { .. } | State::Idle => domain.alarms.cancel(Alarm::Item(id)),
        State::Closed => {
            domain.alarms.cancel(Alarm::Item(id));
            domain.entries.retire(id);
            scans::room(domain, env);
        }
        State::Due { .. } | State::Gone { .. } => unreachable!("a step settles these before it ends"),
    }
}

/// The record of `item` as the working set knows it, if it is held and read;
/// the position taken; and the nonce of a record write that may have landed.
pub(crate) fn record(domain: &Domain, item: Item) -> Option<(Record, Position, Option<u64>)> {
    let id = domain.index.get(&item)?;
    let entry = domain.entries.get(*id).expect("the index names live entries");
    Some((entry.record?, entry.taken, entry.uncertain))
}

/// A write found `item`'s record as `record`: posted, edited, or changed by
/// someone else.
pub(crate) fn recorded(domain: &mut Domain, item: Item, record: Record) {
    let Some(id) = domain.index.get(&item) else {
        return;
    };
    let entry = domain.entries.get_mut(*id).expect("the index names live entries");
    if entry.record.is_some() {
        entry.record = Some(record);
        entry.uncertain = None;
    }
}

/// A record write of `item` carrying `nonce` gave up after an attempt that
/// may have landed: what the record now is, is not known.
pub(crate) fn uncertain(domain: &mut Domain, item: Item, nonce: u64) {
    let Some(id) = domain.index.get(&item) else {
        return;
    };
    let entry = domain.entries.get_mut(*id).expect("the index names live entries");
    if entry.record.is_some() {
        entry.uncertain = Some(nonce);
    }
}

/// Where comments of `item` posted from now are looked for: after the last
/// one the working set passed, or from the first.
pub(crate) fn passed(domain: &Domain, item: Item) -> u64 {
    match domain.index.get(&item) {
        Some(id) => domain.entries.get(*id).expect("the index names live entries").announced.comment,
        None => 0,
    }
}
