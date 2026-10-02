//! The agent sub-model's state and its entry points (section 3).

use temper_lib::{Deadlines, Env, Queue, Slab, Time};

use crate::agent::{self, Agent, Alarm};
use crate::boundary::{Event, Request};
use crate::facts::{Fact, Facts};
use crate::limits::{self, Limits};

/// The most requests an entry point emits per call: a spawned process's run is
/// started, and the process waited for, reaped, sent its start message and
/// read. The parent reserves this much room in `out` before calling it.
pub const MAX_OUT: u32 = 5;

/// The agent sub-model's state.
#[derive(Debug)]
pub struct Model {
    /// The process slots.
    pub(crate) agents: Slab<Agent>,
    pub(crate) alarms: Deadlines<Alarm>,
    pub(crate) facts: Facts,
}

impl Model {
    /// A model with room for `limits`.
    #[must_use]
    pub fn new(limits: &Limits) -> Model {
        let alarms = limits::alarms(limits).expect("worst_case accepted the limits");
        Model {
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
    /// top-level model, which calls [`fire`].
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

    /// How many facts were dropped for want of room, since the model was
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
pub fn step(model: &mut Model, env: &Env<Limits>, event: Event, out: &mut Queue<Request>) {
    match event {
        Event::Spawn { client, spawn } => agent::spawn(model, env, client, spawn, out),
        Event::Deliver { agent, event } => agent::deliver(model, env, agent, event, out),
        Event::Answer { agent, call, reply } => agent::answer(model, env, agent, call, reply, out),
        Event::Stop { agent } => agent::stop(model, env, agent, out),
        Event::Spawned { owner, process } => agent::spawned(model, env, owner, process, out),
        Event::Unspawned { owner, detail } => agent::unspawned(model, env, owner, detail, out),
        Event::Sent { owner } => agent::sent(model, env, owner, out),
        Event::Unsent { owner } => agent::unsent(model, env, owner, out),
        Event::Received { owner, message } => agent::received(model, env, owner, message, out),
        Event::Malformed { owner } => agent::malformed(model, env, owner, out),
        Event::Hangup { owner } => agent::hangup(model, env, owner, out),
        Event::Signalled { owner } => agent::signalled(model, env, owner, out),
        Event::Exited { owner } => agent::exited(model, env, owner, out),
        Event::Reaped { owner, detail } => agent::reaped(model, env, owner, detail, out),
    }
}

/// Fires the earliest alarm due at `env.now`, if there is one, emitting at most
/// [`MAX_OUT`] requests. A stage fires its alarms after its input events, so
/// progress that arrived in the same iteration wins over a deadline that passed
/// while the loop waited.
pub fn fire(model: &mut Model, env: &Env<Limits>, out: &mut Queue<Request>) {
    let Some(alarm) = model.alarms.expire(env.now) else {
        return;
    };
    match alarm {
        Alarm::Watchdog { agent } => agent::watchdog(model, env, agent, out),
        Alarm::Wall { agent } => agent::wall(model, env, agent, out),
        Alarm::Grace { agent } => agent::grace(model, env, agent, out),
    }
}
