//! A scripted host: what the worker would be to the runs, played from a seed.
//!
//! It speaks the run's vocabulary for the worker, as the top level will route
//! it to and from the protocol layer, and plays the worker beyond it, with
//! some liberties a real worker does not take:
//!
//! - It has jobs to run, and starts a run for each at a time drawn from the
//!   first `window`, on a charter drawn from the script. It checks out the
//!   charter's repositories itself, and names each by the root io knows it
//!   by.
//! - It cancels some of the runs once they are admitted, after a drawn
//!   delay; some of those again, and some once they have answered. A second
//!   cancel names a run that has decided how it ends, and a late one a run
//!   that has answered: the run ignores both, which is what they test.
//! - It answers each push after a drawn latency: the branch has moved, for
//!   every push of a job that drew so when it started (a push is a
//!   fast-forward from where the run started, so a moved branch stays
//!   moved); else, with the configured chance, the push fails; otherwise it
//!   is done. A push withdrawn before then (`CancelHost`) is answered as
//!   cancelled, and changes nothing; one withdrawn after is not in flight,
//!   and the withdraw changes nothing.
//! - It counts the check notices it hears.
//!
//! A job's table:
//!
//! ```text
//! state       event or alarm   next        emits
//! Waiting     start alarm      Starting    Start
//! Starting    admitted         Running     (the cancel alarm, if it is to be cancelled)
//!             answered         gone
//! Running     cancel alarm     Cancelling  Cancel (the alarm again, to cancel twice)
//!             answered         gone        (or Lingering, to cancel late)
//! Cancelling  cancel alarm     Cancelling  Cancel
//!             answered         gone        (or Lingering)
//! Lingering   cancel alarm     gone        Cancel
//! ```
//!
//! It checks as it goes that each start is admitted at most once, before its
//! answer, and answered exactly once; that a run pushes and tells of its
//! checks only once admitted and before it answers; and that it names its
//! pushes apart.
//!
//! What it does at a later time it keeps alarms for, which the world fires.

use std::collections::{BTreeMap, BTreeSet};

use temper_agent_model_run::charter::{Checkout, Endpoint, Grants, Llm, Outlet, Repository, Tools};
use temper_agent_model_run::outcome::{ChangeSpec, Children, OutcomeSpec, VerdictRule};
use temper_agent_model_run::{Budget, Charter, Event, Push};
use temper_lib::{Duration, ReplyTo, Rng, Time, Token};

use temper_world::Span;

/// What a brief says, over and over.
const TEXT: &[u8] = b"Fix the failing test in the parser, and keep the change small. ";

/// How the host behaves.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Script {
    /// Runs to start, one per job, each at a time drawn from the first
    /// `window`.
    pub jobs: u32,
    pub window: Duration,
    /// The chance, per mille, that a run is cancelled once it is admitted,
    /// after a delay drawn from `cancel`.
    pub cancels: u32,
    pub cancel: Span,
    /// The chances, per mille, that a run is cancelled again after a cancel,
    /// and that a run is cancelled after it has answered, each after a delay
    /// drawn from `cancel`.
    pub recancels: u32,
    pub late_cancels: u32,
    /// The length of each brief, drawn from `brief_min..=brief_max`.
    pub brief_min: u32,
    pub brief_max: u32,
    /// Each budget's turns and tokens of every kind, each drawn from its
    /// range, and its time, drawn from `time`.
    pub turns_min: u32,
    pub turns_max: u32,
    pub tokens_min: u64,
    pub tokens_max: u64,
    pub time: Span,
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
    /// The chance, per mille, that a charter grants sub-agents.
    pub agents: u32,
    /// The time to push.
    pub push: Span,
    /// The chance, per mille, that a job's branch moves before its run
    /// pushes: every push of that job finds it moved.
    pub moved: u32,
    /// The chance, per mille, that a push fails.
    pub push_failures: u32,
}

/// What the host counted.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct Tally {
    /// Answers it took, pushes it served, pushes withdrawn while in flight,
    /// and check notices it heard.
    pub answered: u32,
    pub pushes: u32,
    pub withdrawn: u32,
    pub notices: u32,
}

pub struct Host {
    script: Script,
    rng: Rng,
    /// The jobs not done yet, by their names, which are their runs' names at
    /// the host.
    jobs: BTreeMap<Token, Job>,
    /// The pushes in flight, by the runs' names for them, and the job each
    /// is of.
    pushes: BTreeMap<Token, Token>,
    /// When each job starts or its run is cancelled, and when each push is
    /// answered: in order, and by what they are for.
    alarms: BTreeSet<(Time, Alarm)>,
    armed: BTreeMap<Alarm, Time>,
    /// Names for jobs and repositories.
    names: u64,
    tally: Tally,
}

struct Job {
    /// Whether its branch moved before its run pushed.
    moved: bool,
    state: State,
}

/// A job that is not done: once done, it is gone.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum State {
    /// Its start alarm is armed.
    Waiting,
    /// Its run is started, and not admitted yet.
    Starting,
    /// Its run is admitted as `run`. Its alarm, if armed, cancels it.
    Running { run: Token },
    /// Its run `run` is cancelled, and has not answered. Its alarm, if armed,
    /// cancels it again.
    Cancelling { run: Token },
    /// Its run `run` has answered; its alarm cancels it all the same.
    Lingering { run: Token },
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Alarm {
    Job(Token),
    Push(Token),
}

impl Host {
    /// A host with `script.jobs` jobs to start, from time zero.
    #[must_use]
    pub fn new(script: Script, seed: u64) -> Host {
        let mut host = Host {
            script,
            rng: Rng::new(seed),
            jobs: BTreeMap::new(),
            pushes: BTreeMap::new(),
            alarms: BTreeSet::new(),
            armed: BTreeMap::new(),
            names: 0,
            tally: Tally::default(),
        };
        for _ in 0..script.jobs {
            let moved = host.rng.chance(script.moved);
            let job = host.name();
            host.jobs.insert(job, Job { moved, state: State::Waiting });
            let at = Time::ZERO.saturating_add(Duration::from_nanos(host.rng.below(script.window.as_nanos())));
            host.arm(Alarm::Job(job), at);
        }
        host
    }

    #[must_use]
    pub fn tally(&self) -> Tally {
        self.tally
    }

    #[must_use]
    pub fn next_deadline(&self) -> Option<Time> {
        self.alarms.first().map(|(at, _)| *at)
    }

    #[must_use]
    pub fn is_due(&self, now: Time) -> bool {
        self.next_deadline().is_some_and(|at| at <= now)
    }

    /// Starts the jobs, cancels the runs and answers the pushes whose alarms
    /// are due at `now`, in the order they fall due.
    pub fn fire(&mut self, now: Time, out: &mut Vec<Event>) {
        while let Some(&(at, alarm)) = self.alarms.first()
            && at <= now
        {
            self.disarm(alarm);
            match alarm {
                Alarm::Job(job) => self.job_alarm(now, job, out),
                Alarm::Push(owner) => self.answer_push(owner, out),
            }
        }
    }

    /// Starting, admitted: the run may be cancelled later.
    pub fn admitted(&mut self, now: Time, job: Token, run: Token) {
        let state = self.state(job);
        match state {
            State::Starting => {
                if self.rng.chance(self.script.cancels) {
                    self.cancel_later(now, job);
                }
                self.set(job, State::Running { run });
            }
            State::Waiting | State::Running { .. } | State::Cancelling { .. } | State::Lingering { .. } => {
                panic!("a run is admitted once, after it starts and before its answer: {state:?}")
            }
        }
    }

    /// Any state with a run, answered: the job is done, unless it is to
    /// cancel its run late.
    pub fn answered(&mut self, now: Time, job: Token) {
        let state = self.state(job);
        self.disarm(Alarm::Job(job));
        self.tally.answered += 1;
        match state {
            State::Running { run } | State::Cancelling { run } if self.rng.chance(self.script.late_cancels) => {
                self.cancel_later(now, job);
                self.set(job, State::Lingering { run });
            }
            State::Starting | State::Running { .. } | State::Cancelling { .. } => {
                self.jobs.remove(&job);
            }
            State::Waiting | State::Lingering { .. } => panic!("a run is answered once, after it starts: {state:?}"),
        }
    }

    /// The run of `job` runs its checks: the host's watchdog would wait for
    /// them.
    pub fn checking(&mut self, job: Token) {
        self.assert_running(job, "a run tells of its checks once admitted, before it answers");
        self.tally.notices += 1;
    }

    /// A push from the run of `job`, which names it `owner`: answered later.
    pub fn push(&mut self, now: Time, job: Token, owner: Token) {
        self.assert_running(job, "a run pushes once admitted, before it answers");
        self.tally.pushes += 1;
        assert!(self.pushes.insert(owner, job).is_none(), "a run names its pushes apart");
        let at = now.saturating_add(self.script.push.draw(&mut self.rng));
        self.arm(Alarm::Push(owner), at);
    }

    /// The run withdraws its push `owner`: if it is in flight, it is answered
    /// as cancelled, and its outcome is never decided.
    pub fn cancel_host(&mut self, owner: Token, out: &mut Vec<Event>) {
        if self.pushes.remove(&owner).is_none() {
            return;
        }
        self.disarm(Alarm::Push(owner));
        self.tally.withdrawn += 1;
        out.push(Event::HostCancelled { owner });
    }

    /// The host's side of a settled world: every job started and answered,
    /// every late cancel sent, and every push answered.
    pub fn assert_settled(&self) {
        assert!(self.jobs.is_empty(), "the host took every answer and sent every cancel");
        assert!(self.pushes.is_empty(), "the host answered every push");
        assert!(self.alarms.is_empty() && self.armed.is_empty(), "no alarm outlives what it is for");
        assert_eq!(self.tally.answered, self.script.jobs, "every job was started and answered");
    }

    fn job_alarm(&mut self, now: Time, job: Token, out: &mut Vec<Event>) {
        let state = self.state(job);
        match state {
            State::Waiting => {
                let charter = self.charter();
                out.push(Event::Start { reply_to: ReplyTo::new(job), worker: job, charter });
                self.set(job, State::Starting);
            }
            State::Running { run } => {
                out.push(Event::Cancel { run });
                if self.rng.chance(self.script.recancels) {
                    self.cancel_later(now, job);
                }
                self.set(job, State::Cancelling { run });
            }
            State::Cancelling { run } => out.push(Event::Cancel { run }),
            State::Lingering { run } => {
                out.push(Event::Cancel { run });
                self.jobs.remove(&job);
            }
            State::Starting => unreachable!("no alarm is armed while a run starts"),
        }
    }

    /// The push `owner`'s latency has passed: it is decided.
    fn answer_push(&mut self, owner: Token, out: &mut Vec<Event>) {
        let job = self.pushes.remove(&owner).expect("a push is in flight until its alarm fires");
        let moved = self.jobs.get(&job).expect("a job lives until its run, which waits on its push, answers").moved;
        let push = if moved {
            Push::Moved
        } else if self.rng.chance(self.script.push_failures) {
            Push::Failed
        } else {
            Push::Done
        };
        out.push(Event::Pushed { owner, push });
    }

    fn assert_running(&self, job: Token, contract: &str) {
        let state = self.state(job);
        match state {
            State::Running { .. } | State::Cancelling { .. } => {}
            State::Waiting | State::Starting | State::Lingering { .. } => panic!("{contract}: {state:?}"),
        }
    }

    fn cancel_later(&mut self, now: Time, job: Token) {
        let at = now.saturating_add(self.script.cancel.draw(&mut self.rng));
        self.arm(Alarm::Job(job), at);
    }

    fn state(&self, job: Token) -> State {
        self.jobs.get(&job).expect("a job lives until its run has answered and its last cancel is sent").state
    }

    fn set(&mut self, job: Token, state: State) {
        self.jobs.get_mut(&job).expect("a job lives until it is done").state = state;
    }

    fn arm(&mut self, alarm: Alarm, at: Time) {
        assert!(self.armed.insert(alarm, at).is_none(), "one alarm at a time for each job and push");
        self.alarms.insert((at, alarm));
    }

    fn disarm(&mut self, alarm: Alarm) {
        if let Some(at) = self.armed.remove(&alarm) {
            self.alarms.remove(&(at, alarm));
        }
    }

    fn name(&mut self) -> Token {
        self.names += 1;
        Token::new(self.names)
    }

    /// A charter drawn from the script:
    ///
    /// - The brief is a text of a length drawn from its range.
    /// - The checkout is one or two repositories, the first writable with the
    ///   configured chance.
    /// - Reading is always granted; writing, the shell, forge reads and a
    ///   "comment" outlet each at random; sub-agents with the configured
    ///   chance, with two more models listed for them.
    /// - The outcome is a change, the verdicts "approve" (no children) and
    ///   "request-changes" (one to eight children, each "blocking" or a
    ///   "nit", with a "path" and a "body"), or either, with the configured
    ///   chances; so is whether a change must pass its checks. The partner's
    ///   outcomes that fit are made for these.
    /// - The budget's turns, tokens and time are drawn from their ranges.
    fn charter(&mut self) -> Charter {
        let script = self.script;
        let len = self.rng.between(u64::from(script.brief_min), u64::from(script.brief_max));
        let brief = TEXT.iter().copied().cycle().take(usize::try_from(len).expect("a u32 fits")).collect();
        let writable = self.rng.chance(script.writable);
        let mut repositories = vec![Repository { name: Box::from(&b"temper"[..]), root: self.name(), writable }];
        if self.rng.chance(500) {
            repositories.push(Repository { name: Box::from(&b"docs"[..]), root: self.name(), writable: false });
        }
        let tools = Tools { inspect: true, modify: self.rng.chance(500), shell: self.rng.chance(500) };
        let forge = self.rng.chance(500);
        let outlets = if self.rng.chance(500) { vec![Outlet { name: Box::from(&b"comment"[..]) }] } else { Vec::new() };
        let verdicts = if self.rng.chance(script.verdicts) { verdicts() } else { Vec::new() };
        let change = self.rng.chance(script.changes) || verdicts.is_empty();
        let checks = change && self.rng.chance(script.checks);
        let tokens = |rng: &mut Rng| rng.between(script.tokens_min, script.tokens_max);
        let budget = Budget {
            turns: u32::try_from(self.rng.between(u64::from(script.turns_min), u64::from(script.turns_max)))
                .expect("drawn between two u32s"),
            input: tokens(&mut self.rng),
            output: tokens(&mut self.rng),
            cache_read: tokens(&mut self.rng),
            cache_write: tokens(&mut self.rng),
            time: script.time.draw(&mut self.rng),
        };
        let agents = self.rng.chance(script.agents);
        let llm = |model: &[u8]| Llm { endpoint: Endpoint(0), model: Box::from(model), max_tokens: script.max_tokens };
        Charter {
            brief,
            checkout: Checkout { repositories: repositories.into() },
            grants: Grants { tools, forge, agents, outlets: outlets.into() },
            outcome: OutcomeSpec { change: change.then_some(ChangeSpec { checks }), verdicts: verdicts.into() },
            budget,
            llm: llm(b"fake-1"),
            models: Box::new([llm(b"fake-2"), llm(b"fake-3")]),
        }
    }
}

fn verdicts() -> Vec<VerdictRule> {
    let approve = VerdictRule {
        name: Box::from(&b"approve"[..]),
        children: Children { min: 0, max: 0 },
        kinds: Box::new([]),
        fields: Box::new([]),
    };
    let request = VerdictRule {
        name: Box::from(&b"request-changes"[..]),
        children: Children { min: 1, max: 8 },
        kinds: Box::new([Box::from(&b"blocking"[..]), Box::from(&b"nit"[..])]),
        fields: Box::new([Box::from(&b"path"[..]), Box::from(&b"body"[..])]),
    };
    vec![approve, request]
}
