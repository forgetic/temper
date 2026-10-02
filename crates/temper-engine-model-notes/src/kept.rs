//! A kept scope (engine-model.md, section 10): the index of one scope's
//! notes, one line per entry, learned from the scope's wiki through the
//! parent. It runs one wiki operation at a time, so that what each answers
//! is taken in the order it was asked: a page read before a listing is never
//! taken in after it.
//!
//! Its pass (its first, or another for a scope that could never be listed)
//! is a listing, then the reads of the pages that listing wanted, and the
//! calls that need the scope wait for it. What changes meanwhile (a hint, a
//! note written, a listing asked for) waits for the pass to end, so that no
//! stream of changes holds the calls waiting back.
//!
//! When idle, it starts the first of these:
//!
//! | Pass | Wanted | Becomes | Does |
//! |---|---|---|---|
//! | listing | its listing | listing | asks for the list |
//! | reading | a page its listing wanted | fetching | asks for the page |
//! | reading | nothing more | idle, follows | ends the pass: the calls waiting for it go on |
//! | ended | a note waits to be written, new or removed | writing | asks for the write |
//! | ended | a note waits to be revised | checking | asks for the page afresh |
//! | ended | a page is lacking (listed at a revision it does not know, or changed) | fetching | asks for the page |
//! | ended | a listing (`Refresh`, a `Changed` with no room) | listing | asks for the list |
//!
//! And on an operation's end:
//!
//! | Busy | Event | Becomes | Does |
//! |---|---|---|---|
//! | listing | `Listed`, pages | idle, follows | marks the entries listed, wants the pages it does not know at their revision (in a pass, those the pass reads), forgets the entries no longer listed |
//! | listing | `Listed`, none | idle, follows | keeps what it knew |
//! | fetching | `Fetched`, a page | idle, follows | learns its line; one past the limits, or with no room, is left out, and forgotten |
//! | fetching | `Fetched`, gone | idle, follows | forgets the entry |
//! | fetching | `Fetched`, failed | idle, follows | keeps what it knew |
//! | checking | `Fetched`, at the revision the note names | writing | learns its line, asks for the edit |
//! | checking | `Fetched`, another revision, gone or failed | idle, follows | learns what it read, answers the note: moved, missing, unavailable |
//! | writing | `Wrote` | idle, follows | learns what the write did, and answers the note |
//! | listing, fetching, checking, writing | another kind of terminal | | the contract rules it out |
//!
//! A scope is kept until a call needs room for another, and is evicted only
//! once it is idle with no call pinning it, the least recently used first.

use alloc::boxed::Box;

use temper_lib::{Env, Id, List, Map, Queue, Set};

use crate::boundary::{Author, Fetched, Listed, Page, Reference, Refusal, Request, Scope, Wrote};
use crate::call::{self, Call};
use crate::facts::Fact;
use crate::limits::Limits;
use crate::model::{self, Model, Op};

/// A scope whose index is kept.
#[derive(Debug)]
pub(crate) struct Kept {
    pub(crate) scope: Scope,
    /// Where its pass stands. Until it has ended, the calls that need it
    /// wait.
    pub(crate) pass: Pass,
    /// Whether its pages have been listed once: until then its index is not
    /// known.
    pub(crate) listed: bool,
    /// The listings taken in: an entry is marked with the last that named it.
    pub(crate) listings: u64,
    pub(crate) entries: Map<Box<[u8]>, Known>,
    /// The pages to read: listed at a revision it does not know, or changed.
    pub(crate) lacking: Set<Box<[u8]>>,
    /// Whether a listing is wanted.
    pub(crate) relist: bool,
    pub(crate) busy: Busy,
    /// Notes waiting to be written, in order, and calls waiting for its
    /// first pass.
    pub(crate) writes: Queue<Id<Call>>,
    pub(crate) waiting: Queue<Id<Call>>,
    /// Calls that need it kept until they are answered.
    pub(crate) pins: u32,
    /// When a call last needed it, by the model's count of uses.
    pub(crate) used: u64,
}

/// What a kept scope knows of an entry: its line, the revision it read it
/// at, and the last listing that named it.
#[derive(Debug)]
pub(crate) struct Known {
    pub(crate) revision: u64,
    pub(crate) description: Box<[u8]>,
    pub(crate) author: Author,
    pub(crate) references: Box<[Reference]>,
    pub(crate) listing: u64,
}

/// A kept scope's pass: a listing, then the reads it wanted.
#[derive(Debug)]
pub(crate) enum Pass {
    /// Until a listing is taken in, or fails.
    Listing,
    /// Reading the pages the listing wanted, those left.
    Reading(Set<Box<[u8]>>),
    Ended,
}

/// The wiki operation a kept scope has in flight.
#[derive(Debug)]
pub(crate) enum Busy {
    Idle,
    Listing,
    Fetching {
        name: Box<[u8]>,
    },
    /// Reading afresh the page the note `call` revises.
    Checking {
        call: Id<Call>,
    },
    /// Writing for the note `call`.
    Writing {
        call: Id<Call>,
    },
}

impl Kept {
    fn new(scope: Scope, limits: &Limits, used: u64) -> Kept {
        Kept {
            scope,
            pass: Pass::Listing,
            listed: false,
            listings: 0,
            entries: Map::with_capacity(limits.entries),
            lacking: Set::with_capacity(limits.entries),
            relist: true,
            busy: Busy::Idle,
            writes: Queue::with_capacity(limits.calls),
            waiting: Queue::with_capacity(limits.calls),
            pins: 0,
            used,
        }
    }

    /// Whether it may be evicted: nothing in flight, nothing waiting, and no
    /// call pinning it.
    fn is_idle(&self) -> bool {
        let idle = match self.busy {
            Busy::Idle => true,
            Busy::Listing | Busy::Fetching { .. } | Busy::Checking { .. } | Busy::Writing { .. } => false,
        };
        idle && self.writes.is_empty() && self.pins == 0
    }
}

/// Keeps each of `scopes` and pins it for a call, or refuses as busy without
/// changing anything if there is no room for them: a scope not kept takes a
/// free place, or that of the least recently used idle one, evicted. Asks for
/// the first listing of each scope it starts keeping, and lists again one
/// that could never be listed, the call waiting for it.
pub(crate) fn keep(
    model: &mut Model,
    env: &Env<Limits>,
    scopes: &[Scope],
    out: &mut Queue<Request>,
) -> Result<(), Refusal> {
    let mut missing: u32 = 0;
    for scope in scopes {
        if !model.scopes.contains_key(scope) {
            missing = missing.saturating_add(1);
        }
    }
    let mut room = model.kept.capacity().saturating_sub(model.kept.len());
    for (scope, id) in &model.scopes {
        let kept = model.kept.get(*id).expect("a scope kept is in the slab");
        if kept.is_idle() && !scopes.contains(scope) {
            room = room.saturating_add(1);
        }
    }
    if missing > room {
        return Err(Refusal::Busy);
    }
    // Pins those kept first, so that none of them is evicted for another.
    for scope in scopes {
        model.uses = model.uses.saturating_add(1);
        if let Some(id) = model.scopes.get(scope).copied() {
            let kept = model.kept.get_mut(id).expect("a scope kept is in the slab");
            kept.pins = kept.pins.saturating_add(1);
            kept.used = model.uses;
            let ended = match kept.pass {
                Pass::Ended => true,
                Pass::Listing | Pass::Reading(_) => false,
            };
            if !kept.listed && ended {
                kept.pass = Pass::Listing;
                kept.relist = true;
                follow(model, id, out);
            }
        }
    }
    for scope in scopes {
        if model.scopes.contains_key(scope) {
            continue;
        }
        let mut fresh = Kept::new(*scope, &env.limits, model.uses);
        fresh.pins = 1;
        let id = match model.kept.insert(fresh) {
            Ok(id) => {
                model.facts.push(Fact::Kept { evicted: false });
                id
            }
            Err(fresh) => {
                let id = least_used(model).expect("room was counted");
                let kept = model.kept.get_mut(id).expect("a scope kept is in the slab");
                let evicted = kept.scope;
                *kept = fresh;
                model.scopes.remove(&evicted);
                model.facts.push(Fact::Kept { evicted: true });
                id
            }
        };
        let inserted = model.scopes.insert(*scope, id);
        assert!(inserted.is_ok(), "a scope kept has a place in the map as in the slab");
        follow(model, id, out);
    }
    Ok(())
}

/// The least recently used scope that may be evicted.
fn least_used(model: &Model) -> Option<Id<Kept>> {
    let (mut least, mut oldest) = (None, u64::MAX);
    for (_, id) in &model.scopes {
        let kept = model.kept.get(*id).expect("a scope kept is in the slab");
        if kept.is_idle() && (least.is_none() || kept.used < oldest) {
            least = Some(*id);
            oldest = kept.used;
        }
    }
    least
}

/// A call no longer needs `scope` kept.
pub(crate) fn unpin(model: &mut Model, scope: Scope) {
    let id = *model.scopes.get(&scope).expect("a pinned scope is kept");
    let kept = model.kept.get_mut(id).expect("a scope kept is in the slab");
    kept.pins = kept.pins.checked_sub(1).expect("a scope is unpinned once for each pin");
}

/// Starts what the kept scope `id` is to do next, if it is idle: its pass's
/// listing and reads first, ending the pass once they are done; then a
/// write, a page's read, a listing.
pub(crate) fn follow(model: &mut Model, id: Id<Kept>, out: &mut Queue<Request>) {
    let kept = model.kept.get_mut(id).expect("a scope kept is in the slab");
    match kept.busy {
        Busy::Idle => {}
        Busy::Listing | Busy::Fetching { .. } | Busy::Checking { .. } | Busy::Writing { .. } => return,
    }
    let scope = kept.scope;
    let read = match &mut kept.pass {
        Pass::Listing => {
            if kept.relist {
                kept.relist = false;
                kept.busy = Busy::Listing;
                let op = model::start(model, Op::Scope(id));
                out.push(Request::List { owner: op.token(), scope });
            }
            return;
        }
        Pass::Reading(names) => names.pop_first(),
        Pass::Ended => None,
    };
    if let Some(name) = read {
        kept.lacking.remove(&name);
        fetch(model, id, name, out);
        return;
    }
    let ended = match kept.pass {
        Pass::Reading(_) => {
            kept.pass = Pass::Ended;
            true
        }
        Pass::Listing | Pass::Ended => false,
    };
    if ended {
        for _ in 0..kept.waiting.len() {
            let kept = model.kept.get_mut(id).expect("a scope kept is in the slab");
            let waiting = kept.waiting.pop().expect("as many as were counted");
            call::wake(model, waiting);
        }
    }
    let kept = model.kept.get_mut(id).expect("a scope kept is in the slab");
    if let Some(note) = kept.writes.pop() {
        call::send(model, id, note, out);
        return;
    }
    if let Some(name) = kept.lacking.pop_first() {
        fetch(model, id, name, out);
        return;
    }
    if kept.relist {
        kept.relist = false;
        kept.busy = Busy::Listing;
        let op = model::start(model, Op::Scope(id));
        out.push(Request::List { owner: op.token(), scope });
    }
}

/// The kept scope `id` reads the page `name`.
fn fetch(model: &mut Model, id: Id<Kept>, name: Box<[u8]>, out: &mut Queue<Request>) {
    let kept = model.kept.get_mut(id).expect("a scope kept is in the slab");
    let scope = kept.scope;
    kept.busy = Busy::Fetching { name: name.clone() };
    let op = model::start(model, Op::Scope(id));
    out.push(Request::Fetch { owner: op.token(), scope, name });
}

/// The pages of `scope` may have changed: they are listed again, if it is
/// kept.
pub(crate) fn refresh(model: &mut Model, scope: Scope, out: &mut Queue<Request>) {
    let Some(id) = model.scopes.get(&scope).copied() else {
        return;
    };
    model.kept.get_mut(id).expect("a scope kept is in the slab").relist = true;
    follow(model, id, out);
}

/// The page `name` of `scope` changed: it is read again, if the scope is
/// kept; or, with no room to remember that, the scope is listed again.
pub(crate) fn changed(model: &mut Model, env: &Env<Limits>, scope: Scope, name: Box<[u8]>, out: &mut Queue<Request>) {
    let Some(id) = model.scopes.get(&scope).copied() else {
        return;
    };
    if !named(&name, &env.limits) {
        return;
    }
    let kept = model.kept.get_mut(id).expect("a scope kept is in the slab");
    if kept.lacking.insert(name).is_err() {
        kept.relist = true;
    }
    follow(model, id, out);
}

/// The listing of the kept scope `id` ended, with its pages, or `None` if
/// they could not be listed: the index follows the listing.
pub(crate) fn listed(
    model: &mut Model,
    env: &Env<Limits>,
    id: Id<Kept>,
    pages: Option<Box<[Listed]>>,
    out: &mut Queue<Request>,
) {
    let kept = model.kept.get_mut(id).expect("a scope with an operation in flight is kept");
    match kept.busy {
        Busy::Listing => kept.busy = Busy::Idle,
        Busy::Idle | Busy::Fetching { .. } | Busy::Checking { .. } | Busy::Writing { .. } => {
            unreachable!("a list ends a listing")
        }
    }
    // A pass's listing is taken in: the pass reads what it wants.
    let in_pass = match kept.pass {
        Pass::Listing => {
            kept.pass = Pass::Reading(Set::with_capacity(env.limits.entries));
            true
        }
        Pass::Reading(_) | Pass::Ended => false,
    };
    let Some(pages) = pages else {
        model.facts.push(Fact::Unlisted);
        follow(model, id, out);
        return;
    };
    let listing = kept.listings.checked_add(1).expect("listings are counted in a u64");
    kept.listings = listing;
    kept.listed = true;
    let limits = &env.limits;
    let (mut taken, mut left_out) = (0_u32, 0_u32);
    for page in pages {
        if taken >= limits.entries || !named(&page.name, limits) {
            left_out = left_out.saturating_add(1);
            continue;
        }
        taken = taken.saturating_add(1);
        let wanted = match kept.entries.get_mut(&page.name) {
            Some(known) => {
                known.listing = listing;
                known.revision != page.revision
            }
            None => true,
        };
        if !wanted {
            continue;
        }
        let room = if in_pass {
            match &mut kept.pass {
                Pass::Reading(names) => names.insert(page.name).is_ok(),
                Pass::Listing | Pass::Ended => unreachable!("the pass reads what its listing wants"),
            }
        } else {
            kept.lacking.insert(page.name).is_ok()
        };
        if !room {
            left_out = left_out.saturating_add(1);
        }
    }
    // Forgets the entries this listing does not name: their pages are gone.
    let mut gone = List::with_capacity(kept.entries.len());
    for (name, known) in &kept.entries {
        if known.listing != listing {
            let pushed = gone.push(name.clone());
            assert!(pushed.is_ok(), "room for every entry");
        }
    }
    for name in &gone {
        kept.entries.remove(name);
    }
    model.facts.push(Fact::Listed { pages: taken, left_out });
    follow(model, id, out);
}

/// The read of a page of the kept scope `id` ended: the index learns its
/// line, or forgets it.
pub(crate) fn fetched(model: &mut Model, env: &Env<Limits>, id: Id<Kept>, fetched: Fetched, out: &mut Queue<Request>) {
    let kept = model.kept.get_mut(id).expect("a scope with an operation in flight is kept");
    let name = match &kept.busy {
        Busy::Fetching { name } => name.clone(),
        Busy::Checking { call } => {
            let note = *call;
            kept.busy = Busy::Idle;
            call::checked(model, env, id, note, fetched, out);
            return;
        }
        Busy::Idle | Busy::Listing | Busy::Writing { .. } => unreachable!("a read ends a fetch"),
    };
    kept.busy = Busy::Idle;
    let fact = match fetched {
        Fetched::Page { revision, page } => {
            if learn(kept, &env.limits, name, revision, page) {
                Fact::Read
            } else {
                Fact::LeftOut
            }
        }
        Fetched::Gone => {
            kept.entries.remove(&name);
            Fact::Read
        }
        Fetched::Failed => Fact::Unread,
    };
    model.facts.push(fact);
    follow(model, id, out);
}

/// The write of the kept scope `id` ended: the note it was for learns how.
pub(crate) fn wrote(model: &mut Model, id: Id<Kept>, wrote: Wrote, out: &mut Queue<Request>) {
    let kept = model.kept.get_mut(id).expect("a scope with an operation in flight is kept");
    let note = match kept.busy {
        Busy::Writing { call } => call,
        Busy::Idle | Busy::Listing | Busy::Fetching { .. } | Busy::Checking { .. } => {
            unreachable!("a write ends a write")
        }
    };
    kept.busy = Busy::Idle;
    call::written(model, id, note, wrote, out);
    follow(model, id, out);
}

/// Learns the line of the page `name` at `revision`, as the last listing
/// would have named it, and says whether it did: a page past the limits, or
/// one with no room left, is left out, and forgotten if it was known.
pub(crate) fn learn(kept: &mut Kept, limits: &Limits, name: Box<[u8]>, revision: u64, page: Page) -> bool {
    if !fits(&page, limits) {
        kept.entries.remove(&name);
        return false;
    }
    let Page { description, author, references, body: _ } = page;
    let known = Known { revision, description, author, references, listing: kept.listings };
    kept.entries.insert(name, known).is_ok()
}

/// Whether `name` may name an entry under `limits`.
pub(crate) fn named(name: &[u8], limits: &Limits) -> bool {
    !name.is_empty() && fits_bytes(name, limits.name_bytes)
}

/// Whether `page` fits `limits`.
pub(crate) fn fits(page: &Page, limits: &Limits) -> bool {
    fits_bytes(&page.description, limits.description_bytes)
        && fits_bytes(&page.body, limits.body_bytes)
        && fits_count(page.references.len(), limits.references)
}

/// Whether `bytes` are at most `most`.
pub(crate) fn fits_bytes(bytes: &[u8], most: u32) -> bool {
    fits_count(bytes.len(), most)
}

/// Whether `len` is at most `most`.
pub(crate) fn fits_count(len: usize, most: u32) -> bool {
    match u32::try_from(len) {
        Ok(len) => len <= most,
        Err(_) => false,
    }
}
