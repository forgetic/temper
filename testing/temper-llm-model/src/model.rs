//! The fake provider's state and its entry points.

use core::mem;

use temper_lib::{Deadlines, Duration, Env, Id, Queue, ReplyTo, Rng, Slab, Time};

use crate::api::{Answer, Error, Query};
use crate::respond;

/// The most requests an entry point emits per call.
pub const MAX_OUT: u32 = 1;

/// How the fake behaves, handed to every step read-only.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Config {
    /// Calls held at once. A call beyond them is refused as overloaded.
    pub calls: u32,
    /// The time to answer is drawn from `latency_min..=latency_max`.
    pub latency_min: Duration,
    pub latency_max: Duration,
    /// The chance, per mille, that a call fails as overloaded.
    pub overloaded: u32,
    /// The chance, per mille, that a call fails as rate-limited.
    pub rate_limited: u32,
    /// What a rate-limit failure asks the client to wait.
    pub retry_after: Duration,
    /// The chances, per mille, that a call fails as unavailable, as too long
    /// for the context window, or as unauthorised.
    pub unavailable: u32,
    pub too_long: u32,
    pub unauthorized: u32,
    /// The chance, per mille, that an answer is refused by the content filter.
    pub refused: u32,
    /// The chance, per mille, that an answer says it calls tools and calls
    /// none.
    pub no_calls: u32,
    /// The most tokens a final answer takes: each takes between one and this
    /// many, and is cut short at the query's `max_tokens`.
    pub answer_tokens: u32,
    /// The most tool calls an answer makes: each that makes some makes
    /// between one and this many.
    pub calls_per_answer: u32,
    /// The chance, per mille, that a tool call is malformed: it names a tool
    /// that was not offered, or its arguments are not an object or lack the
    /// path.
    pub malformed: u32,
    /// Rounds of tool calls after each of the client's messages before the
    /// fake answers it.
    pub tool_rounds: u32,
}

/// protocol -> model
#[derive(PartialEq, Eq, Debug)]
pub enum Event {
    /// A call: answer `query`.
    Call { reply_to: ReplyTo, query: Query },
}

/// model -> protocol
#[derive(PartialEq, Eq, Debug)]
pub enum Request {
    /// The answer to a `Call`: exactly one per call.
    Reply { to: ReplyTo, result: Result<Answer, Error> },
}

/// The fake provider's state.
#[derive(Debug)]
pub struct Model {
    calls: Slab<Call>,
    /// When each call is answered.
    timers: Deadlines<Id<Call>>,
    rng: Rng,
    /// Tool call ids issued.
    minted: u64,
}

/// A call being answered.
#[derive(Debug)]
struct Call {
    state: State,
}

#[derive(Debug)]
enum State {
    /// The answer is decided, and goes out when the call's timer fires.
    Thinking { reply_to: ReplyTo, result: Result<Answer, Error> },
    /// Terminal: holds nothing.
    Closed,
}

impl Model {
    #[must_use]
    pub fn new(config: &Config, seed: u64) -> Model {
        Model {
            calls: Slab::with_capacity(config.calls),
            timers: Deadlines::with_capacity(config.calls),
            rng: Rng::new(seed),
            minted: 0,
        }
    }

    /// Calls present, answered ones included until they are reclaimed.
    #[must_use]
    pub fn calls(&self) -> u32 {
        self.calls.len()
    }

    #[must_use]
    pub fn next_deadline(&self) -> Option<Time> {
        self.timers.next()
    }

    #[must_use]
    pub fn is_due(&self, now: Time) -> bool {
        match self.timers.next() {
            Some(at) => at <= now,
            None => false,
        }
    }

    pub fn reclaim(&mut self) {
        self.calls.reclaim();
    }
}

/// Handles one event, emitting at most [`MAX_OUT`] requests.
pub fn step(model: &mut Model, env: &Env<Config>, event: Event, out: &mut Queue<Request>) {
    match event {
        Event::Call { reply_to, query } => call(model, env, reply_to, &query, out),
    }
}

/// Answers the earliest call due at `env.now`, if there is one.
pub fn fire(model: &mut Model, env: &Env<Config>, out: &mut Queue<Request>) {
    let Some(id) = model.timers.expire(env.now) else {
        return;
    };
    let call = model.calls.get_mut(id).expect("a call lives until its timer fires");
    let state = mem::replace(&mut call.state, State::Closed);
    call.state = match state {
        State::Thinking { reply_to, result } => answer(reply_to, result, out),
        State::Closed => unreachable!("a closed call has no timer"),
    };
    model.calls.retire(id);
}

fn call(model: &mut Model, env: &Env<Config>, reply_to: ReplyTo, query: &Query, out: &mut Queue<Request>) {
    if model.calls.is_full() {
        out.push(Request::Reply { to: reply_to, result: Err(Error::Overloaded) });
        return;
    }
    let config = &env.limits;
    let result = respond::respond(&mut model.rng, &mut model.minted, config, query);
    let latency = model.rng.between(config.latency_min.as_nanos(), config.latency_max.as_nanos());
    let call = Call { state: State::Thinking { reply_to, result } };
    let id = model.calls.insert(call).expect("checked for room above");
    let at = env.now.saturating_add(Duration::from_nanos(latency));
    model.timers.arm(id, at).expect("one timer per call fits");
}

/// Thinking, timer: the answer goes out.
fn answer(reply_to: ReplyTo, result: Result<Answer, Error>, out: &mut Queue<Request>) -> State {
    out.push(Request::Reply { to: reply_to, result });
    State::Closed
}
