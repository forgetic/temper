//! The model's state and its entry points (section 3). Each hands what it is
//! given to the sub-model it is for, then completes the hand-offs between the
//! sub-models (4.5) before it returns, routing what is for the protocol layer
//! out.
//!
//! Hand-offs between the run and the sessions go both ways, so a chain of them
//! could go on within one step: a session's delegated call that the run
//! answers at once, an answer that lets the session start its next batch, a
//! close that withdraws a call whose sub-agent the run closes in turn. Two of
//! the run's records therefore reach a session only from the ready list
//! (programming-model.md, 2): a `Return`, which the peer's delegated call
//! holds as its answer, and a `Close`, which the peer holds. The loop drains
//! the list with [`resume`] at the start of the model's stage in a later
//! iteration, after the reclaim point, so a session starts at most one batch
//! of delegated calls an iteration, and a close cascades down the sub-agent
//! tree one level an iteration. A session takes either in any state it can be
//! in when it comes (it waits for each delegated call's answer, and takes a
//! close in any state, dropping it once it has ended). The run's `Open` and
//! `Say` go at once: neither leads back (a session just opened or continued
//! makes no call and does not yield in that step), and a `Say` must reach the
//! session while it is still yielded, before its time can run out.
//!
//! So an entry point's hand-offs are at most three deep: the sub-model it is
//! for (a session's step, say), the run's steps for what that emitted, the
//! sessions' steps for what the run opened or said at once, and the run's for
//! what those emitted; [`max_out`] follows from the sub-models' along that
//! chain.

use temper_agent_model_run as run;
use temper_agent_model_session::{self as session, llm as sllm};
use temper_lib::{Env, Id, Map, Queue, Rng, Set, Slab, Time, Token};

use crate::boundary::{Event, Request};
use crate::facts::Fact;
use crate::limits::{self, Limits};
use crate::peer::Peer;
use crate::route;

/// The most requests an entry point emits per call under `limits`: what the
/// sub-models emit in the most steps it takes of each (see the module), as
/// each of their requests is one of ours or a hand-off. The loop reserves this
/// much room in `out` before calling it.
#[must_use]
pub const fn max_out(limits: &Limits) -> u32 {
    limits::run_out(limits).saturating_add(limits::session_out(limits))
}

/// The agent model's state: its sub-models', what it keeps of each
/// conversation between them, and room for what they emit within a step.
#[derive(Debug)]
pub struct Model {
    pub(crate) run: run::Model,
    pub(crate) session: session::Model,
    /// The conversations the run opened, until their sessions end.
    pub(crate) peers: Slab<Peer>,
    /// Peers by the run's token for their conversation, which is their
    /// session's opener, and by their session's own.
    pub(crate) conversations: Map<Token, Id<Peer>>,
    pub(crate) sessions: Map<Token, Id<Peer>>,
    /// Delegated calls between a session's `Delegate` and its answer, by the
    /// session's token for them.
    pub(crate) flights: Map<Token, Flight>,
    pub(crate) ready: Ready,
    /// Tickets the peers hold.
    pub(crate) tickets: u32,
    /// What each sub-model emits in a step, until it is routed. Empty between
    /// steps.
    pub(crate) run_out: Queue<run::Request>,
    pub(crate) session_out: Queue<session::Request>,
    facts: Queue<Fact>,
    lost: u64,
}

/// A delegated call in flight: the run serves it, and the peer's session
/// waits for its answer.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) struct Flight {
    pub(crate) peer: Id<Peer>,
    /// The session withdrew it.
    pub(crate) withdrawn: bool,
    pub(crate) answer: Due,
}

/// What a delegated call's session is answered.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum Due {
    /// The run has not returned it.
    Waiting,
    /// The run's answer, kept under a ticket, waiting on the ready list.
    Answered { answer: sllm::Answer },
    /// The run returned it cancelled, after the session withdrew it.
    Cancelled,
}

/// A hand-off from the run to a session, held by what it is about.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(crate) enum Handoff {
    /// The run closed the peer's conversation.
    Close { peer: Id<Peer> },
    /// The run answered the delegated call the session names `owner`.
    Answer { owner: Token },
}

/// The hand-offs waiting for a later iteration, each at most once: those made
/// before the last reclaim point, which [`resume`] delivers, and those made
/// since, which wait for the next.
#[derive(Debug)]
pub(crate) struct Ready {
    now: Set<Handoff>,
    next: Set<Handoff>,
}

impl Ready {
    const fn with_capacity(capacity: u32) -> Ready {
        Ready { now: Set::with_capacity(capacity), next: Set::with_capacity(capacity) }
    }

    fn is_ready(&self) -> bool {
        !self.now.is_empty()
    }

    fn pop(&mut self) -> Option<Handoff> {
        let handoff = *self.now.first()?;
        self.now.remove(&handoff);
        Some(handoff)
    }

    /// Holds `handoff` until the next iteration.
    pub(crate) fn defer(&mut self, handoff: Handoff) {
        if !self.now.contains(&handoff) {
            self.next.insert(handoff).expect("room on the ready list for every hand-off");
        }
    }

    /// Forgets `handoff`, which is moot.
    pub(crate) fn forget(&mut self, handoff: Handoff) {
        self.now.remove(&handoff);
        self.next.remove(&handoff);
    }

    /// The reclaim point: what was deferred in this iteration may be delivered
    /// in the next.
    fn promote(&mut self) {
        for _ in 0..self.next.capacity() {
            let Some(handoff) = self.next.first().copied() else {
                break;
            };
            self.next.remove(&handoff);
            self.now.insert(handoff).expect("room on the ready list for every hand-off");
        }
    }
}

impl Model {
    /// A model with room for `limits`, which [`crate::worst_case`] accepts,
    /// drawing randomness from `seed`: each sub-model's seed is drawn from it.
    #[must_use]
    pub fn new(limits: &Limits, seed: u64) -> Model {
        let mut rng = Rng::new(seed);
        let peers = limits.run.conversations;
        let flights = limits::flights(limits).expect("worst_case accepted the limits");
        let handoffs = limits::handoffs(limits).expect("worst_case accepted the limits");
        let facts = limits::facts(limits).expect("worst_case accepted the limits");
        Model {
            run: run::Model::new(&limits.run),
            session: session::Model::new(&limits.session, rng.next_u64()),
            peers: Slab::with_capacity(peers),
            conversations: Map::with_capacity(peers),
            sessions: Map::with_capacity(peers),
            flights: Map::with_capacity(flights),
            ready: Ready::with_capacity(handoffs),
            tickets: 0,
            run_out: Queue::with_capacity(limits::run_out(limits)),
            session_out: Queue::with_capacity(limits::session_out(limits)),
            facts: Queue::with_capacity(facts),
            lost: 0,
        }
    }

    /// The run sub-model, for a world to look at.
    #[must_use]
    pub const fn run(&self) -> &run::Model {
        &self.run
    }

    /// The session sub-model, for a world to look at.
    #[must_use]
    pub const fn session(&self) -> &session::Model {
        &self.session
    }

    /// Conversations the run opened whose sessions have not ended, ended ones
    /// included until they are reclaimed.
    #[must_use]
    pub const fn peers(&self) -> u32 {
        self.peers.len()
    }

    /// Delegated calls whose sessions have not been answered.
    #[must_use]
    pub fn flights(&self) -> u32 {
        self.flights.len()
    }

    /// Tickets held for the sessions: the asks of calls not dispatched yet,
    /// and the run's answers.
    #[must_use]
    pub const fn tickets(&self) -> u32 {
        self.tickets
    }

    /// When the earliest alarm falls due.
    #[must_use]
    pub fn next_deadline(&self) -> Option<Time> {
        let session = self.session.next_deadline();
        match self.run.next_deadline() {
            Some(run) => match session {
                Some(session) => Some(run.min(session)),
                None => Some(run),
            },
            None => session,
        }
    }

    /// Whether an alarm is due at `now`. The loop calls [`fire`] while one is.
    #[must_use]
    pub fn is_due(&self, now: Time) -> bool {
        self.run.is_due(now) || self.session.is_due(now)
    }

    /// Whether something waits to go on: a session after a batch its tools
    /// answered at once, or a hand-off from the run. While one does, the loop
    /// calls [`resume`] at the start of the model's stage, before its input
    /// events.
    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.session.is_ready() || self.ready.is_ready()
    }

    /// The oldest fact not drained yet, the run's or the sessions' (which the
    /// tools' are among).
    pub fn pop_fact(&mut self) -> Option<Fact> {
        self.facts.pop()
    }

    /// How many facts were dropped for want of room, the sub-models' included.
    #[must_use]
    pub fn facts_lost(&self) -> u64 {
        self.lost.saturating_add(self.run.facts_lost()).saturating_add(self.session.facts_lost())
    }

    /// The reclaim point: frees what closed in this iteration.
    pub fn reclaim(&mut self) {
        self.run.reclaim();
        self.session.reclaim();
        self.peers.reclaim();
        self.ready.promote();
    }
}

/// Handles one event, emitting at most [`max_out`] requests.
pub fn step(model: &mut Model, env: &Env<Limits>, event: Event, out: &mut Queue<Request>) {
    route::event(model, env, event);
    settle(model, env, out);
}

/// Fires the earliest alarm due at `env.now`, the run's or a session's, if
/// there is one, emitting at most [`max_out`] requests. On a tie, the run's
/// goes first. A stage fires its alarms after its input events, so progress
/// that arrived in the same iteration wins over a deadline that passed while
/// the loop waited.
pub fn fire(model: &mut Model, env: &Env<Limits>, out: &mut Queue<Request>) {
    let run_due = model.run.is_due(env.now);
    let session_due = model.session.is_due(env.now);
    if run_due && (!session_due || model.run.next_deadline() <= model.session.next_deadline()) {
        run::fire(&mut model.run, &route::run_env(env), &mut model.run_out);
    } else if session_due {
        session::fire(&mut model.session, &route::session_env(env), &mut model.session_out);
    }
    settle(model, env, out);
}

/// Goes on with one thing that waits, if there is one, emitting at most
/// [`max_out`] requests: a session after a batch its tools answered within
/// the step that started it, or a hand-off from the run deferred in an earlier
/// iteration.
pub fn resume(model: &mut Model, env: &Env<Limits>, out: &mut Queue<Request>) {
    if model.session.is_ready() {
        session::resume(&mut model.session, &route::session_env(env), &mut model.session_out);
    } else if let Some(handoff) = model.ready.pop() {
        route::deliver(model, env, handoff);
    }
    settle(model, env, out);
}

/// Completes the hand-offs, then gathers the facts.
fn settle(model: &mut Model, env: &Env<Limits>, out: &mut Queue<Request>) {
    route::hand_off(model, env, out);
    gather(model, &env.limits);
}

/// Drains the sub-models' facts into the model's own queue, counting what does
/// not fit.
fn gather(model: &mut Model, limits: &Limits) {
    for _ in 0..limits.run.facts {
        let Some(fact) = model.run.pop_fact() else {
            break;
        };
        keep(model, Fact::Run { fact });
    }
    for _ in 0..limits.session.facts {
        let Some(fact) = model.session.pop_fact() else {
            break;
        };
        keep(model, Fact::Session { fact });
    }
}

fn keep(model: &mut Model, fact: Fact) {
    if model.facts.try_push(fact).is_err() {
        model.lost = model.lost.saturating_add(1);
    }
}
