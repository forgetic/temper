//! The work hub's state and its entry points (engine-domain.md, section 3).

use temper_lib::{Deadlines, Env, Id, Map, Queue, Rng, Slab, Time};

use crate::boundary::{Event, Item, Request};
use crate::facts::{Fact, Facts};
use crate::limits::Limits;
use crate::tracked::{self, Tracked};

/// The most requests an entry point emits per call under `limits`: the
/// answer to a call and what it sets going (a record to write, a cancel, an
/// adoption), or an acknowledgement and the next request of the item it
/// frees, or a snapshot to keep and a record to write; or, as a run is
/// placed, every inbound event kept for it. The parent reserves this much
/// room in `out` before calling it.
#[must_use]
pub const fn max_out(limits: &Limits) -> u32 {
    if limits.undelivered > 2 { limits.undelivered } else { 2 }
}

/// The work hub's state.
#[derive(Debug)]
pub struct Domain {
    /// The items taken in.
    pub(crate) tracked: Slab<Tracked>,
    /// The items taken in, by their forge names, closed ones aside.
    pub(crate) names: Map<Item, Id<Tracked>>,
    /// One alarm per item at most: its wake, its retry or the end of its
    /// grace, as its state says.
    pub(crate) alarms: Deadlines<Id<Tracked>>,
    /// The jitter of backoffs.
    pub(crate) rng: Rng,
    pub(crate) facts: Facts,
}

impl Domain {
    /// A domain with room for `limits`, drawing jitter from `seed`.
    #[must_use]
    pub fn new(limits: &Limits, seed: u64) -> Domain {
        Domain {
            tracked: Slab::with_capacity(limits.items),
            names: Map::with_capacity(limits.items),
            alarms: Deadlines::with_capacity(limits.items),
            rng: Rng::new(seed),
            facts: Facts::with_capacity(limits.facts),
        }
    }

    /// Items taken in, those done included until they are reclaimed.
    #[must_use]
    pub fn items(&self) -> u32 {
        self.tracked.len()
    }

    /// When the earliest alarm falls due.
    #[must_use]
    pub fn next_deadline(&self) -> Option<Time> {
        self.alarms.next()
    }

    /// Whether an alarm is due at `now`. While one is, the loop fires the
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

    /// How many facts were dropped for want of room, since the domain was
    /// made.
    #[must_use]
    pub fn facts_lost(&self) -> u64 {
        self.facts.lost()
    }

    /// The reclaim point: frees what closed in this iteration.
    pub fn reclaim(&mut self) {
        self.tracked.reclaim();
    }
}

/// Handles one event, emitting at most [`max_out`] requests.
pub fn step(domain: &mut Domain, env: &Env<Limits>, event: Event, out: &mut Queue<Request>) {
    match event {
        Event::Take { reply_to, item, read } => tracked::take(domain, env, reply_to, item, read, out),
        Event::Stop { reply_to, item } => tracked::stop(domain, reply_to, item, out),
        Event::Release { reply_to, item } => tracked::release(domain, reply_to, item, out),
        Event::Inbox { item, event, wake } => tracked::inbox(domain, env, item, event, wake, out),
        Event::Placed { item, attempt } => tracked::placed(domain, item, attempt, out),
        Event::Undelivered { item, attempt, event } => tracked::undelivered(domain, item, attempt, event),
        Event::Listed { item, attempt } => tracked::listed(domain, item, attempt),
        Event::Answered { item, attempt, answer } => tracked::answered(domain, env, item, attempt, answer, out),
        Event::Decided { owner, due } => tracked::decided(domain, owner, due, out),
        Event::Written { owner, wrote } => tracked::written(domain, env, owner, wrote, out),
        Event::Recorded { owner, comment } => tracked::recorded(domain, owner, comment, out),
        Event::Applied { owner, applied } => tracked::applied(domain, env, owner, applied, out),
        Event::Acted { owner, acted } => tracked::acted(domain, owner, acted, out),
    }
}

/// Fires the earliest alarm due at `env.now`, if there is one, emitting at most
/// [`max_out`] requests. A stage fires its alarms after its input events, so
/// what arrived in the same iteration wins over a deadline that passed while
/// the loop waited.
pub fn fire(domain: &mut Domain, env: &Env<Limits>, out: &mut Queue<Request>) {
    if let Some(id) = domain.alarms.expire(env.now) {
        tracked::alarm(domain, id, out);
    }
}
