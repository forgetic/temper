//! The domain's state and its entry points (section 3). Each hands what it is
//! given to the child domain or the link it is for, then completes the
//! hand-offs between the child domains (4.5) before it returns, routing what is
//! for the protocol layer out.
//!
//! The host is the hub, and its capabilities answer it: a hand-off goes from a
//! capability to the host (an agent child domain's step tells the host at most
//! two things, a checkout's one, as their boundaries say), from the host to a
//! capability, and at most once more back and forth, when a capability answers
//! a request of the host at once (the agent child domain bounces an event it
//! cannot take, the checkout refuses a prepare, or ends a push or a save with
//! nothing to do) and the host answers the engine, replies to the run or
//! releases the workspace, which leads back to it no more. No hand-off waits
//! on a ready list: an entry point completes them all, and [`max_out`]
//! follows from the child domains' along that chain (the `limits` module).

use jig_worker_host as host;
use skein_lib::{Env, Id, Map, Queue, Slab, Time, Token};
use temper_worker_domain_agent as agent;
use temper_worker_domain_checkout as checkout;

use crate::boundary::{Event, Request, Told};
use crate::facts::Fact;
use crate::limits::{self, Limits};
use crate::link::{Fired, Link};
use crate::route;
use crate::workspace::{Items, Workspace};

/// The most requests an entry point emits per call under `limits`: what the
/// child domains emit in the most steps it takes of each (see the module), as
/// each of their requests is one of ours or a hand-off; and, on connecting,
/// the answers kept, one for each slot at most, and the relays and bounces
/// kept while the engine was out of reach.
/// The loop reserves this much room in `out` before calling it.
#[must_use]
pub const fn max_out(limits: &Limits) -> u32 {
    let bounces = limits.host.slots.saturating_mul(limits.host.held.saturating_add(limits.agent.events));
    limits::routed(limits)
        .saturating_add(limits.host.slots)
        .saturating_add(limits.stalled)
        .saturating_add(bounces)
        .saturating_add(limits.host.slots.saturating_mul(limits.turns))
}

/// The worker domain's state: its child domains', the engine link, what it
/// keeps of each workspace between the host and the checkout, and room for what
/// the child domains emit within a step.
#[derive(Debug)]
pub struct Domain {
    pub(crate) host: host::Domain,
    pub(crate) checkout: checkout::Domain,
    pub(crate) agent: agent::Domain,
    pub(crate) link: Link,
    /// The workspaces the host asked for, until the checkout has released
    /// them.
    pub(crate) workspaces: Slab<Workspace>,
    /// Application workspace items retained while the host owns each run.
    pub(crate) items: Slab<Items>,
    pub(crate) items_by_run: Map<Token, Id<Items>>,
    /// Workspaces being prepared, by the host's token for their run.
    pub(crate) preparing: Map<Token, Id<Workspace>>,
    /// What each child domain emits in a step, until it is routed. Empty
    /// between steps.
    pub(crate) host_out: Queue<host::Request>,
    pub(crate) checkout_out: Queue<checkout::Request>,
    pub(crate) agent_out: Queue<agent::Request>,
    /// The run's facts for the engine, and how many did not fit.
    facts: Queue<Fact>,
    lost: u64,
}

impl Domain {
    /// A domain with room for `limits`, which [`crate::worst_case`] accepts,
    /// drawing randomness from `seed`. It dials the engine at once: its first
    /// alarm is due from the start.
    #[must_use]
    pub fn new(limits: &Limits, seed: u64) -> Domain {
        let slots = limits.host.slots;
        let facts = limits::facts(limits).expect("worst_case accepted the limits");
        Domain {
            host: host::Domain::new(&limits.host),
            checkout: checkout::Domain::new(&limits.checkout),
            agent: agent::Domain::new(&limits.agent),
            link: Link::new(limits, seed),
            workspaces: Slab::with_capacity(slots),
            items: Slab::with_capacity(slots.saturating_add(1)),
            items_by_run: Map::with_capacity(slots.saturating_add(1)),
            preparing: Map::with_capacity(slots),
            host_out: Queue::with_capacity(limits::host_out(limits)),
            checkout_out: Queue::with_capacity(limits::checkout_out(limits)),
            agent_out: Queue::with_capacity(limits::agent_out(limits)),
            facts: Queue::with_capacity(facts),
            lost: 0,
        }
    }

    /// The host child domain, for a world to look at.
    #[must_use]
    pub const fn host(&self) -> &host::Domain {
        &self.host
    }

    /// The checkout child domain, for a world to look at.
    #[must_use]
    pub const fn checkout(&self) -> &checkout::Domain {
        &self.checkout
    }

    /// The agent child domain, for a world to look at.
    #[must_use]
    pub const fn agent(&self) -> &agent::Domain {
        &self.agent
    }

    /// Workspaces the host asked for that the checkout has not released,
    /// released ones included until they are reclaimed.
    #[must_use]
    pub const fn workspaces(&self) -> u32 {
        self.workspaces.len()
    }

    /// Whether the channel to the engine is open.
    #[must_use]
    pub fn is_connected(&self) -> bool {
        self.link.is_up()
    }

    /// Answers the engine has yet to acknowledge: sent, or waiting for a
    /// channel. No more than the slots.
    #[must_use]
    pub fn held(&self) -> u32 {
        self.link.held()
    }

    /// Turns retained until commitment acknowledgement.
    #[must_use]
    pub fn retained_turns(&self) -> u32 {
        self.link.retained_turns()
    }

    /// Relays and bounces waiting for a channel to the engine.
    #[must_use]
    pub fn stalled(&self) -> u32 {
        self.link.stalled()
    }

    /// Turns and answers given up by a worker shutting down with the engine
    /// out of reach past the grace, since the domain was made.
    #[must_use]
    pub const fn abandoned(&self) -> u64 {
        self.link.abandoned()
    }

    /// Whether the worker has shut down: told to, every run has answered, and
    /// the engine has every answer, or it was given up. The shell stops then.
    #[must_use]
    pub fn is_done(&self) -> bool {
        self.link.is_shut() && self.host.hosted() == 0 && self.link.held() == 0
    }

    /// When the earliest alarm falls due: the link's, or an agent's.
    #[must_use]
    pub fn next_deadline(&self) -> Option<Time> {
        let agent = self.agent.next_deadline();
        match self.link.next_deadline() {
            Some(link) => match agent {
                Some(agent) => Some(link.min(agent)),
                None => Some(link),
            },
            None => agent,
        }
    }

    /// Whether an alarm is due at `now`. The loop calls [`fire`] while one is.
    #[must_use]
    pub fn is_due(&self, now: Time) -> bool {
        match self.next_deadline() {
            Some(at) => at <= now,
            None => false,
        }
    }

    /// Whether the host has runs to cancel, one at a time. While it does, the
    /// loop calls [`resume`] at the start of the domain's stage, before its
    /// input events.
    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.host.is_ready()
    }

    /// The oldest fact not drained yet, a child domain's or the link's.
    pub fn pop_fact(&mut self) -> Option<Fact> {
        self.facts.pop()
    }

    /// How many facts were dropped for want of room, the child domains'
    /// included.
    #[must_use]
    pub fn facts_lost(&self) -> u64 {
        self.lost
            .saturating_add(self.host.facts_lost())
            .saturating_add(self.checkout.facts_lost())
            .saturating_add(self.agent.facts_lost())
    }

    /// The oldest of the run's facts for the engine not taken yet, while the
    /// channel is open: the protocol layer sends it best effort, after what
    /// the domain emitted, so that nothing goes before the hello (the step the
    /// channel opens in emits it). While the channel is down, facts wait, and
    /// what does not fit is dropped and counted.
    pub fn pop_told(&mut self) -> Option<Told> {
        if !self.link.is_up() {
            return None;
        }
        match self.host.pop_told() {
            Some(told) => Some(Told { run: told.run, attempt: told.attempt, fact: told.fact }),
            None => None,
        }
    }

    /// How many of the run's facts were dropped for want of room.
    #[must_use]
    pub fn told_lost(&self) -> u64 {
        self.host.told_lost()
    }

    /// The reclaim point: frees what closed in this iteration.
    pub fn reclaim(&mut self) {
        self.host.reclaim();
        self.checkout.reclaim();
        self.agent.reclaim();
        self.workspaces.reclaim();
        self.items.reclaim();
    }
}

/// Keeps `fact` if there is room for it, and counts it otherwise.
pub(crate) fn keep(domain: &mut Domain, fact: Fact) {
    if domain.facts.try_push(fact).is_err() {
        domain.lost = domain.lost.saturating_add(1);
    }
}

/// Handles one event, emitting at most [`max_out`] requests.
pub fn step(domain: &mut Domain, env: &Env<Limits>, event: Event, out: &mut Queue<Request>) {
    route::event(domain, env, event, out);
    settle(domain, env, out);
}

/// Fires the earliest alarm due at `env.now`, the link's or an agent's, if
/// there is one, emitting at most [`max_out`] requests. On a tie, the link's
/// goes first. A stage fires its alarms after its input events, so progress
/// that arrived in the same iteration wins over a deadline that passed while
/// the loop waited.
pub fn fire(domain: &mut Domain, env: &Env<Limits>, out: &mut Queue<Request>) {
    let link_due = match domain.link.next_deadline() {
        Some(at) => at <= env.now,
        None => false,
    };
    let agent_due = domain.agent.is_due(env.now);
    if link_due && (!agent_due || domain.link.next_deadline() <= domain.agent.next_deadline()) {
        match domain.link.fire(env, out) {
            Some(Fired::Dialled | Fired::Turn) | None => {}
            Some(Fired::Grace) => {
                keep(domain, Fact::Grace);
                let cancel = host::Event::CancelAll { reason: host::Reason::Contact };
                route::host_step(domain, env, cancel);
            }
        }
    } else if agent_due {
        assert!(domain.agent_out.room() >= agent::MAX_OUT, "an entry point steps the agents no more than its bound");
        agent::fire(&mut domain.agent, &route::agent_env(env), &mut domain.agent_out);
    }
    settle(domain, env, out);
}

/// Cancels one run the host has on its ready list, if it has one, emitting at
/// most [`max_out`] requests.
pub fn resume(domain: &mut Domain, env: &Env<Limits>, out: &mut Queue<Request>) {
    assert!(
        domain.host_out.room() >= host::max_out(&env.limits.host),
        "an entry point steps the host within its bound"
    );
    host::resume(&mut domain.host, &route::host_env(env), &mut domain.host_out);
    settle(domain, env, out);
}

/// Completes the hand-offs, then gathers the facts. A worker shutting down
/// with no run left gives up the answers it cannot deliver, if the engine is
/// out of reach past the grace.
fn settle(domain: &mut Domain, env: &Env<Limits>, out: &mut Queue<Request>) {
    route::hand_off(domain, env, out);
    if domain.host.unanswered() == 0 {
        domain.link.give_up();
    }
    gather(domain, &env.limits);
}

/// Drains the child domains' facts into the domain's own queue, counting what
/// does not fit.
fn gather(domain: &mut Domain, limits: &Limits) {
    for _ in 0..limits.host.facts {
        let Some(fact) = domain.host.pop_fact() else {
            break;
        };
        keep(domain, Fact::Host { fact });
    }
    for _ in 0..limits.checkout.facts {
        let Some(fact) = domain.checkout.pop_fact() else {
            break;
        };
        keep(domain, Fact::Checkout { fact });
    }
    for _ in 0..limits.agent.facts {
        let Some(fact) = domain.agent.pop_fact() else {
            break;
        };
        keep(domain, Fact::Agent { fact });
    }
}
