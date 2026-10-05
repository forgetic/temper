//! The agent child domain's state and its entry points (section 3).

use skein_lib::{Deadlines, Env, Queue, Slab, Time};

use crate::agent::{self, Agent, Alarm};
use crate::boundary::{Event, Request};
use crate::facts::{Fact, Facts};
use crate::limits::{self, Limits};

/// The most requests an entry point emits per call: a spawned process's run is
/// started, and the process waited for, reaped, sent its start message and
/// read. The parent reserves this much room in `out` before calling it.
pub const MAX_OUT: u32 = 5;

/// The agent child domain's state.
#[derive(Debug)]
pub struct Domain {
    /// The process slots.
    pub(crate) agents: Slab<Agent>,
    pub(crate) alarms: Deadlines<Alarm>,
    pub(crate) facts: Facts,
}

impl Domain {
    /// A domain with room for `limits`.
    #[must_use]
    pub fn new(limits: &Limits) -> Domain {
        let alarms = limits::alarms(limits).expect("worst_case accepted the limits");
        Domain {
            agents: Slab::with_capacity(limits.agents),
            alarms: Deadlines::with_capacity(alarms),
            facts: Facts::with_capacity(limits.facts),
        }
    }

    /// Agents present: the slots taken, gone ones included until they are
    /// reclaimed.
    #[must_use]
    pub fn agents(&self) -> u32 {
        self.agents.len()
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
        self.agents.reclaim();
    }
}

/// Handles one event, emitting at most [`MAX_OUT`] requests.
pub fn step(domain: &mut Domain, env: &Env<Limits>, event: Event, out: &mut Queue<Request>) {
    match event {
        Event::SpawnV2 { client, spawn } => agent::spawn_v2(domain, env, client, spawn, out),
        Event::TurnCredit { agent, read } => agent::turn_credit(domain, env, agent, read, out),
        Event::Spawn { client, spawn } => agent::spawn(domain, env, client, spawn, out),
        Event::Deliver { agent, name, event } => agent::deliver(domain, env, agent, name, event, out),
        Event::Answer { agent, call, reply } => agent::answer(domain, env, agent, call, reply, out),
        Event::Grant { agent, grant } => agent::grant(domain, env, agent, grant, out),
        Event::Stop { agent } => agent::stop(domain, env, agent, out),
        Event::Spawned { owner, process } => agent::spawned(domain, env, owner, process, out),
        Event::Unspawned { owner, detail } => agent::unspawned(domain, env, owner, detail, out),
        Event::Sent { owner } => agent::sent(domain, env, owner, out),
        Event::Unsent { owner } => agent::unsent(domain, env, owner, out),
        Event::Received { owner, message } => agent::received(domain, env, owner, message, out),
        Event::Malformed { owner } => agent::malformed(domain, env, owner, out),
        Event::Hangup { owner } => agent::hangup(domain, env, owner, out),
        Event::Signalled { owner } => agent::signalled(domain, env, owner, out),
        Event::Exited { owner } => agent::exited(domain, env, owner, out),
        Event::Reaped { owner, detail } => agent::reaped(domain, env, owner, detail, out),
    }
}

/// Fires the earliest alarm due at `env.now`, if there is one, emitting at most
/// [`MAX_OUT`] requests. A stage fires its alarms after its input events, so
/// progress that arrived in the same iteration wins over a deadline that passed
/// while the loop waited.
pub fn fire(domain: &mut Domain, env: &Env<Limits>, out: &mut Queue<Request>) {
    let Some(alarm) = domain.alarms.expire(env.now) else {
        return;
    };
    match alarm {
        Alarm::Watchdog { agent } => agent::watchdog(domain, env, agent, out),
        Alarm::Wall { agent } => agent::wall(domain, env, agent, out),
        Alarm::Grace { agent } => agent::grace(domain, env, agent, out),
    }
}
