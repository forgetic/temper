//! The fake worker's state and its entry points.
//!
//! A job's transition table:
//!
//! ```text
//! state       event or alarm   next        emits
//! Waiting     start alarm      Starting    start
//! Starting    admitted         Running     (the cancel alarm, if it is to be cancelled)
//!             answered         Closed
//! Running     cancel alarm     Cancelling  cancel (the alarm again, to cancel twice)
//!             answered         Closed      (or Lingering, to cancel late)
//! Cancelling  cancel alarm     Cancelling  cancel
//!             answered         Closed      (or Lingering)
//! Lingering   cancel alarm     Closed      cancel
//! ```
//!
//! A late cancel names a run that has answered, and a second one a run that
//! has decided how it ends: the agent ignores both, which is what they test.
//!
//! A push is answered after a latency drawn from the configuration: the
//! branch has moved, for as many pushes as the job drew when it started;
//! then, with the configured chance, the push fails; otherwise it is done.

use core::mem;

use temper_lib::{Deadlines, Duration, Env, Id, Queue, ReplyTo, Rng, Slab, Time, Token};

use crate::api::{Answer, Change, Charter, Pushed};
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
    /// The chances, per mille, that a run is cancelled again after a cancel,
    /// and that a run is cancelled after it has answered, each after a delay
    /// drawn from `cancel_min..=cancel_max`.
    pub recancels: u32,
    pub late_cancels: u32,
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
    /// The chances, per mille, that a charter's first repository is writable,
    /// that its outcome may be a change, that a change must pass its checks,
    /// and that its outcome may be a verdict. One that may be neither may be
    /// a change.
    pub writable: u32,
    pub changes: u32,
    pub checks: u32,
    pub verdicts: u32,
    /// The time to push, drawn from `push_min..=push_max`.
    pub push_min: Duration,
    pub push_max: Duration,
    /// How many of a job's pushes find the branch moved: drawn from
    /// `0..=moves` when it starts.
    pub moves: u32,
    /// The chance, per mille, that a push fails.
    pub push_failures: u32,
}

/// protocol -> model
#[derive(PartialEq, Eq, Debug)]
pub enum Event {
    /// The agent took the run of the job `owner`, and calls it `run`.
    Admitted { owner: Token, run: Token },
    /// Terminal for `Start`: the agent's answer for the job `owner`.
    Answered { owner: Token, answer: Answer },
    /// A call from the run of the job `job`: push `change`. Answered by one
    /// `Pushed`.
    Push { reply_to: ReplyTo, job: Token, change: Change },
    /// The run of the job `job` runs checks until `deadline` at the latest.
    Checking { job: Token, deadline: Time },
}

/// model -> protocol
#[derive(PartialEq, Eq, Debug)]
pub enum Request {
    /// Start a run on `charter` for the job `owner`. Exactly one answer comes.
    Start { owner: Token, charter: Charter },
    /// Cancel the run `run`. Its answer still comes.
    Cancel { run: Token },
    /// The answer to a `Push`: exactly one per push.
    Pushed { to: ReplyTo, pushed: Pushed },
}

/// The fake worker's state.
#[derive(Debug)]
pub struct Model {
    jobs: Slab<Job>,
    pushes: Slab<Pushing>,
    /// When each job starts or its run is cancelled, and when each push is
    /// answered.
    timers: Deadlines<Alarm>,
    rng: Rng,
    /// Answers taken, pushes served, and check notices heard.
    answered: u32,
    pushed: u32,
    checking: u32,
}

#[derive(Debug)]
struct Job {
    /// Pushes left that find the branch moved.
    moves: u32,
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
    /// Its run `run` is cancelled, and not answered yet. Its timer, if armed,
    /// cancels it again.
    Cancelling { run: Token },
    /// Its run `run` has answered; its timer cancels it all the same.
    Lingering { run: Token },
    /// Terminal: holds nothing.
    Closed,
}

/// A push being served.
#[derive(Debug)]
enum Pushing {
    /// Its answer is decided, and goes out when its timer fires.
    Waiting { reply_to: ReplyTo, pushed: Pushed },
    /// Terminal: holds nothing.
    Closed,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Alarm {
    Job(Id<Job>),
    Push(Id<Pushing>),
}

impl Model {
    /// A worker with `config.jobs` jobs to start, from time zero.
    #[must_use]
    pub fn new(config: &Config, seed: u64) -> Model {
        let mut model = Model {
            jobs: Slab::with_capacity(config.jobs),
            pushes: Slab::with_capacity(config.jobs),
            timers: Deadlines::with_capacity(config.jobs.saturating_mul(2)),
            rng: Rng::new(seed),
            answered: 0,
            pushed: 0,
            checking: 0,
        };
        for _ in 0..config.jobs {
            let moves =
                u32::try_from(model.rng.below(u64::from(config.moves).saturating_add(1))).expect("drawn below a u32");
            let id = model.jobs.insert(Job { moves, state: State::Waiting }).expect("a slot per job");
            let at = Time::ZERO.saturating_add(Duration::from_nanos(model.rng.below(config.window.as_nanos())));
            model.timers.arm(Alarm::Job(id), at).expect("a timer per job");
        }
        model
    }

    /// Jobs not answered yet, answered ones included until they are reclaimed.
    #[must_use]
    pub fn jobs(&self) -> u32 {
        self.jobs.len()
    }

    /// Pushes being served, answered ones included until they are reclaimed.
    #[must_use]
    pub fn pushes(&self) -> u32 {
        self.pushes.len()
    }

    #[must_use]
    pub fn answered(&self) -> u32 {
        self.answered
    }

    #[must_use]
    pub fn pushed(&self) -> u32 {
        self.pushed
    }

    #[must_use]
    pub fn checking(&self) -> u32 {
        self.checking
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
        self.pushes.reclaim();
    }
}

/// Handles one event, emitting at most [`MAX_OUT`] requests.
pub fn step(model: &mut Model, env: &Env<Config>, event: Event, _out: &mut Queue<Request>) {
    match event {
        Event::Admitted { owner, run } => admitted(model, env, owner, run),
        Event::Answered { owner, answer: _ } => answered(model, env, owner),
        Event::Push { reply_to, job, change: _ } => push(model, env, reply_to, job),
        Event::Checking { job: _, deadline: _ } => model.checking = model.checking.saturating_add(1),
    }
}

/// Starts the job, cancels the run, or answers the push whose timer is the
/// earliest due at `env.now`, if there is one.
pub fn fire(model: &mut Model, env: &Env<Config>, out: &mut Queue<Request>) {
    let Some(alarm) = model.timers.expire(env.now) else {
        return;
    };
    match alarm {
        Alarm::Job(id) => job_alarm(model, env, id, out),
        Alarm::Push(id) => {
            let pushing = model.pushes.get_mut(id).expect("a push lives until its timer fires");
            *pushing = match mem::replace(pushing, Pushing::Closed) {
                Pushing::Waiting { reply_to, pushed } => {
                    out.push(Request::Pushed { to: reply_to, pushed });
                    Pushing::Closed
                }
                Pushing::Closed => unreachable!("a closed push has no timer"),
            };
            model.pushes.retire(id);
        }
    }
}

fn job_alarm(model: &mut Model, env: &Env<Config>, id: Id<Job>, out: &mut Queue<Request>) {
    let Model { jobs, timers, rng, .. } = model;
    let config = &env.limits;
    let job = jobs.get_mut(id).expect("a job lives until its last cancel");
    let state = mem::replace(&mut job.state, State::Closed);
    job.state = match state {
        State::Waiting => {
            out.push(Request::Start { owner: id.token(), charter: charter::draw(rng, config) });
            State::Starting
        }
        State::Running { run } => {
            out.push(Request::Cancel { run });
            if rng.chance(config.recancels) {
                let delay = rng.between(config.cancel_min.as_nanos(), config.cancel_max.as_nanos());
                let at = env.now.saturating_add(Duration::from_nanos(delay));
                timers.arm(Alarm::Job(id), at).expect("a timer per job");
            }
            State::Cancelling { run }
        }
        State::Cancelling { run } => {
            out.push(Request::Cancel { run });
            State::Cancelling { run }
        }
        State::Lingering { run } => {
            out.push(Request::Cancel { run });
            State::Closed
        }
        State::Starting | State::Closed => unreachable!("timers run only while waiting, running or lingering"),
    };
    follow(jobs, id);
}

/// Retires a job once it is Closed.
fn follow(jobs: &mut Slab<Job>, id: Id<Job>) {
    let job = jobs.get(id).expect("a job lives until it is retired");
    match job.state {
        State::Closed => jobs.retire(id),
        State::Waiting
        | State::Starting
        | State::Running { .. }
        | State::Cancelling { .. }
        | State::Lingering { .. } => {}
    }
}

/// Starting, admitted: the run may be cancelled later.
fn admitted(model: &mut Model, env: &Env<Config>, owner: Token, run: Token) {
    let Model { jobs, timers, rng, .. } = model;
    let id = Id::from_token(owner);
    let config = &env.limits;
    let job = jobs.get_mut(id).expect("a job lives until its run is answered");
    let state = mem::replace(&mut job.state, State::Closed);
    job.state = match state {
        State::Starting => {
            if rng.chance(config.cancels) {
                let delay = rng.between(config.cancel_min.as_nanos(), config.cancel_max.as_nanos());
                let at = env.now.saturating_add(Duration::from_nanos(delay));
                timers.arm(Alarm::Job(id), at).expect("a timer per job");
            }
            State::Running { run }
        }
        State::Waiting | State::Running { .. } | State::Cancelling { .. } | State::Lingering { .. } | State::Closed => {
            unreachable!("a run is admitted once, after it starts and before its answer")
        }
    };
}

/// Any state with a run, answered: the job is done, unless it is to cancel its
/// run late.
fn answered(model: &mut Model, env: &Env<Config>, owner: Token) {
    let config = &env.limits;
    let id = Id::from_token(owner);
    let job = model.jobs.get_mut(id).expect("a job lives until its run is answered");
    model.timers.cancel(Alarm::Job(id));
    model.answered = model.answered.saturating_add(1);
    let state = mem::replace(&mut job.state, State::Closed);
    job.state = match state {
        State::Running { run } | State::Cancelling { run } if model.rng.chance(config.late_cancels) => {
            let delay = model.rng.between(config.cancel_min.as_nanos(), config.cancel_max.as_nanos());
            let at = env.now.saturating_add(Duration::from_nanos(delay));
            model.timers.arm(Alarm::Job(id), at).expect("a timer per job");
            State::Lingering { run }
        }
        State::Starting | State::Running { .. } | State::Cancelling { .. } => State::Closed,
        State::Waiting | State::Lingering { .. } | State::Closed => {
            unreachable!("a run is answered once, after it starts")
        }
    };
    follow(&mut model.jobs, id);
}

/// A push from the run of `job`: decided now, answered later.
fn push(model: &mut Model, env: &Env<Config>, reply_to: ReplyTo, job: Token) {
    let config = &env.limits;
    let job = model.jobs.get_mut(Id::from_token(job)).expect("a job lives until its run is answered");
    match job.state {
        State::Running { .. } | State::Cancelling { .. } => {}
        State::Waiting | State::Starting | State::Lingering { .. } | State::Closed => {
            unreachable!("a run pushes once it is admitted, before it answers")
        }
    }
    let pushed = if job.moves > 0 {
        job.moves = job.moves.saturating_sub(1);
        Pushed::Moved
    } else if model.rng.chance(config.push_failures) {
        Pushed::Failed
    } else {
        Pushed::Done
    };
    model.pushed = model.pushed.saturating_add(1);
    let id = model.pushes.insert(Pushing::Waiting { reply_to, pushed }).expect("a run pushes one change at a time");
    let latency = model.rng.between(config.push_min.as_nanos(), config.push_max.as_nanos());
    let at = env.now.saturating_add(Duration::from_nanos(latency));
    model.timers.arm(Alarm::Push(id), at).expect("a timer per push");
}
