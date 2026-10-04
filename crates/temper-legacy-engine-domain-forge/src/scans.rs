//! Keeping up with the forge (engine-domain.md, section 12): each
//! repository's listings, and its slow pass.
//!
//! A repository's first pass is the cold start: it lists the open items
//! carrying the tracking label, admitting each into the working set to be read
//! for its record, then the open issues carrying the hand-in label, offering
//! each to the parent. Every pass after it lists what changed since the
//! newest updated time the last one saw, a page at a time: an item held is
//! read again if it changed, or leaves if it closed; a pull request linked to
//! one is read again; an open item carrying the tracking label that is not
//! held is admitted; an open issue carrying the hand-in label is offered. So
//! the cost of keeping up follows the rate of change, not the number of items.
//! A pass also has the pull requests of the items held read again on a
//! backoff of their own, since CI and a moving base move no updated time
//! ([`crate::items`]).
//!
//! Passes follow each other every `Limits::poll`, and sooner after a webhook:
//! `Limits::hinted` after the last began. Polling is the backstop, so a lost
//! webhook costs only latency. An item refused for want of room waits on the
//! forge, carrying its labels; once an item leaves, the next pass lists the
//! labels again, before the changes.
//!
//! Every time a listing compares is the forge's: the items' updated times,
//! and when each page was made, which the forge says. The cold start lists
//! changes from when its first page was made; a later pass from the newest
//! updated time it saw.
//!
//! Listings page by time where they can: the next page starts at the updated
//! time of the last item of this one, inclusive, and only a page whose items
//! all share one time asks for the next page by number. An item that changes
//! while a listing is paged moves to its end, and costs a duplicate, never a
//! miss; a later pass begins at the newest time seen, inclusive, so what
//! changed in that same second is listed again (see [`crate::items`] for how
//! an item listed again at the same time is told apart). Paging by number can
//! miss an item when another one of its time moves meanwhile: a listing that
//! did both, an item it showed being updated no earlier than its first page
//! was made, is followed by one from that time again (or, for the labels, by
//! another listing of them).
//!
//! The slow pass lists every open item of a repository, a page every
//! `Limits::slow`, by number, at the lowest priority, in cycles. Of each
//! page, it reads at most `Limits::probes` of the items not held that carry
//! neither label, a different few each cycle, for a record of the engine's:
//! an item whose tracking label a person removed is found again, and
//! admitted. At the end of a cycle, the items held that none of its pages
//! showed open are read, to find whether they are still there.
//!
//! A listing that fails is tried again after a backoff, from the page it
//! failed on; a slow pass that fails waits for its next page.

use skein_lib::bytes::copy_of;
use skein_lib::{Env, Id, List, Queue, Time};

use crate::api::{self, Answer, Error, Kind, Mark, Op, State as Open, Summary};
use crate::boundary::{Item, Request};
use crate::calls::{self, Call, Purpose};
use crate::domain::{Alarm, Domain};
use crate::facts::{Fact, Priority};
use crate::items::{self, Seen};
use crate::limits::Limits;

/// A repository's listings.
#[derive(Debug)]
pub(crate) struct Scan {
    pass: Pass,
    /// Passes begun.
    passes: u64,
    /// Where the next listing of changes starts: the newest updated time a
    /// pass saw. None until the cold start is done.
    mark: Option<Time>,
    /// When the forge made the first page of the pass running, and the
    /// newest time it said a page was made at: the forge's both.
    begun: Option<Time>,
    clock: Time,
    /// When the last pass began, and when the next is due while none runs.
    began: Time,
    due: Time,
    /// A webhook came while a pass ran: the next follows it soon.
    hinted: bool,
    /// An item was refused for want of room: the next pass lists the labels.
    waiting: bool,
    slow: Slow,
    /// The slow pass's cycles begun, from one.
    cycle: u64,
    /// The items of the slow pass's page that may be read.
    candidates: List<u64>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Pass {
    /// None runs: the next is due at `Scan::due`.
    Idle,
    /// A page is asked for.
    Busy { progress: Progress, call: Id<Call>, attempt: u32 },
    /// The last page failed: asked for again at `until`.
    Waiting { progress: Progress, attempt: u32, until: Time },
}

/// How far a listing is: the page of `listing` asked for, of the items
/// updated at or after `since`, the `page`th; the newest updated time it has
/// seen; when its first page was made; the earliest time it paged by number
/// at, and whether an item changed while it ran.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
struct Progress {
    listing: Listing,
    since: Time,
    page: u32,
    newest: Time,
    started: Option<Time>,
    tied: Option<Time>,
    moved: bool,
}

impl Progress {
    /// The first page of `listing` from `since`.
    const fn first(listing: Listing, since: Time) -> Progress {
        Progress { listing, since, page: 1, newest: since, started: None, tied: None, moved: false }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Listing {
    /// Open items carrying the tracking label.
    Tracked,
    /// Open issues carrying the hand-in label.
    HandedIn,
    /// Every item that changed.
    Changes,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Slow {
    /// The `page`th page is next, when its alarm fires.
    Idle {
        page: u32,
    },
    Listing {
        page: u32,
        call: Id<Call>,
    },
    /// Reading the `at`th of the candidates probed, from `from`, the
    /// comments after `after`; `next` is the page after this one.
    Probing {
        next: u32,
        from: u32,
        at: u32,
        after: u64,
        call: Id<Call>,
    },
}

/// Sets each repository's listings up: its cold start is due at once, and
/// its slow pass one interval in.
pub(crate) fn start(domain: &mut Domain, limits: &Limits) {
    for repository in 0..limits.repositories {
        let scan = Scan {
            pass: Pass::Idle,
            passes: 0,
            mark: None,
            begun: None,
            clock: Time::ZERO,
            began: Time::ZERO,
            due: Time::ZERO,
            hinted: false,
            waiting: false,
            slow: Slow::Idle { page: 1 },
            cycle: 1,
            candidates: List::with_capacity(limits.page),
        };
        domain.scans.push(scan).expect("a scan per repository");
        domain.alarms.arm(Alarm::Poll(repository), Time::ZERO).expect("an alarm per repository fits");
        let slow = Time::ZERO.saturating_add(limits.slow);
        domain.alarms.arm(Alarm::Slow(repository), slow).expect("an alarm per repository fits");
    }
}

/// The newest time the forge said `repository`'s pages were made at: what is
/// made on the forge from now on is updated no earlier.
pub(crate) fn clock(domain: &Domain, repository: u32) -> Time {
    scan(domain, repository).clock
}

/// The slow pass's cycle over `repository` running.
pub(crate) fn cycle(domain: &Domain, repository: u32) -> u64 {
    scan(domain, repository).cycle
}

/// An item of `repository` was refused for want of room: once there is
/// room, a pass lists the labels again.
pub(crate) fn wait(domain: &mut Domain, repository: u32) {
    scan_mut(domain, repository).waiting = true;
}

/// Room freed in the working set: the repositories with work waiting list
/// their labels soon.
pub(crate) fn room(domain: &mut Domain, env: &Env<Limits>) {
    for repository in 0..env.limits.repositories {
        if scan(domain, repository).waiting {
            nudge(domain, env, repository);
        }
    }
}

/// A webhook: `repository` changed, and the commit `commit` or the branch
/// `branch` if it names one, as a status or a push does: the pull requests
/// on that head, or into that base, are read again. The item a webhook names
/// is not relied on: the next pass finds it.
pub(crate) fn hint(
    domain: &mut Domain,
    env: &Env<Limits>,
    repository: u32,
    commit: Option<[u8; 32]>,
    branch: Option<&[u8]>,
) {
    assert!(repository < env.limits.repositories, "a webhook of the deployment's repositories");
    if commit.is_some() || branch.is_some() {
        items::hinted(domain, repository, commit, branch);
    }
    nudge(domain, env, repository);
}

/// Brings `repository`'s next pass forward: `Limits::hinted` after the last
/// began, or once the one running ends.
fn nudge(domain: &mut Domain, env: &Env<Limits>, repository: u32) {
    let scan = scan_mut(domain, repository);
    match scan.pass {
        Pass::Idle => {
            let soon = scan.began.saturating_add(env.limits.hinted).max(env.now);
            if soon < scan.due {
                scan.due = soon;
                domain.alarms.arm(Alarm::Poll(repository), soon).expect("an alarm per repository fits");
            }
        }
        Pass::Busy { .. } | Pass::Waiting { .. } => scan.hinted = true,
    }
}

/// `repository`'s poll alarm: its next pass begins, or the page that failed
/// is asked for again.
pub(crate) fn poll(domain: &mut Domain, env: &Env<Limits>, repository: u32) {
    let scan = scan(domain, repository);
    match scan.pass {
        Pass::Idle => begin(domain, env, repository),
        Pass::Waiting { progress, attempt, until: _ } => ask(domain, repository, progress, attempt),
        Pass::Busy { .. } => unreachable!("the poll alarm runs while no listing is asked for"),
    }
}

/// A pass begins: the labels if the cold start is not done or work waits,
/// the changes otherwise; and the pull requests whose backoff passed are
/// read again.
fn begin(domain: &mut Domain, env: &Env<Limits>, repository: u32) {
    let room = !domain.entries.is_full();
    let scan = scan_mut(domain, repository);
    scan.passes = scan.passes.saturating_add(1);
    scan.began = env.now;
    scan.begun = None;
    scan.hinted = false;
    // Work waiting at the entrance is listed again once there is room.
    let rescan = scan.waiting && room;
    if rescan {
        scan.waiting = false;
    }
    let (listing, since) = match scan.mark {
        Some(mark) if !rescan => (Listing::Changes, mark),
        Some(_) | None => (Listing::Tracked, Time::ZERO),
    };
    domain.facts.push(Fact::Listing { repository });
    ask(domain, repository, Progress::first(listing, since), 0);
    items::poll(domain, env, repository);
}

/// Asks for a page of a listing.
fn ask(domain: &mut Domain, repository: u32, progress: Progress, attempt: u32) {
    let call = calls::queue(&mut domain.calls, Purpose::Listing(repository), Priority::Keep);
    scan_mut(domain, repository).pass = Pass::Busy { progress, call, attempt };
}

/// What `repository`'s listing call asks, as it goes out.
pub(crate) fn op(domain: &Domain, repository: u32) -> (u32, Op) {
    let op = match scan(domain, repository).pass {
        Pass::Busy { progress: Progress { listing, since, page, .. }, .. } => match listing {
            Listing::Tracked => Op::Items {
                state: Some(Open::Open),
                kind: None,
                label: Some(copy_of(&domain.config.tracking)),
                author: None,
                since,
                page,
            },
            Listing::HandedIn => Op::Items {
                state: Some(Open::Open),
                kind: Some(Kind::Issue),
                label: Some(copy_of(&domain.config.hand_in)),
                author: None,
                since,
                page,
            },
            Listing::Changes => Op::Items { state: None, kind: None, label: None, author: None, since, page },
        },
        Pass::Idle | Pass::Waiting { .. } => unreachable!("a listing goes out while it is asked for"),
    };
    (repository, op)
}

/// Terminal for `repository`'s listing call: each item listed is taken in
/// turn, and the next page asked for, or the next listing, or the pass ends.
///
/// A listing that paged by number among items of one updated time, and moved
/// meanwhile (an item it listed shows a time no earlier than when its first
/// page was made, the forge's both), may have missed one of them, shifted
/// onto a page it had read: its next pass starts from that time again, and a
/// listing of the labels is repeated.
pub(crate) fn answered(
    domain: &mut Domain,
    env: &Env<Limits>,
    repository: u32,
    result: Result<Answer, Error>,
    out: &mut Queue<Request>,
) {
    let (progress, attempt) = match scan(domain, repository).pass {
        Pass::Busy { progress, call: _, attempt } => (progress, attempt),
        Pass::Idle | Pass::Waiting { .. } => unreachable!("a listing's terminal comes while it is asked for"),
    };
    let (items, more, now) = match result {
        Ok(answer) => api::items(answer),
        Err(error) => {
            failed(domain, env, repository, progress, attempt, error);
            return;
        }
    };
    let most = usize::try_from(env.limits.page).expect("a u32 fits in a usize");
    let items = items.get(..most).unwrap_or(&items);
    let scan = scan_mut(domain, repository);
    scan.clock = scan.clock.max(now);
    if scan.begun.is_none() {
        scan.begun = Some(now);
    }
    let mut progress = progress;
    let started = match progress.started {
        Some(started) => started,
        None => now,
    };
    progress.started = Some(started);
    for summary in items {
        progress.newest = progress.newest.max(summary.updated);
        if summary.updated >= started {
            progress.moved = true;
        }
        take(domain, env, repository, summary, now, out);
    }
    if more {
        progress = match items.last() {
            Some(last) if last.updated > progress.since => Progress { since: last.updated, page: 1, ..progress },
            Some(_) | None => {
                let tied = match progress.tied {
                    Some(tied) => tied,
                    None => progress.since,
                };
                Progress { page: progress.page.saturating_add(1), tied: Some(tied), ..progress }
            }
        };
        ask(domain, repository, progress, 0);
        return;
    }
    let missed = match progress.tied {
        Some(tied) if progress.moved => Some(tied),
        Some(_) | None => None,
    };
    let scan = scan_mut(domain, repository);
    match progress.listing {
        Listing::Tracked | Listing::HandedIn if missed.is_some() => {
            // An item carrying the label may have been missed: listed again
            // by the next pass.
            scan.waiting = true;
        }
        Listing::Tracked | Listing::HandedIn | Listing::Changes => {}
    }
    match progress.listing {
        Listing::Tracked => ask(domain, repository, Progress::first(Listing::HandedIn, Time::ZERO), 0),
        Listing::HandedIn => match scan.mark {
            Some(mark) => ask(domain, repository, Progress::first(Listing::Changes, mark), 0),
            None => {
                // The cold start is done: changes are listed from when its
                // first page was made.
                scan.mark = Some(match scan.begun {
                    Some(begun) => begun,
                    None => Time::ZERO,
                });
                domain.loading.listing = domain.loading.listing.saturating_sub(1);
                rest(domain, env, repository);
            }
        },
        Listing::Changes => {
            let Some(mark) = scan.mark else {
                unreachable!("changes are listed once the cold start is done");
            };
            scan.mark = match missed {
                Some(tied) => Some(tied),
                None => Some(mark.max(progress.newest)),
            };
            rest(domain, env, repository);
        }
    }
}

/// A listing's page failed: asked for again at once for the rate, whose
/// reset holds every call, or after a backoff.
fn failed(domain: &mut Domain, env: &Env<Limits>, repository: u32, progress: Progress, attempt: u32, error: Error) {
    match error {
        Error::RateLimited { .. } => ask(domain, repository, progress, attempt),
        Error::Unavailable
        | Error::Timeout
        | Error::Forbidden
        | Error::Missing
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
            let until = env.now.saturating_add(items::backoff(&env.limits, &mut domain.rng, attempt));
            scan_mut(domain, repository).pass = Pass::Waiting { progress, attempt, until };
            domain.alarms.arm(Alarm::Poll(repository), until).expect("an alarm per repository fits");
        }
    }
}

/// The pass ended: the next is due a poll after it began, or soon if a
/// webhook came meanwhile.
fn rest(domain: &mut Domain, env: &Env<Limits>, repository: u32) {
    let scan = scan_mut(domain, repository);
    scan.pass = Pass::Idle;
    let wait = if scan.hinted { env.limits.hinted } else { env.limits.poll };
    scan.due = scan.began.saturating_add(wait).max(env.now);
    let due = scan.due;
    domain.alarms.arm(Alarm::Poll(repository), due).expect("an alarm per repository fits");
}

/// An item a listing made at `now` shows: one held is taken in turn, and so
/// is a pull request linked to one; an open one carrying the tracking label
/// is admitted, if there is room; an open issue carrying the hand-in label
/// is offered.
fn take(
    domain: &mut Domain,
    env: &Env<Limits>,
    repository: u32,
    summary: &Summary,
    now: Time,
    out: &mut Queue<Request>,
) {
    let item = Item { repository, number: summary.number };
    if let Some(&id) = domain.pulls.get(&item) {
        items::linked(domain, env, id, summary, now);
    }
    if let Some(&id) = domain.index.get(&item) {
        items::listed(domain, env, id, summary, now, out);
        return;
    }
    if summary.state == Open::Closed {
        return;
    }
    if carries(summary, &domain.config.tracking) {
        let seen = Seen::listed(summary.updated, now, env.limits.resolution);
        if items::admit(domain, env, item, seen).is_none() {
            domain.facts.push(Fact::Refused { item });
            scan_mut(domain, repository).waiting = true;
        }
    } else if summary.kind == Kind::Issue && carries(summary, &domain.config.hand_in) {
        out.push(Request::Offered { item });
    }
}

/// Whether `summary` carries `label`.
fn carries(summary: &Summary, label: &[u8]) -> bool {
    for carried in &summary.labels {
        if **carried == *label {
            return true;
        }
    }
    false
}

/// `repository`'s slow alarm: the slow pass lists its next page.
pub(crate) fn slow(domain: &mut Domain, repository: u32) {
    let page = match scan(domain, repository).slow {
        Slow::Idle { page } => page,
        Slow::Listing { .. } | Slow::Probing { .. } => unreachable!("the slow alarm runs while the slow pass rests"),
    };
    let call = calls::queue(&mut domain.calls, Purpose::Slow(repository), Priority::Slow);
    scan_mut(domain, repository).slow = Slow::Listing { page, call };
}

/// What `repository`'s slow pass asks, as its call goes out.
pub(crate) fn slow_op(domain: &Domain, repository: u32) -> (u32, Op) {
    let scan = scan(domain, repository);
    let op = match scan.slow {
        Slow::Listing { page, call: _ } => {
            Op::Items { state: Some(Open::Open), kind: None, label: None, author: None, since: Time::ZERO, page }
        }
        Slow::Probing { from, at, after, .. } => Op::Item { number: candidate(scan, from, at), after },
        Slow::Idle { .. } => unreachable!("the slow pass's call goes out while it asks"),
    };
    (repository, op)
}

/// The `at`th candidate probed of a page, from `from`: a different few each
/// cycle.
fn candidate(scan: &Scan, from: u32, at: u32) -> u64 {
    let count = scan.candidates.len();
    let index = from.saturating_add(at).checked_rem(count).expect("a candidate probed is listed");
    *scan.candidates.get(index).expect("a candidate probed is listed")
}

/// Terminal for `repository`'s slow pass call: a page's candidates, or what
/// a candidate's comments say.
pub(crate) fn slow_answered(domain: &mut Domain, env: &Env<Limits>, repository: u32, result: Result<Answer, Error>) {
    match scan(domain, repository).slow {
        Slow::Listing { page, call: _ } => slow_listed(domain, env, repository, page, result),
        Slow::Probing { next, from, at, after, call: _ } => {
            probed(domain, env, repository, next, from, at, after, result);
        }
        Slow::Idle { .. } => unreachable!("the slow pass's terminal comes while it asks"),
    }
}

/// The slow pass's page: the items held it shows are marked seen this
/// cycle; of the open items not held that carry neither label, a few are
/// read, one at a time. Its last page ends the cycle.
fn slow_listed(domain: &mut Domain, env: &Env<Limits>, repository: u32, page: u32, result: Result<Answer, Error>) {
    let (items, more, _) = match result {
        Ok(answer) => api::items(answer),
        Err(Error::RateLimited { .. }) => {
            let call = calls::queue(&mut domain.calls, Purpose::Slow(repository), Priority::Slow);
            scan_mut(domain, repository).slow = Slow::Listing { page, call };
            return;
        }
        Err(
            Error::Unavailable
            | Error::Timeout
            | Error::Forbidden
            | Error::Missing
            | Error::TooLarge
            | Error::Empty
            | Error::Full
            | Error::Exists
            | Error::NothingToMerge
            | Error::Closed
            | Error::Stale
            | Error::Conflict
            | Error::Protected
            | Error::Circular,
        ) => {
            slow_rest(domain, env, repository, page);
            return;
        }
    };
    let most = usize::try_from(env.limits.page).expect("a u32 fits in a usize");
    let items = items.get(..most).unwrap_or(&items);
    let cycle = scan(domain, repository).cycle;
    scan_mut(domain, repository).candidates.clear();
    for summary in items {
        let item = Item { repository, number: summary.number };
        if let Some(&id) = domain.index.get(&item) {
            if summary.state == Open::Open {
                items::slow_listed(domain, id, cycle);
            }
            continue;
        }
        let labelled = carries(summary, &domain.config.tracking) || carries(summary, &domain.config.hand_in);
        if summary.state == Open::Open && !labelled {
            let _kept = scan_mut(domain, repository).candidates.push(summary.number);
        }
    }
    let next = if more { page.saturating_add(1) } else { 1 };
    if !more {
        // The cycle ends: what it held and never showed is read.
        items::unlisted(domain, repository, cycle);
        scan_mut(domain, repository).cycle = cycle.saturating_add(1);
    }
    let count = scan(domain, repository).candidates.len();
    let from = match u32::try_from(cycle.wrapping_mul(u64::from(env.limits.probes))) {
        Ok(start) => start.checked_rem(count).unwrap_or(0),
        Err(_) => 0,
    };
    probe(domain, env, repository, next, from, 0, 0);
}

/// Reads the `at`th candidate probed from `from`, its comments after
/// `after`, or rests if the probes of this page are done.
fn probe(domain: &mut Domain, env: &Env<Limits>, repository: u32, next: u32, from: u32, at: u32, after: u64) {
    let count = scan(domain, repository).candidates.len();
    if at >= count.min(env.limits.probes) {
        slow_rest(domain, env, repository, next);
        return;
    }
    let call = calls::queue(&mut domain.calls, Purpose::Slow(repository), Priority::Slow);
    scan_mut(domain, repository).slow = Slow::Probing { next, from, at, after, call };
}

/// A candidate's comments: a record of the engine's admits it.
#[expect(clippy::too_many_arguments, reason = "a cell handler over the fields it touches")]
fn probed(
    domain: &mut Domain,
    env: &Env<Limits>,
    repository: u32,
    next: u32,
    from: u32,
    at: u32,
    after: u64,
    result: Result<Answer, Error>,
) {
    let (summary, comments, more) = match result {
        Ok(answer) => api::item(answer),
        Err(Error::RateLimited { .. }) => {
            probe(domain, env, repository, next, from, at, after);
            return;
        }
        Err(Error::Missing | Error::Forbidden) => {
            probe(domain, env, repository, next, from, at.saturating_add(1), 0);
            return;
        }
        Err(
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
            | Error::Protected
            | Error::Circular,
        ) => {
            slow_rest(domain, env, repository, next);
            return;
        }
    };
    let engine = domain.config.engine;
    let most = usize::try_from(env.limits.page).expect("a u32 fits in a usize");
    let mut last = after;
    for comment in comments.get(..most).unwrap_or(&comments) {
        last = last.max(comment.id);
        let record = match comment.mark {
            Mark::Record { .. } | Mark::Mangled => comment.author == engine,
            Mark::None | Mark::Key { .. } => false,
        };
        if record {
            let item = Item { repository, number: summary.number };
            if summary.state == Open::Open {
                // Without room it is found again by a later slow pass.
                let _admitted = items::admit(domain, env, item, Seen::NONE);
            }
            probe(domain, env, repository, next, from, at.saturating_add(1), 0);
            return;
        }
    }
    if more && last > after {
        probe(domain, env, repository, next, from, at, last);
    } else {
        probe(domain, env, repository, next, from, at.saturating_add(1), 0);
    }
}

/// The slow pass rests until its next page is due.
fn slow_rest(domain: &mut Domain, env: &Env<Limits>, repository: u32, page: u32) {
    scan_mut(domain, repository).slow = Slow::Idle { page };
    let at = env.now.saturating_add(env.limits.slow);
    domain.alarms.arm(Alarm::Slow(repository), at).expect("an alarm per repository fits");
}

fn scan(domain: &Domain, repository: u32) -> &Scan {
    domain.scans.get(repository).expect("a scan per repository")
}

fn scan_mut(domain: &mut Domain, repository: u32) -> &mut Scan {
    domain.scans.get_mut(repository).expect("a scan per repository")
}
