//! The fake worker's state and its entry points.
//!
//! A job's transition table:
//!
//! ```text
//! state       event or alarm   next        emits
//! Waiting     start alarm      Starting    start
//! Starting    admitted         Running     (the cancel alarm, if it is to be cancelled)
//!             answered         Closed
//! Running     cancel alarm     Cancelling  cancel
//!             answered         Closed
//! Cancelling  answered         Closed
//! ```

use core::mem;

use temper_lib::{Deadlines, Duration, Env, Id, Queue, Rng, Slab, Time, Token};

use crate::api::{Answer, Charter};
use crate::charter;

/// The most requests an entry point emits per call.
pub const MAX_OUT: u32 = 1;

/// How the fake behaves, handed to every step read-only.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Config {
    /// Runs to start, one per job.
    pub jobs: u32,
    /// Each job starts at a time drawn from the first `window`.
    pub window: Duration,
    /// The chance, per mille, that a run is cancelled once it is admitted,
    /// after a delay drawn from `cancel_min..=cancel_max`.
    pub cancels: u32,
    pub cancel_min: Duration,
    pub cancel_max: Duration,
    /// The length of each brief, drawn from `brief_min..=brief_max`.
    pub brief_min: u32,
    pub brief_max: u32,
    /// Each budget's turns, tokens of every kind, and time, each drawn from its
    /// range.
    pub turns_min: u32,
    pub turns_max: u32,
    pub tokens_min: u64,
    pub tokens_max: u64,
    pub time_min: Duration,
    pub time_max: Duration,
    /// What each charter's LLM asks for as `max_tokens`.
    pub max_tokens: u32,
}

/// protocol -> model
#[derive(PartialEq, Eq, Debug)]
pub enum Event {
    /// The agent took the run of the job `owner`, and calls it `run`.
    Admitted { owner: Token, run: Token },
    /// Terminal for `Start`: the agent's answer for the job `owner`.
    Answered { owner: Token, answer: Answer },
}

/// model -> protocol
#[derive(PartialEq, Eq, Debug)]
pub enum Request {
    /// Start a run on `charter` for the job `owner`. Exactly one answer comes.
    Start { owner: Token, charter: Charter },
    /// Cancel the run `run`. Its answer still comes.
    Cancel { run: Token },
}

/// The fake worker's state.
#[derive(Debug)]
pub struct Model {
    jobs: Slab<Job>,
    /// When each job starts, or its run is cancelled.
    timers: Deadlines<Id<Job>>,
    rng: Rng,
    /// Answers taken.
    answered: u32,
}

#[derive(Debug)]
struct Job {
    state: State,
}

#[derive(Debug)]
enum State {
    /// Its start alarm is armed.
    Waiting,
    /// Its run is started, and not admitted yet.
    Starting,
    /// Its run is admitted as `run`. Its timer, if armed, cancels it.
    Running { run: Token },
    /// Its run is cancelled, and not answered yet.
    Cancelling,
    /// Terminal: holds nothing.
    Closed,
}

impl Model {
    /// A worker with `config.jobs` jobs to start, from time zero.
    #[must_use]
    pub fn new(config: &Config, seed: u64) -> Model {
        let mut model = Model {
            jobs: Slab::with_capacity(config.jobs),
            timers: Deadlines::with_capacity(config.jobs),
            rng: Rng::new(seed),
            answered: 0,
        };
        for _ in 0..config.jobs {
            let id = model.jobs.insert(Job { state: State::Waiting }).expect("a slot per job");
            let at = Time::ZERO.saturating_add(Duration::from_nanos(model.rng.below(config.window.as_nanos())));
            model.timers.arm(id, at).expect("a timer per job");
        }
        model
    }

    /// Jobs not answered yet, answered ones included until they are reclaimed.
    #[must_use]
    pub fn jobs(&self) -> u32 {
        self.jobs.len()
    }

    #[must_use]
    pub fn answered(&self) -> u32 {
        self.answered
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
        self.jobs.reclaim();
    }
}

/// Handles one event, emitting at most [`MAX_OUT`] requests.
pub fn step(model: &mut Model, env: &Env<Config>, event: Event, _out: &mut Queue<Request>) {
    match event {
        Event::Admitted { owner, run } => admitted(model, env, owner, run),
        Event::Answered { owner, answer: _ } => answered(model, owner),
    }
}

/// Starts the job, or cancels the run, whose timer is the earliest due at
/// `env.now`, if there is one.
pub fn fire(model: &mut Model, env: &Env<Config>, out: &mut Queue<Request>) {
    let Some(id) = model.timers.expire(env.now) else {
        return;
    };
    let Model { jobs, timers: _, rng, answered: _ } = model;
    let job = jobs.get_mut(id).expect("a job lives until its run is answered");
    let state = mem::replace(&mut job.state, State::Closed);
    job.state = match state {
        State::Waiting => {
            out.push(Request::Start { owner: id.token(), charter: charter::draw(rng, &env.limits) });
            State::Starting
        }
        State::Running { run } => {
            out.push(Request::Cancel { run });
            State::Cancelling
        }
        State::Starting | State::Cancelling | State::Closed => unreachable!("timers run only while waiting or running"),
    };
}

/// Starting, admitted: the run may be cancelled later.
fn admitted(model: &mut Model, env: &Env<Config>, owner: Token, run: Token) {
    let Model { jobs, timers, rng, answered: _ } = model;
    let id = Id::from_token(owner);
    let config = &env.limits;
    let job = jobs.get_mut(id).expect("a job lives until its run is answered");
    let state = mem::replace(&mut job.state, State::Closed);
    job.state = match state {
        State::Starting => {
            if rng.chance(config.cancels) {
                let delay = rng.between(config.cancel_min.as_nanos(), config.cancel_max.as_nanos());
                timers.arm(id, env.now.saturating_add(Duration::from_nanos(delay))).expect("a timer per job");
            }
            State::Running { run }
        }
        State::Waiting | State::Running { .. } | State::Cancelling | State::Closed => {
            unreachable!("a run is admitted once, after it starts and before its answer")
        }
    };
}

/// Any state with a run, answered: the job is done.
fn answered(model: &mut Model, owner: Token) {
    let id = Id::from_token(owner);
    let job = model.jobs.get_mut(id).expect("a job lives until its run is answered");
    let state = mem::replace(&mut job.state, State::Closed);
    match state {
        State::Starting | State::Running { .. } | State::Cancelling => {}
        State::Waiting | State::Closed => unreachable!("a run is answered once, after it starts"),
    }
    model.timers.cancel(id);
    model.jobs.retire(id);
    model.answered = model.answered.saturating_add(1);
}
