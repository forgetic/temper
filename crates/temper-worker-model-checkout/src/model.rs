//! The checkout sub-model's state and its entry point (section 3).

use temper_lib::{Env, Queue, Slab};

use crate::boundary::{Event, Request};
use crate::cache::Cache;
use crate::facts::{Fact, Facts};
use crate::hold::{self, Hold};
use crate::limits::{self, Limits};

/// The most requests a step emits: an admitted prepare's `Held` and its first
/// operation, or a prepare's, a push's or a save's end and the `Released` of a
/// hold released while it was under way. The parent reserves this much room in `out` before
/// calling it.
pub const MAX_OUT: u32 = 2;

/// The checkout sub-model's state.
#[derive(Debug)]
pub struct Model {
    pub(crate) cache: Cache,
    pub(crate) holds: Slab<Hold>,
    pub(crate) facts: Facts,
}

impl Model {
    /// A model with room for `limits`.
    #[must_use]
    pub fn new(limits: &Limits) -> Model {
        let holds = limits::holds(limits).expect("worst_case accepted the limits");
        Model {
            cache: Cache::with_capacity(limits.workspaces),
            holds: Slab::with_capacity(holds),
            facts: Facts::with_capacity(limits.facts),
        }
    }

    /// Workspaces in the cache, held or idle.
    #[must_use]
    pub fn workspaces(&self) -> u32 {
        self.cache.len()
    }

    /// Workspaces no client holds: those the cache may evict.
    #[must_use]
    pub fn idle(&self) -> u32 {
        self.cache.idle()
    }

    /// Holds present, released ones included until they are reclaimed.
    #[must_use]
    pub fn holds(&self) -> u32 {
        self.holds.len()
    }

    /// The workstream of the cache's `nth` workspace that holds its
    /// repositories cloned, in byte order, if it has that many: what the
    /// worker tells the engine it holds checkouts for. A workspace whose disk
    /// is not known (being built, or damaged) is not counted.
    #[must_use]
    pub fn workstream(&self, nth: u32) -> Option<&[u8]> {
        self.cache.key(nth)
    }

    /// The oldest fact not yet drained. The parent drains them at its own
    /// pace; what does not fit meanwhile is dropped and counted.
    pub fn pop_fact(&mut self) -> Option<Fact> {
        self.facts.pop()
    }

    /// How many facts were dropped for want of room since the model was made.
    #[must_use]
    pub fn facts_lost(&self) -> u64 {
        self.facts.lost()
    }

    /// The reclaim point: frees what was released in this iteration.
    pub fn reclaim(&mut self) {
        self.holds.reclaim();
    }
}

/// Handles one event, emitting at most [`MAX_OUT`] requests.
pub fn step(model: &mut Model, env: &Env<Limits>, event: Event, out: &mut Queue<Request>) {
    match event {
        Event::Prepare { client, spec } => hold::prepare(model, env, client, spec, out),
        Event::Push { hold, message } => hold::push(model, env, hold, message, out),
        Event::Save { hold, branch, message } => hold::save(model, env, hold, branch, message, out),
        Event::Abort { hold } => hold::abort(model, hold, out),
        Event::Release { hold } => hold::release(model, hold, out),
        Event::Done { owner, done } => hold::done(model, env, owner, done, out),
    }
}
