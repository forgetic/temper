//! The tools child domain's state and its entry point (section 3).

use skein_lib::{Env, Queue, Slab};

use crate::boundary::{Event, Request};
use crate::facts::{Fact, Facts};
use crate::job::{self, Job};
use crate::kit::{self, Kit};
use crate::limits::Limits;

/// The most requests one call of [`step`] emits under `limits`: closing a kit
/// cancels each call it is running, up to `limits.calls`, and any other event
/// emits at most two. The parent reserves this much room in `out` before
/// calling it.
#[must_use]
pub const fn max_out(limits: &Limits) -> u32 {
    if limits.calls > 2 { limits.calls } else { 2 }
}

/// The tools child domain's state.
#[derive(Debug)]
pub struct Domain {
    pub(crate) kits: Slab<Kit>,
    pub(crate) jobs: Slab<Job>,
    pub(crate) facts: Facts,
}

impl Domain {
    /// A domain with room for `limits`.
    #[must_use]
    pub fn new(limits: &Limits) -> Domain {
        let jobs = job::slots(limits).expect("worst_case accepted the limits");
        Domain {
            kits: Slab::with_capacity(limits.kits),
            jobs: Slab::with_capacity(jobs),
            facts: Facts::with_capacity(limits.facts),
        }
    }

    /// Kits present, closed ones included until they are reclaimed.
    #[must_use]
    pub fn kits(&self) -> u32 {
        self.kits.len()
    }

    /// Calls running, answered ones included until they are reclaimed.
    #[must_use]
    pub fn jobs(&self) -> u32 {
        self.jobs.len()
    }

    /// The oldest fact not yet drained. The parent drains them at its own
    /// pace; what does not fit meanwhile is dropped and counted.
    pub fn pop_fact(&mut self) -> Option<Fact> {
        self.facts.pop()
    }

    /// How many facts were dropped for want of room, since the domain was
    /// made.
    #[must_use]
    pub fn facts_lost(&self) -> u64 {
        self.facts.lost()
    }

    /// The reclaim point: frees what closed in this iteration.
    pub fn reclaim(&mut self) {
        self.jobs.reclaim();
        self.kits.reclaim();
    }
}

/// Handles one event, emitting at most [`max_out`] requests.
pub fn step(domain: &mut Domain, env: &Env<Limits>, event: Event, out: &mut Queue<Request>) {
    match event {
        Event::Open { session, authority } => kit::open(domain, env, session, authority, out),
        Event::Call { kit, reply_to, call, deadline } => kit::call(domain, env, kit, reply_to, call, deadline, out),
        Event::Close { kit } => kit::close(domain, kit, out),
        Event::Done { owner, done } => job::done(domain, env, owner, done, out),
    }
}
