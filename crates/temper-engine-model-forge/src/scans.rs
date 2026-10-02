//! Keeping up with the forge (engine-model.md, section 12): each
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
//! A pass also has the pull requests whose CI has not settled read again,
//! since statuses move no updated time.
//!
//! Passes follow each other every `Limits::poll`, and sooner after a webhook:
//! `Limits::hinted` after the last began. Polling is the backstop, so a lost
//! webhook costs only latency. An item refused for want of room waits on the
//! forge, carrying its labels; once an item leaves, the next pass lists the
//! labels again, before the changes.
//!
//! Listings page by time where they can: the next page starts at the updated
//! time of the last item of this one, inclusive, and only a page whose items
//! all share one time asks for the next page by number. An item that changes
//! while a listing is paged moves to its end, and costs a duplicate, never a
//! miss; a later pass begins at the newest time seen, inclusive, so what
//! changed in that same second is listed again (see [`crate::items`] for how
//! an item listed again at the same time is told apart). Paging by number can
//! miss an item when another one of its time moves meanwhile: a pass that did
//! both is followed by one from that time again (or, for the labels, by
//! another listing of them).
//!
//! The slow pass lists every open item of a repository, a page every
//! `Limits::slow`, by number, at the lowest priority, and reads each that is
//! not held and carries neither label for a record of the engine's: an item
//! whose tracking label a person removed is found again, and admitted.
//!
//! A listing that fails is tried again after a backoff, from the page it
//! failed on; a slow pass that fails waits for its next page.

use temper_lib::bytes::copy_of;
use temper_lib::{Env, Id, List, Queue, Time};

use crate::api::{self, Answer, Error, Kind, Mark, Op, State as Open, Summary};
use crate::boundary::{Item, Request};
use crate::calls::{self, Call, Purpose};
use crate::facts::{Fact, Priority};
use crate::items;
use crate::limits::Limits;
use crate::model::{Alarm, Model};

/// A repository's listings.
#[derive(Debug)]
pub(crate) struct Scan {
    pass: Pass,
    /// Passes begun.
    passes: u64,
    /// Where the next listing of changes starts: the newest updated time a
    /// pass saw. None until the cold start is done.
    mark: Option<Time>,
    /// When the last pass began, and when the next is due while none runs.
    began: Time,
    due: Time,
    /// A webhook came while a pass ran: the next follows it soon.
    hinted: bool,
    /// An item was refused for want of room: the next pass lists the labels.
    waiting: bool,
    slow: Slow,
    /// The items of the slow pass's page to be read.
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

/// How far a pass is: the page of `listing` asked for, of the items updated
/// at or after `since`, the `page`th; the newest updated time it has seen;
/// the earliest time it paged by number at, and whether an item changed
/// while it ran.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
struct Progress {
    listing: Listing,
    since: Time,
    page: u32,
    newest: Time,
    tied: Option<Time>,
    moved: bool,
}

impl Progress {
    /// The first page of `listing` from `since`.
    const fn first(listing: Listing, since: Time) -> Progress {
        Progress { listing, since, page: 1, newest: since, tied: None, moved: false }
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
    /// Reading the `at`th candidate's comments after `after`; `next` is the
    /// page after this one.
    Probing {
        next: u32,
        at: u32,
        after: u64,
        call: Id<Call>,
    },
}

/// Sets each repository's listings up: its cold start is due at once, and
/// its slow pass one interval in.
pub(crate) fn start(model: &mut Model, limits: &Limits) {
    for repository in 0..limits.repositories {
        let scan = Scan {
            pass: Pass::Idle,
            passes: 0,
            mark: None,
            began: Time::ZERO,
            due: Time::ZERO,
            hinted: false,
            waiting: false,
            slow: Slow::Idle { page: 1 },
            candidates: List::with_capacity(0),
        };
        model.scans.push(scan).expect("a scan per repository");
        model.alarms.arm(Alarm::Poll(repository), Time::ZERO).expect("an alarm per repository fits");
        let slow = Time::ZERO.saturating_add(limits.slow);
        model.alarms.arm(Alarm::Slow(repository), slow).expect("an alarm per repository fits");
    }
}

/// The passes `repository`'s listings have begun.
pub(crate) fn passes(model: &Model, repository: u32) -> u64 {
    scan(model, repository).passes
}

/// An item of `repository` was refused for want of room: once there is
/// room, a pass lists the labels again.
pub(crate) fn wait(model: &mut Model, repository: u32) {
    scan_mut(model, repository).waiting = true;
}

/// Room freed in the working set: the repositories with work waiting list
/// their labels soon.
pub(crate) fn room(model: &mut Model, env: &Env<Limits>) {
    for repository in 0..env.limits.repositories {
        if scan(model, repository).waiting {
            nudge(model, env, repository);
        }
    }
}

/// A webhook: `repository` changed, and the commit `commit` if it says, as
/// a status does: the pull requests on that head are read again. The item a
/// webhook names is not relied on: the next pass finds it.
pub(crate) fn hint(model: &mut Model, env: &Env<Limits>, repository: u32, commit: Option<[u8; 32]>) {
    assert!(repository < env.limits.repositories, "a webhook of the deployment's repositories");
    if let Some(commit) = commit {
        items::poll_ci(model, repository, Some(commit));
    }
    nudge(model, env, repository);
}

/// Brings `repository`'s next pass forward: `Limits::hinted` after the last
/// began, or once the one running ends.
fn nudge(model: &mut Model, env: &Env<Limits>, repository: u32) {
    let scan = scan_mut(model, repository);
    match scan.pass {
        Pass::Idle => {
            let soon = scan.began.saturating_add(env.limits.hinted).max(env.now);
            if soon < scan.due {
                scan.due = soon;
                model.alarms.arm(Alarm::Poll(repository), soon).expect("an alarm per repository fits");
            }
        }
        Pass::Busy { .. } | Pass::Waiting { .. } => scan.hinted = true,
    }
}

/// `repository`'s poll alarm: its next pass begins, or the page that failed
/// is asked for again.
pub(crate) fn poll(model: &mut Model, env: &Env<Limits>, repository: u32) {
    let scan = scan(model, repository);
    match scan.pass {
        Pass::Idle => begin(model, env, repository),
        Pass::Waiting { progress, attempt, until: _ } => ask(model, repository, progress, attempt),
        Pass::Busy { .. } => unreachable!("the poll alarm runs while no listing is asked for"),
    }
}

/// A pass begins: the labels if the cold start is not done or work waits,
/// the changes otherwise; and the pull requests whose CI has not settled are
/// read again.
fn begin(model: &mut Model, env: &Env<Limits>, repository: u32) {
    let room = !model.entries.is_full();
    let scan = scan_mut(model, repository);
    scan.passes = scan.passes.saturating_add(1);
    scan.began = env.now;
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
    model.facts.push(Fact::Listing { repository });
    ask(model, repository, Progress::first(listing, since), 0);
    items::poll_ci(model, repository, None);
}

/// Asks for a page of a listing.
fn ask(model: &mut Model, repository: u32, progress: Progress, attempt: u32) {
    let call = calls::queue(&mut model.calls, Purpose::Listing(repository), Priority::Keep);
    scan_mut(model, repository).pass = Pass::Busy { progress, call, attempt };
}

/// What `repository`'s listing call asks, as it goes out.
pub(crate) fn op(model: &Model, repository: u32) -> (u32, Op) {
    let op = match scan(model, repository).pass {
        Pass::Busy { progress: Progress { listing, since, page, .. }, .. } => match listing {
            Listing::Tracked => Op::Items {
                state: Some(Open::Open),
                kind: None,
                label: Some(copy_of(&model.config.tracking)),
                since,
                page,
            },
            Listing::HandedIn => Op::Items {
                state: Some(Open::Open),
                kind: Some(Kind::Issue),
                label: Some(copy_of(&model.config.hand_in)),
                since,
                page,
            },
            Listing::Changes => Op::Items { state: None, kind: None, label: None, since, page },
        },
        Pass::Idle | Pass::Waiting { .. } => unreachable!("a listing goes out while it is asked for"),
    };
    (repository, op)
}

/// Terminal for `repository`'s listing call: each item listed is taken in
/// turn, and the next page asked for, or the next listing, or the pass ends.
///
/// A pass that paged by number among items of one updated time, and moved
/// meanwhile (an item it listed shows a time from after it began, at the
/// forge's resolution), may have missed one of them, shifted onto a page it
/// had read: its next pass starts from that time again, and a listing of the
/// labels is repeated.
pub(crate) fn answered(
    model: &mut Model,
    env: &Env<Limits>,
    repository: u32,
    result: Result<Answer, Error>,
    out: &mut Queue<Request>,
) {
    let (progress, attempt) = match scan(model, repository).pass {
        Pass::Busy { progress, call: _, attempt } => (progress, attempt),
        Pass::Idle | Pass::Waiting { .. } => unreachable!("a listing's terminal comes while it is asked for"),
    };
    let (items, more) = match result {
        Ok(answer) => api::items(answer),
        Err(error) => {
            failed(model, env, repository, progress, attempt, error);
            return;
        }
    };
    let most = usize::try_from(env.limits.page).expect("a u32 fits in a usize");
    assert!(items.len() <= most, "the protocol layer brings a page at most");
    let scan = scan(model, repository);
    let (pass, began) = (scan.passes, scan.began);
    let mut progress = progress;
    for summary in &items {
        progress.newest = progress.newest.max(summary.updated);
        if summary.updated.saturating_add(env.limits.resolution) > began {
            progress.moved = true;
        }
        take(model, env, repository, summary, pass, out);
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
        ask(model, repository, progress, 0);
        return;
    }
    let missed = match progress.tied {
        Some(tied) if progress.moved => Some(tied),
        Some(_) | None => None,
    };
    let scan = scan_mut(model, repository);
    match progress.listing {
        Listing::Tracked | Listing::HandedIn if missed.is_some() => {
            // An item carrying the label may have been missed: listed again
            // by the next pass.
            scan.waiting = true;
        }
        Listing::Tracked | Listing::HandedIn | Listing::Changes => {}
    }
    match progress.listing {
        Listing::Tracked => ask(model, repository, Progress::first(Listing::HandedIn, Time::ZERO), 0),
        Listing::HandedIn => match scan.mark {
            Some(mark) => ask(model, repository, Progress::first(Listing::Changes, mark), 0),
            None => {
                // The cold start is done: changes are listed from when it
                // began.
                scan.mark = Some(scan.began);
                model.loading.listing = model.loading.listing.saturating_sub(1);
                rest(model, env, repository);
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
            rest(model, env, repository);
        }
    }
}

/// A listing's page failed: asked for again at once for the rate, whose
/// reset holds every call, or after a backoff.
fn failed(model: &mut Model, env: &Env<Limits>, repository: u32, progress: Progress, attempt: u32, error: Error) {
    match error {
        Error::RateLimited { .. } => ask(model, repository, progress, attempt),
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
        | Error::Protected => {
            let attempt = attempt.saturating_add(1);
            let until = env.now.saturating_add(items::backoff(&env.limits, &mut model.rng, attempt));
            scan_mut(model, repository).pass = Pass::Waiting { progress, attempt, until };
            model.alarms.arm(Alarm::Poll(repository), until).expect("an alarm per repository fits");
        }
    }
}

/// The pass ended: the next is due a poll after it began, or soon if a
/// webhook came meanwhile.
fn rest(model: &mut Model, env: &Env<Limits>, repository: u32) {
    let scan = scan_mut(model, repository);
    scan.pass = Pass::Idle;
    let wait = if scan.hinted { env.limits.hinted } else { env.limits.poll };
    scan.due = scan.began.saturating_add(wait).max(env.now);
    let due = scan.due;
    model.alarms.arm(Alarm::Poll(repository), due).expect("an alarm per repository fits");
}

/// An item a listing shows: one held is taken in turn, and so is a pull
/// request linked to one; an open one carrying the tracking label is
/// admitted, if there is room; an open issue carrying the hand-in label is
/// offered.
fn take(model: &mut Model, env: &Env<Limits>, repository: u32, summary: &Summary, pass: u64, out: &mut Queue<Request>) {
    let item = Item { repository, number: summary.number };
    if let Some(&id) = model.pulls.get(&item) {
        items::linked(model, env, id, summary, pass);
    }
    if let Some(&id) = model.index.get(&item) {
        items::listed(model, env, id, summary, pass, out);
        return;
    }
    if summary.state == Open::Closed {
        return;
    }
    if carries(summary, &model.config.tracking) {
        if items::admit(model, env, item, summary.updated).is_none() {
            model.facts.push(Fact::Refused { item });
            scan_mut(model, repository).waiting = true;
        }
    } else if summary.kind == Kind::Issue && carries(summary, &model.config.hand_in) {
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
pub(crate) fn slow(model: &mut Model, repository: u32) {
    let page = match scan(model, repository).slow {
        Slow::Idle { page } => page,
        Slow::Listing { .. } | Slow::Probing { .. } => unreachable!("the slow alarm runs while the slow pass rests"),
    };
    let call = calls::queue(&mut model.calls, Purpose::Slow(repository), Priority::Slow);
    scan_mut(model, repository).slow = Slow::Listing { page, call };
}

/// What `repository`'s slow pass asks, as its call goes out.
pub(crate) fn slow_op(model: &Model, repository: u32) -> (u32, Op) {
    let scan = scan(model, repository);
    let op = match scan.slow {
        Slow::Listing { page, call: _ } => {
            Op::Items { state: Some(Open::Open), kind: None, label: None, since: Time::ZERO, page }
        }
        Slow::Probing { at, after, .. } => {
            let number = *scan.candidates.get(at).expect("a candidate probed is listed");
            Op::Item { number, after }
        }
        Slow::Idle { .. } => unreachable!("the slow pass's call goes out while it asks"),
    };
    (repository, op)
}

/// Terminal for `repository`'s slow pass call: a page's candidates, or what
/// a candidate's comments say.
pub(crate) fn slow_answered(model: &mut Model, env: &Env<Limits>, repository: u32, result: Result<Answer, Error>) {
    match scan(model, repository).slow {
        Slow::Listing { page, call: _ } => slow_listed(model, env, repository, page, result),
        Slow::Probing { next, at, after: _, call: _ } => probed(model, env, repository, next, at, result),
        Slow::Idle { .. } => unreachable!("the slow pass's terminal comes while it asks"),
    }
}

/// The slow pass's page: the open items not held that carry neither label
/// are read, one at a time.
fn slow_listed(model: &mut Model, env: &Env<Limits>, repository: u32, page: u32, result: Result<Answer, Error>) {
    let (items, more) = match result {
        Ok(answer) => api::items(answer),
        Err(_) => {
            slow_rest(model, env, repository, page);
            return;
        }
    };
    let next = if more { page.saturating_add(1) } else { 1 };
    // The last page's candidates go before this one's are kept.
    scan_mut(model, repository).candidates = List::with_capacity(0);
    let mut candidates = List::with_capacity(env.limits.page);
    for summary in &items {
        let item = Item { repository, number: summary.number };
        let labelled = carries(summary, &model.config.tracking) || carries(summary, &model.config.hand_in);
        if summary.state == Open::Open && !labelled && !model.index.contains_key(&item) {
            candidates.push(summary.number).expect("a page of candidates at most");
        }
    }
    scan_mut(model, repository).candidates = candidates;
    probe(model, env, repository, next, 0, 0);
}

/// Reads the `at`th candidate's comments after `after`, or rests if there is
/// none left.
fn probe(model: &mut Model, env: &Env<Limits>, repository: u32, next: u32, at: u32, after: u64) {
    if at >= scan(model, repository).candidates.len() {
        slow_rest(model, env, repository, next);
        return;
    }
    let call = calls::queue(&mut model.calls, Purpose::Slow(repository), Priority::Slow);
    scan_mut(model, repository).slow = Slow::Probing { next, at, after, call };
}

/// A candidate's comments: a record of the engine's admits it.
fn probed(model: &mut Model, env: &Env<Limits>, repository: u32, next: u32, at: u32, result: Result<Answer, Error>) {
    let (summary, comments, more) = match result {
        Ok(answer) => api::item(answer),
        Err(Error::Missing | Error::Forbidden) => {
            probe(model, env, repository, next, at.saturating_add(1), 0);
            return;
        }
        Err(_) => {
            slow_rest(model, env, repository, next);
            return;
        }
    };
    let engine = model.config.engine;
    let mut last = 0;
    for comment in &comments {
        last = comment.id;
        let record = match comment.mark {
            Mark::Record(_) | Mark::Mangled => comment.author == engine,
            Mark::None | Mark::Key(_) => false,
        };
        if record {
            let item = Item { repository, number: summary.number };
            if summary.state == Open::Open {
                // Without room it is found again by a later slow pass.
                let _admitted = items::admit(model, env, item, Time::ZERO);
            }
            probe(model, env, repository, next, at.saturating_add(1), 0);
            return;
        }
    }
    if more {
        probe(model, env, repository, next, at, last);
    } else {
        probe(model, env, repository, next, at.saturating_add(1), 0);
    }
}

/// The slow pass rests until its next page is due.
fn slow_rest(model: &mut Model, env: &Env<Limits>, repository: u32, page: u32) {
    scan_mut(model, repository).slow = Slow::Idle { page };
    let at = env.now.saturating_add(env.limits.slow);
    model.alarms.arm(Alarm::Slow(repository), at).expect("an alarm per repository fits");
}

fn scan(model: &Model, repository: u32) -> &Scan {
    model.scans.get(repository).expect("a scan per repository")
}

fn scan_mut(model: &mut Model, repository: u32) -> &mut Scan {
    model.scans.get_mut(repository).expect("a scan per repository")
}
