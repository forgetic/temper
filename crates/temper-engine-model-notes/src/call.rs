//! A call in flight, from the parent's event to its one answer.
//!
//! | Call | Admitted | Then | Answered |
//! |---|---|---|---|
//! | `Index`, `Search` | its scopes kept and pinned | waits for each scope's first pass | from the indexes, at once or once woken |
//! | `Recall`, by name | | reads the page | with the page as it was read |
//! | `Recall`, by search | its scopes kept and pinned | waits for each scope's first pass, then finds what to read in the indexes, and reads each page in turn | with the pages as they were read |
//! | `Note` | its scope kept and pinned | waits its turn in the scope, then writes | with how the write went, the index having learned it |
//!
//! A call is refused at the entrance, before anything changes, when there
//! is no room for it or for its scopes, or when a name, a query or a page
//! is past the limits. A recall reads the wiki afresh, never the index: its
//! answer holds each page as it was when the wiki served it.

use alloc::boxed::Box;
use core::mem;

use temper_lib::{Env, Id, List, Queue, ReplyTo, bytes};

use crate::boundary::{
    Author, Change, Entry, Fetched, Line, Noted, Page, Recall, Reference, Refusal, Request, Scope, Scopes, Wrote,
};
use crate::facts::Fact;
use crate::kept::{self, Busy, Kept, Known, Pass};
use crate::limits::Limits;
use crate::model::{self, Model, Op};

/// A call in flight: how many scopes' first passes it waits for, and where
/// it stands, with whom to answer.
#[derive(Debug)]
pub(crate) struct Call {
    waits: u32,
    kind: Kind,
}

#[derive(Debug)]
enum Kind {
    Index {
        reply_to: ReplyTo,
        scopes: Scopes,
        budget: u32,
    },
    Search {
        reply_to: ReplyTo,
        scopes: Scopes,
        query: Box<[u8]>,
        most: u32,
    },
    /// A recall by search, finding what to read once its scopes are read.
    Finding {
        reply_to: ReplyTo,
        scopes: Scopes,
        query: Box<[u8]>,
        most: u32,
    },
    Reading {
        reply_to: ReplyTo,
        reading: Reading,
    },
    /// A note waiting its turn in its scope, then written.
    Queued {
        reply_to: ReplyTo,
        scope: Scope,
        name: Box<[u8]>,
        change: Change,
    },
    /// A revision, its page being read afresh, to write `page` over it only
    /// if it is still at `revision`.
    Checking {
        reply_to: ReplyTo,
        scope: Scope,
        name: Box<[u8]>,
        page: Page,
        revision: u64,
    },
    Sent {
        reply_to: ReplyTo,
        scope: Scope,
        name: Box<[u8]>,
        line: Option<Learned>,
    },
    /// Answered: the terminal state, until the reclaim point.
    Answered,
}

/// A recall reading its pages in turn.
#[derive(Debug)]
struct Reading {
    /// The scopes it pinned, if it searched them.
    scopes: Option<Scopes>,
    wanted: List<Wanted>,
    /// How many of `wanted` it has asked for.
    next: u32,
    entries: List<Entry>,
    failed: u32,
}

/// A page a recall reads.
#[derive(Debug)]
pub(crate) struct Wanted {
    scope: Scope,
    name: Box<[u8]>,
}

/// What the index learns of a page written: its line.
#[derive(Debug)]
struct Learned {
    description: Box<[u8]>,
    author: Author,
    references: Box<[Reference]>,
}

/// The index of the notes in `scopes`, within `budget`.
pub(crate) fn index(
    model: &mut Model,
    env: &Env<Limits>,
    reply_to: ReplyTo,
    scopes: Scopes,
    budget: u32,
    out: &mut Queue<Request>,
) {
    admit(model, env, scopes, Kind::Index { reply_to, scopes, budget }, out);
}

/// The lines of `scopes` whose descriptions hold `query`.
pub(crate) fn search(
    model: &mut Model,
    env: &Env<Limits>,
    reply_to: ReplyTo,
    scopes: Scopes,
    query: Box<[u8]>,
    most: u32,
    out: &mut Queue<Request>,
) {
    if !kept::fits_bytes(&query, env.limits.description_bytes) {
        refuse(model, reply_to, Refusal::Oversized, out);
        return;
    }
    let most = most.min(env.limits.lines);
    admit(model, env, scopes, Kind::Search { reply_to, scopes, query, most }, out);
}

/// A run's recall: a page by name, read at once; or those a search of its
/// scopes finds, once they are read.
pub(crate) fn recall(
    model: &mut Model,
    env: &Env<Limits>,
    reply_to: ReplyTo,
    recall: Recall,
    out: &mut Queue<Request>,
) {
    match recall {
        Recall::Name { scope, name } => {
            if !kept::named(&name, &env.limits) {
                refuse(model, reply_to, Refusal::Oversized, out);
                return;
            }
            if model.calls.is_full() {
                refuse(model, reply_to, Refusal::Busy, out);
                return;
            }
            let mut wanted = List::with_capacity(1);
            let pushed = wanted.push(Wanted { scope, name });
            assert!(pushed.is_ok(), "room for the one page");
            let reading = Reading { scopes: None, wanted, next: 0, entries: List::with_capacity(1), failed: 0 };
            let call = Call { waits: 0, kind: Kind::Reading { reply_to, reading } };
            let id = model.calls.insert(call).expect("room was checked");
            read(model, id, out);
        }
        Recall::Search { scopes, query, most } => {
            if !kept::fits_bytes(&query, env.limits.description_bytes) {
                refuse(model, reply_to, Refusal::Oversized, out);
                return;
            }
            let most = most.min(env.limits.recalled);
            admit(model, env, scopes, Kind::Finding { reply_to, scopes, query, most }, out);
        }
    }
}

/// A `note` write: it waits its turn in its scope, which is kept for it.
pub(crate) fn note(
    model: &mut Model,
    env: &Env<Limits>,
    reply_to: ReplyTo,
    scope: Scope,
    name: Box<[u8]>,
    change: Change,
    out: &mut Queue<Request>,
) {
    let fits = match &change {
        Change::New(page) | Change::Revise { page, .. } => kept::fits(page, &env.limits),
        Change::Remove => true,
    };
    if !fits || !kept::named(&name, &env.limits) {
        refuse(model, reply_to, Refusal::Oversized, out);
        return;
    }
    if model.calls.is_full() {
        refuse(model, reply_to, Refusal::Busy, out);
        return;
    }
    if let Err(refusal) = kept::keep(model, env, &[scope], out) {
        refuse(model, reply_to, refusal, out);
        return;
    }
    let call = Call { waits: 0, kind: Kind::Queued { reply_to, scope, name, change } };
    let id = model.calls.insert(call).expect("room was checked");
    let kept_id = *model.scopes.get(&scope).expect("kept above");
    let kept = model.kept.get_mut(kept_id).expect("a scope kept is in the slab");
    kept.writes.push(id);
    kept::follow(model, kept_id, out);
}

/// Admits a call that needs `scopes` kept, and answers it at once if they
/// have all been read once; otherwise it waits for them.
fn admit(model: &mut Model, env: &Env<Limits>, scopes: Scopes, kind: Kind, out: &mut Queue<Request>) {
    let order = order(scopes);
    let order = order.as_slice();
    let refusal = if model.calls.is_full() { Err(Refusal::Busy) } else { kept::keep(model, env, order, out) };
    if let Err(refusal) = refusal {
        let reply_to = match kind {
            Kind::Index { reply_to, .. } | Kind::Search { reply_to, .. } | Kind::Finding { reply_to, .. } => reply_to,
            Kind::Reading { .. } | Kind::Queued { .. } | Kind::Checking { .. } | Kind::Sent { .. } | Kind::Answered => {
                unreachable!("only an index, a search or a recall by search waits for its scopes")
            }
        };
        refuse(model, reply_to, refusal, out);
        return;
    }
    let id = model.calls.insert(Call { waits: 0, kind }).expect("room was checked");
    let mut waits: u32 = 0;
    for scope in order {
        let kept_id = *model.scopes.get(scope).expect("kept above");
        let kept = model.kept.get_mut(kept_id).expect("a scope kept is in the slab");
        let waiting = match kept.pass {
            Pass::Listing | Pass::Reading(_) => true,
            Pass::Ended => false,
        };
        if waiting {
            kept.waiting.push(id);
            waits = waits.saturating_add(1);
        }
    }
    if waits == 0 {
        go_on(model, env, id, out);
    } else {
        model.calls.get_mut(id).expect("admitted above").waits = waits;
    }
}

/// One of the scopes `call` waits for has been read once: once they all
/// have, it is ready to go on.
pub(crate) fn wake(model: &mut Model, call: Id<Call>) {
    let entry = model.calls.get_mut(call).expect("a call waiting is in flight");
    entry.waits = entry.waits.checked_sub(1).expect("a call is woken once for each scope it waits for");
    if entry.waits == 0 {
        model.ready.push(call);
    }
}

/// Goes on with a call that was ready.
pub(crate) fn resume(model: &mut Model, env: &Env<Limits>, call: Id<Call>, out: &mut Queue<Request>) {
    go_on(model, env, call, out);
}

/// Goes on with `call`, its scopes all read once: answers an index or a
/// search, and starts a recall's reads.
fn go_on(model: &mut Model, env: &Env<Limits>, id: Id<Call>, out: &mut Queue<Request>) {
    let call = model.calls.get_mut(id).expect("a call ready is in flight");
    let kind = mem::replace(&mut call.kind, Kind::Answered);
    match kind {
        Kind::Index { reply_to, scopes, budget } => {
            let (lines, more) = indexed(model, env, scopes, budget);
            let unread = unread(model, scopes);
            answer(model, id, Some(scopes), Request::Indexed { reply_to, lines, more, unread }, out);
        }
        Kind::Search { reply_to, scopes, query, most } => {
            let (lines, more) = found(model, scopes, &query, most);
            let unread = unread(model, scopes);
            answer(model, id, Some(scopes), Request::Found { reply_to, lines, more, unread }, out);
        }
        Kind::Finding { reply_to, scopes, query, most } => {
            let wanted = wanted(model, scopes, &query, most);
            let entries = List::with_capacity(wanted.len());
            let reading = Reading { scopes: Some(scopes), wanted, next: 0, entries, failed: 0 };
            model.calls.get_mut(id).expect("in flight").kind = Kind::Reading { reply_to, reading };
            read(model, id, out);
        }
        Kind::Reading { .. } | Kind::Queued { .. } | Kind::Checking { .. } | Kind::Sent { .. } | Kind::Answered => {
            unreachable!("only a call waiting for its scopes goes on")
        }
    }
}

/// Asks for the next page `call` wants, or answers with those it read.
fn read(model: &mut Model, id: Id<Call>, out: &mut Queue<Request>) {
    let call = model.calls.get_mut(id).expect("a recall is in flight");
    let reading = match &mut call.kind {
        Kind::Reading { reading, .. } => reading,
        Kind::Index { .. }
        | Kind::Search { .. }
        | Kind::Finding { .. }
        | Kind::Queued { .. }
        | Kind::Checking { .. }
        | Kind::Sent { .. }
        | Kind::Answered => unreachable!("only a recall reads"),
    };
    if let Some(wanted) = reading.wanted.get(reading.next) {
        let (scope, name) = (wanted.scope, wanted.name.clone());
        reading.next = reading.next.saturating_add(1);
        let op = model::start(model, Op::Recall(id));
        out.push(Request::Fetch { owner: op.token(), scope, name });
        return;
    }
    let (reply_to, Reading { scopes, entries, failed, .. }) = match mem::replace(&mut call.kind, Kind::Answered) {
        Kind::Reading { reply_to, reading } => (reply_to, reading),
        Kind::Index { .. }
        | Kind::Search { .. }
        | Kind::Finding { .. }
        | Kind::Queued { .. }
        | Kind::Checking { .. }
        | Kind::Sent { .. }
        | Kind::Answered => unreachable!("matched above"),
    };
    answer(model, id, scopes, Request::Recalled { reply_to, entries: entries.into_boxed(), failed }, out);
}

/// The read of a page a recall wanted ended: it keeps the page, as the wiki
/// served it, and goes on.
pub(crate) fn fetched(model: &mut Model, env: &Env<Limits>, id: Id<Call>, fetched: Fetched, out: &mut Queue<Request>) {
    let call = model.calls.get_mut(id).expect("a recall with a read in flight is in flight");
    let reading = match &mut call.kind {
        Kind::Reading { reading, .. } => reading,
        Kind::Index { .. }
        | Kind::Search { .. }
        | Kind::Finding { .. }
        | Kind::Queued { .. }
        | Kind::Checking { .. }
        | Kind::Sent { .. }
        | Kind::Answered => unreachable!("only a recall reads"),
    };
    let index = reading.next.checked_sub(1).expect("a read was asked for");
    let wanted = reading.wanted.get(index).expect("the page read is one wanted");
    let fact = match fetched {
        Fetched::Page { revision, page } => {
            if kept::fits(&page, &env.limits) {
                let entry = Entry { scope: wanted.scope, name: wanted.name.clone(), revision, page };
                let pushed = reading.entries.push(entry);
                assert!(pushed.is_ok(), "room for every page wanted");
            } else {
                reading.failed = reading.failed.saturating_add(1);
            }
            Fact::Read
        }
        Fetched::Gone => Fact::Read,
        Fetched::Failed => {
            reading.failed = reading.failed.saturating_add(1);
            Fact::Unread
        }
    };
    model.facts.push(fact);
    read(model, id, out);
}

/// The note `call` has its turn in the kept scope `kept_id`: a new or a
/// removed entry is written at once; a revised one has its page read afresh
/// first.
pub(crate) fn send(model: &mut Model, kept_id: Id<Kept>, call: Id<Call>, out: &mut Queue<Request>) {
    let entry = model.calls.get_mut(call).expect("a note waiting is in flight");
    let (reply_to, scope, name, change) = match mem::replace(&mut entry.kind, Kind::Answered) {
        Kind::Queued { reply_to, scope, name, change } => (reply_to, scope, name, change),
        Kind::Sent { .. }
        | Kind::Checking { .. }
        | Kind::Index { .. }
        | Kind::Search { .. }
        | Kind::Finding { .. }
        | Kind::Reading { .. }
        | Kind::Answered => unreachable!("only a note waits to write"),
    };
    let owner = model::start(model, Op::Scope(kept_id)).token();
    let (busy, kind, request) = match change {
        Change::New(page) => {
            let line = Some(learned(&page));
            let kind = Kind::Sent { reply_to, scope, name: name.clone(), line };
            (Busy::Writing { call }, kind, Request::Create { owner, scope, name, page })
        }
        Change::Revise { page, revision } => {
            let kind = Kind::Checking { reply_to, scope, name: name.clone(), page, revision };
            (Busy::Checking { call }, kind, Request::Fetch { owner, scope, name })
        }
        Change::Remove => {
            let kind = Kind::Sent { reply_to, scope, name: name.clone(), line: None };
            (Busy::Writing { call }, kind, Request::Delete { owner, scope, name })
        }
    };
    model.calls.get_mut(call).expect("in flight").kind = kind;
    model.kept.get_mut(kept_id).expect("a scope kept is in the slab").busy = busy;
    out.push(request);
}

/// The page the note `call` revises was read afresh: it is written over only
/// if it is still at the revision the run recalled; otherwise the note is
/// answered with what was read.
pub(crate) fn checked(
    model: &mut Model,
    env: &Env<Limits>,
    kept_id: Id<Kept>,
    call: Id<Call>,
    fetched: Fetched,
    out: &mut Queue<Request>,
) {
    let entry = model.calls.get_mut(call).expect("a note checking is in flight");
    let (reply_to, scope, name, page, recalled) = match mem::replace(&mut entry.kind, Kind::Answered) {
        Kind::Checking { reply_to, scope, name, page, revision } => (reply_to, scope, name, page, revision),
        Kind::Queued { .. }
        | Kind::Sent { .. }
        | Kind::Index { .. }
        | Kind::Search { .. }
        | Kind::Finding { .. }
        | Kind::Reading { .. }
        | Kind::Answered => unreachable!("only a revision checks"),
    };
    let kept = model.kept.get_mut(kept_id).expect("a scope with an operation in flight is kept");
    let (noted, fact) = match fetched {
        Fetched::Page { revision, page: read } => {
            let fact =
                if kept::learn(kept, &env.limits, name.clone(), revision, read) { Fact::Read } else { Fact::LeftOut };
            if revision == recalled {
                // As the run recalled it: written over.
                model.facts.push(fact);
                let owner = model::start(model, Op::Scope(kept_id)).token();
                let line = Some(learned(&page));
                model.calls.get_mut(call).expect("in flight").kind =
                    Kind::Sent { reply_to, scope, name: name.clone(), line };
                model.kept.get_mut(kept_id).expect("a scope kept is in the slab").busy = Busy::Writing { call };
                out.push(Request::Edit { owner, scope, name, page });
                return;
            }
            (Noted::Moved, fact)
        }
        Fetched::Gone => {
            kept.entries.remove(&name);
            (Noted::Missing, Fact::Read)
        }
        Fetched::Failed => (Noted::Unavailable, Fact::Unread),
    };
    model.facts.push(fact);
    kept::unpin(model, scope);
    model.calls.retire(call);
    model.facts.push(Fact::Answered);
    out.push(Request::Noted { reply_to, noted });
    kept::follow(model, kept_id, out);
}

/// The write of the note `call` ended: the kept scope `kept` learns what it
/// did, and the note is answered.
pub(crate) fn written(model: &mut Model, kept_id: Id<Kept>, call: Id<Call>, wrote: Wrote, out: &mut Queue<Request>) {
    let entry = model.calls.get_mut(call).expect("a note writing is in flight");
    let (reply_to, scope, name, line) = match mem::replace(&mut entry.kind, Kind::Answered) {
        Kind::Sent { reply_to, scope, name, line } => (reply_to, scope, name, line),
        Kind::Queued { .. }
        | Kind::Checking { .. }
        | Kind::Index { .. }
        | Kind::Search { .. }
        | Kind::Finding { .. }
        | Kind::Reading { .. }
        | Kind::Answered => unreachable!("only a note writes"),
    };
    let kept = model.kept.get_mut(kept_id).expect("a scope with an operation in flight is kept");
    let noted = match wrote {
        Wrote::Done { revision } => {
            match line {
                Some(Learned { description, author, references }) => {
                    let known = Known { revision, description, author, references, listing: kept.listings };
                    if kept.entries.insert(name, known).is_err() {
                        // With no room left for a new entry, it is left out.
                        model.facts.push(Fact::LeftOut);
                    }
                }
                None => {
                    kept.entries.remove(&name);
                }
            }
            Noted::Done
        }
        Wrote::Missing => {
            kept.entries.remove(&name);
            Noted::Missing
        }
        Wrote::Exists => {
            // Someone else's page: the index reads it.
            if kept.lacking.insert(name).is_err() {
                kept.relist = true;
            }
            Noted::Exists
        }
        Wrote::Failed => Noted::Unavailable,
    };
    model.facts.push(Fact::Wrote { wrote });
    kept::unpin(model, scope);
    model.calls.retire(call);
    model.facts.push(Fact::Answered);
    out.push(Request::Noted { reply_to, noted });
}

/// Answers `call` with `request`, unpins the scopes it pinned, and retires
/// it.
fn answer(model: &mut Model, call: Id<Call>, scopes: Option<Scopes>, request: Request, out: &mut Queue<Request>) {
    if let Some(scopes) = scopes {
        let order = order(scopes);
        for scope in order.as_slice() {
            kept::unpin(model, *scope);
        }
    }
    model.calls.retire(call);
    model.facts.push(Fact::Answered);
    out.push(request);
}

/// Refuses the call of `reply_to` at the entrance.
fn refuse(model: &mut Model, reply_to: ReplyTo, refusal: Refusal, out: &mut Queue<Request>) {
    model.facts.push(Fact::Refused { refusal });
    out.push(Request::Refused { reply_to, refusal });
}

/// The scopes of a run's notes, narrowest first: its goal's, if it has one,
/// its repository's, the deployment's; and how many there are.
fn order(scopes: Scopes) -> Order {
    let repository = Scope::Repository(scopes.repository);
    match scopes.goal {
        Some(number) => {
            let goal = Scope::Goal { repository: scopes.repository, number };
            Order { scopes: [goal, repository, Scope::Deployment], count: 3 }
        }
        None => Order { scopes: [repository, Scope::Deployment, Scope::Deployment], count: 2 },
    }
}

/// A run's scopes, in order: the first `count` of `scopes`.
struct Order {
    scopes: [Scope; 3],
    count: usize,
}

impl Order {
    fn as_slice(&self) -> &[Scope] {
        self.scopes.get(..self.count).expect("two or three scopes")
    }
}

/// The lines of `scopes`, narrowest scope first and by name within a scope,
/// as many as fit `budget` bytes of names and descriptions; and how many more
/// there are, from the first that did not fit.
fn indexed(model: &Model, env: &Env<Limits>, scopes: Scopes, budget: u32) -> (Box<[Line]>, u32) {
    let order = order(scopes);
    let mut lines = List::with_capacity(env.limits.lines);
    let (mut spent, mut more) = (0_u64, 0_u32);
    for scope in order.as_slice() {
        let Some(id) = model.scopes.get(scope) else {
            continue;
        };
        let kept = model.kept.get(*id).expect("a scope kept is in the slab");
        for (name, known) in &kept.entries {
            let cost = u64::try_from(name.len().saturating_add(known.description.len())).unwrap_or(u64::MAX);
            let total = spent.saturating_add(cost);
            if more > 0 || total > u64::from(budget) || lines.room() == 0 {
                more = more.saturating_add(1);
                continue;
            }
            spent = total;
            let pushed = lines.push(line(*scope, name, known));
            assert!(pushed.is_ok(), "room was checked");
        }
    }
    (lines.into_boxed(), more)
}

/// The lines of `scopes` whose descriptions hold `query`, narrowest scope
/// first and by name within a scope, the first `most`; and how many more
/// there are.
fn found(model: &Model, scopes: Scopes, query: &[u8], most: u32) -> (Box<[Line]>, u32) {
    let order = order(scopes);
    let mut lines = List::with_capacity(most);
    let mut more: u32 = 0;
    for scope in order.as_slice() {
        let Some(id) = model.scopes.get(scope) else {
            continue;
        };
        let kept = model.kept.get(*id).expect("a scope kept is in the slab");
        for (name, known) in &kept.entries {
            if !holds(&known.description, query) {
                continue;
            }
            if lines.room() == 0 {
                more = more.saturating_add(1);
                continue;
            }
            let pushed = lines.push(line(*scope, name, known));
            assert!(pushed.is_ok(), "room was checked");
        }
    }
    (lines.into_boxed(), more)
}

/// The line of the entry `name` of `scope`, as `known`.
fn line(scope: Scope, name: &[u8], known: &Known) -> Line {
    Line {
        scope,
        name: bytes::copy_of(name),
        description: known.description.clone(),
        author: known.author,
        references: known.references.clone(),
    }
}

/// The pages of `scopes` whose descriptions hold `query`, the first `most`,
/// for a recall to read.
fn wanted(model: &Model, scopes: Scopes, query: &[u8], most: u32) -> List<Wanted> {
    let order = order(scopes);
    let mut wanted = List::with_capacity(most);
    for scope in order.as_slice() {
        let Some(id) = model.scopes.get(scope) else {
            continue;
        };
        let kept = model.kept.get(*id).expect("a scope kept is in the slab");
        for (name, known) in &kept.entries {
            if wanted.room() > 0 && holds(&known.description, query) {
                let pushed = wanted.push(Wanted { scope: *scope, name: name.clone() });
                assert!(pushed.is_ok(), "room was checked");
            }
        }
    }
    wanted
}

/// How many of `scopes` have never been listed.
fn unread(model: &Model, scopes: Scopes) -> u32 {
    let order = order(scopes);
    let mut unread: u32 = 0;
    for scope in order.as_slice() {
        let listed = match model.scopes.get(scope) {
            Some(id) => model.kept.get(*id).expect("a scope kept is in the slab").listed,
            None => false,
        };
        if !listed {
            unread = unread.saturating_add(1);
        }
    }
    unread
}

/// Whether `description` holds `query`; an empty query is held by every one.
fn holds(description: &[u8], query: &[u8]) -> bool {
    query.is_empty() || bytes::find(description, query).is_some()
}

/// What the index learns of `page` once it is written.
fn learned(page: &Page) -> Learned {
    Learned { description: page.description.clone(), author: page.author, references: page.references.clone() }
}
