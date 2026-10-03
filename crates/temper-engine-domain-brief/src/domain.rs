//! The brief child domain's state and its entry points (programming-model.md,
//! section 3).

use temper_lib::{Deadlines, Env, Id, Queue, Slab, Time};

use crate::boundary::{Event, Request};
use crate::brief::{self, Brief, Reading};
use crate::facts::{Fact, Facts};
use crate::limits::{self, Limits};

/// The brief child domain's state.
#[derive(Debug)]
pub struct Domain {
    pub(crate) briefs: Slab<Brief>,
    /// The reads in flight, each for a section of a brief, or orphaned once
    /// its brief has answered.
    pub(crate) reads: Slab<Reading>,
    /// Each gathering brief's deadline.
    pub(crate) alarms: Deadlines<Id<Brief>>,
    /// The briefs gathering and the reads in flight, which the slabs hold
    /// until the reclaim point after they end: what admission counts.
    pub(crate) gathering: u32,
    pub(crate) reading: u32,
    /// Whether a render was refused as busy since the parent last heard
    /// there is room.
    pub(crate) owed: bool,
    pub(crate) facts: Facts,
}

impl Domain {
    /// A domain with room for `limits`.
    #[must_use]
    pub fn new(limits: &Limits) -> Domain {
        let reads = limits::reads(limits).expect("worst_case accepted the limits");
        Domain {
            briefs: Slab::with_capacity(limits::slots(limits.briefs).expect("worst_case accepted the limits")),
            reads: Slab::with_capacity(limits::slots(reads).expect("worst_case accepted the limits")),
            alarms: Deadlines::with_capacity(limits.briefs),
            gathering: 0,
            reading: 0,
            owed: false,
            facts: Facts::with_capacity(limits.facts),
        }
    }

    /// Briefs present, answered ones included until they are reclaimed.
    #[must_use]
    pub fn briefs(&self) -> u32 {
        self.briefs.len()
    }

    /// Reads in flight, orphaned ones included, and ended ones until they
    /// are reclaimed.
    #[must_use]
    pub fn reads(&self) -> u32 {
        self.reads.len()
    }

    /// When the earliest deadline falls due.
    #[must_use]
    pub fn next_deadline(&self) -> Option<Time> {
        self.alarms.next()
    }

    /// Whether a deadline is due at `now`. While one is, the loop fires the
    /// root domain, which calls [`fire`].
    #[must_use]
    pub fn is_due(&self, now: Time) -> bool {
        match self.alarms.next() {
            Some(at) => at <= now,
            None => false,
        }
    }

    /// The oldest fact not yet drained. The parent drains them at its own
    /// pace; what does not fit meanwhile is dropped and counted.
    pub fn pop_fact(&mut self) -> Option<Fact> {
        self.facts.pop()
    }

    /// How many facts were dropped for want of room since the domain was made.
    #[must_use]
    pub fn facts_lost(&self) -> u64 {
        self.facts.lost()
    }

    /// The reclaim point: frees what ended in this iteration.
    pub fn reclaim(&mut self) {
        self.briefs.reclaim();
        self.reads.reclaim();
    }
}

/// The most requests one step or alarm emits under `limits`: a render's
/// reads, one per section; or an answer and a room notice. The parent
/// reserves this much room in `out`.
#[must_use]
pub const fn max_out(limits: &Limits) -> u32 {
    if limits.sections > 2 { limits.sections } else { 2 }
}

/// Handles one event, emitting at most [`max_out`] requests.
pub fn step(domain: &mut Domain, env: &Env<Limits>, event: Event, out: &mut Queue<Request>) {
    match event {
        Event::Render { reply_to, sections } => brief::render(domain, env, reply_to, sections, out),
        Event::Read { owner, read } => brief::read(domain, env, owner, read, out),
    }
}

/// Fires the earliest deadline due at `env.now`, if there is one, emitting
/// at most [`max_out`] requests. A stage fires its alarms after its input
/// events, so a read that arrived in the same iteration wins over a deadline
/// that passed while the loop waited.
pub fn fire(domain: &mut Domain, env: &Env<Limits>, out: &mut Queue<Request>) {
    let Some(id) = domain.alarms.expire(env.now) else {
        return;
    };
    brief::expire(domain, env, id, out);
}
