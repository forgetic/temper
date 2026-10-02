//! A kept scope (engine-model.md, section 10): the index of one scope's
//! notes, one line per entry, learned from the scope's wiki through the
//! parent. It runs one wiki operation at a time, so that what each answers
//! is taken in the order it was asked: a page read before a listing is never
//! taken in after it.
//!
//! | Busy | Event | Becomes | Does |
//! |---|---|---|---|
//! | idle | a note waits to be written | writing | asks for the write |
//! | idle | a listing is wanted (a new scope, `Refresh`, a `Changed` with no room) | listing | asks for the list |
//! | idle | a page is lacking | fetching | asks for the page |
//! | idle | nothing to do | idle | ends its first pass: the calls waiting for it go on |
//! | listing | `Listed`, pages | idle, follows | marks the entries listed, wants the pages it does not know at their revision, forgets the entries no longer listed |
//! | listing | `Listed`, none | idle, follows | keeps what it knew |
//! | fetching | `Fetched`, a page | idle, follows | learns its line; one past the limits is left out, and forgotten |
//! | fetching | `Fetched`, gone | idle, follows | forgets the entry |
//! | fetching | `Fetched`, failed | idle, follows | keeps what it knew |
//! | writing | `Wrote` | idle, follows | learns what the write did, and answers the note |
//! | listing, fetching, writing | another kind of terminal | | the contract rules it out |
//!
//! A hint that comes while it is busy waits until it is idle: `Refresh`
//! wants a listing, `Changed` the page. A scope is kept until a call needs
//! room for another, and is evicted only once it is idle with no call
//! pinning it, the least recently used first.

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
    /// Whether its first pass, a listing and the reads it wanted, has ended:
    /// until then the calls that need it wait.
    pub(crate) passed: bool,
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

/// The wiki operation a kept scope has in flight.
#[derive(Debug)]
pub(crate) enum Busy {
    Idle,
    Listing,
    Fetching {
        name: Box<[u8]>,
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
            passed: false,
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
            Busy::Listing | Busy::Fetching { .. } | Busy::Writing { .. } => false,
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
            if !kept.listed && kept.passed {
                kept.passed = false;
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
            (least, oldest) = (Some(*id), kept.used);
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

/// Starts what the kept scope `id` is to do next, if it is idle: a write, a
/// listing, a page's read; or, with nothing left, ends its first pass.
pub(crate) fn follow(model: &mut Model, id: Id<Kept>, out: &mut Queue<Request>) {
    let kept = model.kept.get_mut(id).expect("a scope kept is in the slab");
    match kept.busy {
        Busy::Idle => {}
        Busy::Listing | Busy::Fetching { .. } | Busy::Writing { .. } => return,
    }
    let scope = kept.scope;
    if let Some(note) = kept.writes.pop() {
        kept.busy = Busy::Writing { call: note };
        let op = model::start(model, Op::Scope(id));
        call::send(model, note, op, out);
        return;
    }
    if kept.relist {
        kept.relist = false;
        kept.busy = Busy::Listing;
        let op = model::start(model, Op::Scope(id));
        out.push(Request::List { owner: op.token(), scope });
        return;
    }
    if let Some(name) = kept.lacking.pop_first() {
        kept.busy = Busy::Fetching { name: name.clone() };
        let op = model::start(model, Op::Scope(id));
        out.push(Request::Fetch { owner: op.token(), scope, name });
        return;
    }
    if kept.passed {
        return;
    }
    kept.passed = true;
    for _ in 0..kept.waiting.len() {
        let kept = model.kept.get_mut(id).expect("a scope kept is in the slab");
        let Some(waiting) = kept.waiting.pop() else {
            break;
        };
        call::wake(model, waiting);
    }
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
        Busy::Idle | Busy::Fetching { .. } | Busy::Writing { .. } => unreachable!("a list ends a listing"),
    }
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
        if wanted && kept.lacking.insert(page.name).is_err() {
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
        Busy::Idle | Busy::Listing | Busy::Writing { .. } => unreachable!("a read ends a fetch"),
    };
    kept.busy = Busy::Idle;
    let fact = match fetched {
        Fetched::Page { revision, page } => {
            learn(kept, &env.limits, name, revision, page);
            Fact::Read
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
        Busy::Idle | Busy::Listing | Busy::Fetching { .. } => unreachable!("a write ends a write"),
    };
    kept.busy = Busy::Idle;
    call::written(model, id, note, wrote, out);
    follow(model, id, out);
}

/// Learns the line of the page `name` at `revision`, as the last listing
/// would have named it; a page past the limits, or one with no room left,
/// is left out, and forgotten if it was known.
pub(crate) fn learn(kept: &mut Kept, limits: &Limits, name: Box<[u8]>, revision: u64, page: Page) {
    if !fits(&page, limits) {
        kept.entries.remove(&name);
        return;
    }
    let Page { description, author, references, body: _ } = page;
    let known = Known { revision, description, author, references, listing: kept.listings };
    // With no room left for a new entry, it is left out.
    let _left_out = kept.entries.insert(name, known).is_err();
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
