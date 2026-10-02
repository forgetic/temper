use std::collections::{BTreeMap, BTreeSet, VecDeque};

use temper_agent_model_run as run;
use temper_fake_worker_model as worker;
use temper_lib::{Duration, Env, Queue, ReplyTo, Rng, Time, Token};

use crate::partner::{Out, Partner, Script, Tally};
use crate::translate;

/// Room in each model's output queue. Small, so the loop's flow control (take
/// an event only while there is room for what it may produce) is exercised.
const OUT: u32 = 4;

/// Durations drawn uniformly from `min..=max`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Span {
    pub min: Duration,
    pub max: Duration,
}

impl Span {
    #[must_use]
    pub const fn millis(min: u64, max: u64) -> Span {
        Span { min: Duration::from_millis(min), max: Duration::from_millis(max) }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Settings {
    /// Seeds the world, which seeds the worker and the partner.
    pub seed: u64,
    pub run: run::Limits,
    pub worker: worker::Config,
    pub partner: Script,
    /// One-way latency between the worker and the agent.
    pub network: Span,
    /// One-way latency between the run and its conversations. Within the
    /// agent, the top level hands records over in the step that makes them;
    /// a latency here lets them cross in every order.
    pub hop: Span,
    /// What the checkouts hold, and how io works on them.
    pub checkout: Checkouts,
    /// The chance, per mille, that a check or a push in flight wins the race
    /// with its cancel.
    pub races: u32,
}

/// The checkouts the world makes for runs, and io's way with them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Checkouts {
    /// The chance, per mille, that a repository has an `AGENTS.md`, of a
    /// length drawn from `1..=guide_max`.
    pub guides: u32,
    pub guide_max: u32,
    /// The chance, per mille, that a repository has checks, and how many
    /// times they fail before they pass: drawn from `0..=check_failures`.
    pub checks: u32,
    pub check_failures: u32,
    /// How long io takes for each read or probe, and the chance, per mille,
    /// that it fails.
    pub io: Span,
    pub io_failures: u32,
    /// How long checks run.
    pub check: Span,
}

impl Settings {
    /// A world where nothing goes wrong at the agent's side: room for every
    /// run, charters well within the limits, an LLM that works and yields now
    /// and then but never finishes, and no cancels.
    #[must_use]
    pub const fn calm(seed: u64) -> Settings {
        Settings {
            seed,
            run: run::Limits {
                runs: 4,
                conversations: 4,
                run_bytes: 1 << 16,
                repositories: 4,
                outlets: 4,
                verdicts: 4,
                budget: run::Budget {
                    turns: 1000,
                    input: 1 << 32,
                    output: 1 << 32,
                    cache_read: 1 << 32,
                    cache_write: 1 << 32,
                    time: Duration::from_secs(24 * 3600),
                },
                max_tokens: 8192,
                nudges: 2,
                guide_bytes: 1024,
                io_timeout: Duration::from_secs(5),
                outcome_bytes: 4096,
                check_timeout: Duration::from_secs(600),
                check_tail: 256,
            },
            worker: worker::Config {
                jobs: 4,
                window: Duration::from_secs(10),
                cancels: 0,
                cancel_min: Duration::from_secs(1),
                cancel_max: Duration::from_secs(60),
                brief_min: 100,
                brief_max: 2000,
                turns_min: 20,
                turns_max: 40,
                tokens_min: 1 << 20,
                tokens_max: 1 << 21,
                time_min: Duration::from_secs(3600),
                time_max: Duration::from_secs(7200),
                max_tokens: 4096,
                writable: 500,
                changes: 700,
                checks: 800,
                verdicts: 700,
                push_min: Duration::from_millis(10),
                push_max: Duration::from_millis(500),
                moves: 0,
                push_failures: 0,
            },
            partner: Script {
                conversations: 16,
                invalid: 0,
                turn: Span::millis(100, 5_000),
                input: 4_000,
                output: 1_000,
                cache: 2_000,
                faults: 0,
                finishes: 0,
                yields: 200,
                changes: 500,
                good: 1000,
                finish_deadline: Span::millis(600_000, 600_000),
                odd_stops: 0,
                settle: Span::millis(1, 500),
                races: 500,
            },
            network: Span::millis(1, 20),
            hop: Span::millis(0, 2),
            checkout: Checkouts {
                guides: 500,
                guide_max: 2000,
                checks: 500,
                check_failures: 0,
                io: Span::millis(0, 50),
                io_failures: 0,
                check: Span::millis(100, 5_000),
            },
            races: 500,
        }
    }
}

/// What the world counted.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct Stats {
    /// Runs the worker started, and cancels it sent.
    pub starts: u32,
    pub cancels: u32,
    /// Reads, probes and checks the run asked io for, and checks it aborted.
    pub reads: u32,
    pub probes: u32,
    pub checks: u32,
    pub aborts: u32,
    /// Pushes the run asked the worker for, and host calls it cancelled.
    pub pushes: u32,
    pub host_cancels: u32,
    /// Conversations the run opened, nudges it said, and closes it sent.
    pub opens: u32,
    pub says: u32,
    pub closes: u32,
    /// What the partner counted.
    pub partner: Tally,
}

/// Something on its way, delivered at its time.
enum Delivery {
    /// The worker's start reaches the agent.
    Start {
        owner: Token,
        charter: worker::api::Charter,
    },
    /// The worker's cancel reaches the agent.
    Cancel {
        run: Token,
    },
    /// The end of a host call reaches the run.
    Host(run::Event),
    /// The agent's word on a run reaches the worker.
    Admitted {
        owner: Token,
        run: Token,
    },
    Answered {
        owner: Token,
        answer: worker::api::Answer,
    },
    Checking {
        job: Token,
        deadline: Time,
    },
    Push {
        owner: Token,
        job: Token,
        change: worker::api::Change,
    },
    /// The run's requests reach its conversations.
    Open {
        conversation: Token,
        opening: run::Opening,
    },
    Say {
        peer: Token,
    },
    Close {
        peer: Token,
    },
    Return {
        call: Token,
        result: run::Returned,
    },
    /// A conversation's event reaches the run.
    Event(run::Event),
    /// io's answer reaches the run.
    Io(run::Event),
    /// The partner's own timer.
    Wake {
        peer: Token,
        wake: u64,
    },
}

/// The four one-way channels, named by where they lead, each delivering in the
/// order it was given.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Lane {
    Agent,
    Worker,
    Conversations,
    Run,
}

/// A start, as the world tracks it: the budget its run keeps to, and the
/// answer once it has come.
struct Start {
    budget: run::Budget,
    answer: Option<run::Answer>,
}

/// An open, as the world tracks it.
#[derive(Default)]
struct Open {
    started: bool,
    ended: bool,
}

/// A conversation's call, as the world tracks it.
struct Call {
    conversation: Token,
    returned: bool,
}

/// A push, as the world tracks it: for which job, and whether the world, as
/// the protocol layer, has answered its cancel already.
struct Pushing {
    job: Token,
    cancelled: bool,
}

pub struct World {
    now: Time,
    rng: Rng,
    settings: Settings,

    run: run::Model,
    run_env: Env<run::Limits>,
    run_in: VecDeque<run::Event>,
    run_out: Queue<run::Request>,

    worker: worker::Model,
    worker_env: Env<worker::Config>,
    worker_in: VecDeque<worker::Event>,
    worker_out: Queue<worker::Request>,

    partner: Partner,

    /// Deliveries in flight, by time and then by the order they were sent.
    wire: BTreeMap<(Time, u64), Delivery>,
    serial: u64,
    /// When each lane delivers its latest, which the next may not overtake.
    lanes: [Time; 4],
    /// Every start, by the worker's name for it; every open, by the run's
    /// name for the conversation; every conversation's call, by its own name.
    starts: BTreeMap<Token, Start>,
    opens: BTreeMap<Token, Open>,
    calls: BTreeMap<Token, Call>,
    /// The checkouts' files, by their roots and paths, which of them are
    /// executable, and how many more times each repository's checks fail.
    files: BTreeMap<(Token, Vec<u8>), Vec<u8>>,
    executables: BTreeSet<(Token, Vec<u8>)>,
    failures: BTreeMap<Token, u64>,
    /// io's operations in flight, by their owners, and where the result of
    /// each check in flight is on the wire.
    io: BTreeSet<Token>,
    checks: BTreeMap<Token, (Time, u64)>,
    /// Pushes in flight, by the run's owner; and the jobs whose runs pushed a
    /// change.
    pushes: BTreeMap<Token, Pushing>,
    pushed: BTreeSet<Token>,

    stats: Stats,
    trace: Vec<String>,
}

impl World {
    #[must_use]
    pub fn new(settings: Settings) -> World {
        assert!(run::worst_case(&settings.run).is_some(), "the shell refuses limits it cannot provision");
        let mut rng = Rng::new(settings.seed);
        let worker = worker::Model::new(&settings.worker, rng.next_u64());
        let partner = Partner::new(settings.partner, rng.next_u64());
        World {
            now: Time::ZERO,
            rng,
            settings,
            run: run::Model::new(&settings.run),
            run_env: Env { now: Time::ZERO, limits: settings.run },
            run_in: VecDeque::new(),
            run_out: Queue::with_capacity(OUT),
            worker,
            worker_env: Env { now: Time::ZERO, limits: settings.worker },
            worker_in: VecDeque::new(),
            worker_out: Queue::with_capacity(OUT),
            partner,
            wire: BTreeMap::new(),
            serial: 0,
            lanes: [Time::ZERO; 4],
            starts: BTreeMap::new(),
            opens: BTreeMap::new(),
            calls: BTreeMap::new(),
            files: BTreeMap::new(),
            executables: BTreeSet::new(),
            failures: BTreeMap::new(),
            io: BTreeSet::new(),
            checks: BTreeMap::new(),
            pushes: BTreeMap::new(),
            pushed: BTreeSet::new(),
            stats: Stats::default(),
            trace: Vec::new(),
        }
    }

    #[must_use]
    pub fn now(&self) -> Time {
        self.now
    }

    #[must_use]
    pub fn stats(&self) -> Stats {
        Stats { partner: self.partner.tally(), ..self.stats }
    }

    /// What crossed between the models and the world, in order, with times.
    #[must_use]
    pub fn trace(&self) -> &[String] {
        &self.trace
    }

    /// Every run started, with the run's answer once it has come.
    pub fn answers(&self) -> impl Iterator<Item = Option<&run::Answer>> {
        self.starts.values().map(|start| start.answer.as_ref())
    }

    /// Runs until nothing is left to happen, then checks the invariants of a
    /// settled world. Panics if it takes more than `iterations`.
    pub fn run(&mut self, iterations: u32) {
        for _ in 0..iterations {
            self.iterate();
            if self.has_work_now() {
                continue;
            }
            let Some(next) = self.next_time() else {
                self.assert_settled();
                return;
            };
            assert!(next > self.now, "time moves forward");
            self.now = next;
        }
        panic!("the world did not settle in {iterations} iterations");
    }

    /// One iteration of the loop, as the shell would run it.
    fn iterate(&mut self) {
        self.run_env.now = self.now;
        self.worker_env.now = self.now;
        self.deliver();

        // Each stage takes its events, then fires its alarms, while it has room
        // for what one more may produce.
        while self.run_out.room() >= run::MAX_OUT {
            let Some(event) = self.run_in.pop_front() else { break };
            self.log(&format!("run <- {event:?}"));
            run::step(&mut self.run, &self.run_env, event, &mut self.run_out);
        }
        while self.run_out.room() >= run::MAX_OUT && self.run.is_due(self.now) {
            self.log("run alarm");
            run::fire(&mut self.run, &self.run_env, &mut self.run_out);
        }
        while self.worker_out.room() >= worker::MAX_OUT {
            let Some(event) = self.worker_in.pop_front() else { break };
            worker::step(&mut self.worker, &self.worker_env, event, &mut self.worker_out);
        }
        while self.worker_out.room() >= worker::MAX_OUT && self.worker.is_due(self.now) {
            worker::fire(&mut self.worker, &self.worker_env, &mut self.worker_out);
        }

        // What the steps asked for, submitted at the end of the iteration.
        while let Some(request) = self.run_out.pop() {
            self.run_request(request);
        }
        while let Some(request) = self.worker_out.pop() {
            self.worker_request(request);
        }

        // The reclaim point.
        self.run.reclaim();
        self.worker.reclaim();
        assert!(self.run.runs() <= self.settings.run.runs, "runs stay within their slots");
        assert!(self.run.conversations() <= self.settings.run.conversations, "conversations stay within their slots");
        assert!(self.run.calls() <= self.settings.run.conversations, "calls stay within their slots");
    }

    /// The run's requests, carried out the way the top level, the protocol
    /// layer, io and the conversations would.
    fn run_request(&mut self, request: run::Request) {
        self.log(&format!("run -> {request:?}"));
        match request {
            run::Request::Admitted { worker, run } => {
                self.send(Lane::Worker, Delivery::Admitted { owner: worker, run });
            }
            run::Request::Answer { to, answer } => self.answer(to, answer),
            run::Request::Open { conversation, opening } => {
                let fresh = self.opens.insert(conversation, Open::default()).is_none();
                assert!(fresh, "conversations have distinct names");
                self.stats.opens += 1;
                self.send(Lane::Conversations, Delivery::Open { conversation, opening });
            }
            run::Request::Say { peer, text: _ } => {
                self.stats.says += 1;
                self.send(Lane::Conversations, Delivery::Say { peer });
            }
            run::Request::Close { peer } => {
                self.stats.closes += 1;
                self.send(Lane::Conversations, Delivery::Close { peer });
            }
            run::Request::Return { call, result } => {
                let ledger = self.calls.get_mut(&call).expect("a return is of a call that was made");
                assert!(!ledger.returned, "a call returns once");
                ledger.returned = true;
                self.send(Lane::Conversations, Delivery::Return { call, result });
            }
            run::Request::Read { owner, at, max, deadline } => {
                let (at_time, read) = match self.io_result(owner, deadline) {
                    Some(at_time) => {
                        let read = match self.files.get(&(at.root, at.path.to_vec())) {
                            Some(content) => {
                                let max = usize::try_from(max).expect("a u32 fits");
                                let bytes = content[..content.len().min(max)].into();
                                run::Read::Bytes { bytes, whole: content.len() <= max }
                            }
                            None => run::Read::Missing,
                        };
                        (at_time, read)
                    }
                    None => (deadline, run::Read::Failed),
                };
                self.stats.reads += 1;
                self.schedule(at_time, Delivery::Io(run::Event::Read { owner, read }));
            }
            run::Request::Probe { owner, at, deadline } => {
                let (at_time, executable) = match self.io_result(owner, deadline) {
                    Some(at_time) => (at_time, self.executables.contains(&(at.root, at.path.to_vec()))),
                    None => (deadline, false),
                };
                self.stats.probes += 1;
                self.schedule(at_time, Delivery::Io(run::Event::Probed { owner, executable }));
            }
            run::Request::Check { owner, program, deadline, tail } => self.check(owner, &program, deadline, tail),
            run::Request::Abort { owner } => {
                self.stats.aborts += 1;
                // A check whose result is on its way already won the race.
                if let Some(key) = self.checks.get(&owner).copied()
                    && !self.rng.chance(self.settings.races)
                {
                    self.wire.remove(&key).expect("a check in flight has its result on the wire");
                    self.checks.remove(&owner);
                    let at = self.now.saturating_add(self.draw(self.settings.checkout.io));
                    self.schedule(at, Delivery::Io(run::Event::Aborted { owner }));
                }
            }
            run::Request::Checking { worker, deadline } => {
                self.send(Lane::Worker, Delivery::Checking { job: worker, deadline });
            }
            run::Request::Push { worker, owner, change } => {
                let fresh = self.pushes.insert(owner, Pushing { job: worker, cancelled: false }).is_none();
                assert!(fresh, "a host call is in flight once");
                self.stats.pushes += 1;
                self.send(Lane::Worker, Delivery::Push { owner, job: worker, change: translate::change(change) });
            }
            run::Request::CancelHost { owner } => {
                self.stats.host_cancels += 1;
                // A push the worker has answered already won the race; the
                // protocol layer may also wait for one that is about to.
                if let Some(pushing) = self.pushes.get_mut(&owner)
                    && !pushing.cancelled
                    && !self.rng.chance(self.settings.races)
                {
                    pushing.cancelled = true;
                    self.send(Lane::Agent, Delivery::Host(run::Event::HostCancelled { owner }));
                }
            }
        }
    }

    /// The run's answer, checked against its budget and what it did.
    fn answer(&mut self, to: ReplyTo, answer: run::Answer) {
        let owner = to.into_token();
        let start = self.starts.get_mut(&owner).expect("an answer is to a start that was made");
        assert!(start.answer.is_none(), "a start is answered once");
        assert_within(&start.budget, &answer, self.partner.turn_max());
        if let run::Answer::Accepted { outcome: run::outcome::Declared::Change(_), .. } = &answer {
            assert!(self.pushed.contains(&owner), "a change is accepted only once it is pushed");
        }
        let translated = translate::answer(&answer);
        start.answer = Some(answer);
        self.send(Lane::Worker, Delivery::Answered { owner, answer: translated });
    }

    /// Runs the checks at `program` as io would: they fail as many times as
    /// their repository was given, then pass, unless they outlast `deadline`.
    fn check(&mut self, owner: Token, program: &run::Place, deadline: Time, tail: u32) {
        assert!(self.io.insert(owner), "a call has one check in flight at a time");
        assert!(self.executables.contains(&(program.root, program.path.to_vec())), "checks are run where found");
        self.stats.checks += 1;
        let at = self.now.saturating_add(self.draw(self.settings.checkout.check));
        let failures = self.failures.entry(program.root).or_insert(0);
        let (at, exit, written) = if at > deadline {
            (deadline, run::Exit::TimedOut, 100)
        } else if *failures > 0 {
            *failures -= 1;
            (at, run::Exit::Code { code: 1 }, self.rng.between(0, u64::from(tail) * 3))
        } else {
            (at, run::Exit::Code { code: 0 }, 20)
        };
        // io keeps the tail of what the checks wrote.
        let kept = written.min(u64::from(tail));
        let output =
            b"test parse_tabs ... FAILED\n".iter().copied().cycle().take(usize::try_from(kept).expect("small"));
        let ran = run::Ran { exit, output: output.collect(), cut: written - kept };
        let key = self.schedule(at, Delivery::Io(run::Event::Checked { owner, ran }));
        self.checks.insert(owner, key);
    }

    /// When io answers a read or a probe of `owner`'s due by `deadline`, or
    /// `None` if it fails or runs out of time.
    fn io_result(&mut self, owner: Token, deadline: Time) -> Option<Time> {
        assert!(self.io.insert(owner), "a run has one look in flight at a time");
        let at = self.now.saturating_add(self.draw(self.settings.checkout.io));
        let failed = self.rng.chance(self.settings.checkout.io_failures);
        (!failed && at <= deadline).then_some(at)
    }

    /// Makes a checkout of `repositories` repositories: their roots, and what
    /// each holds.
    fn checkout(&mut self, repositories: usize) -> Vec<Token> {
        let settings = self.settings.checkout;
        let mut roots = Vec::new();
        for _ in 0..repositories {
            self.serial += 1;
            let root = Token::new(self.serial);
            if self.rng.chance(settings.guides) {
                let len = usize::try_from(self.rng.between(1, u64::from(settings.guide_max))).expect("small");
                let guide = b"Keep changes small, and run the tests. ".iter().copied().cycle().take(len).collect();
                self.files.insert((root, b"AGENTS.md".to_vec()), guide);
            }
            if self.rng.chance(settings.checks) {
                self.executables.insert((root, b".temper/pre-pr".to_vec()));
                self.failures.insert(root, self.rng.between(0, u64::from(settings.check_failures)));
            }
            roots.push(root);
        }
        roots
    }

    /// The worker's requests, carried to the agent the way the two protocol
    /// layers would.
    fn worker_request(&mut self, request: worker::Request) {
        match request {
            worker::Request::Start { owner, charter } => {
                let start = Start { budget: translate::budget(charter.budget), answer: None };
                assert!(self.starts.insert(owner, start).is_none(), "jobs have distinct names");
                self.stats.starts += 1;
                self.send(Lane::Agent, Delivery::Start { owner, charter });
            }
            worker::Request::Cancel { run } => {
                self.stats.cancels += 1;
                self.send(Lane::Agent, Delivery::Cancel { run });
            }
            worker::Request::Pushed { to, pushed } => {
                let owner = to.into_token();
                let Pushing { job, cancelled } = self.pushes.remove(&owner).expect("the worker answers a push made");
                // The cancel was answered already: the answer is late.
                if cancelled {
                    return;
                }
                let push = translate::push(pushed);
                if push == run::Push::Done {
                    self.pushed.insert(job);
                }
                self.send(Lane::Agent, Delivery::Host(run::Event::Pushed { owner, push }));
            }
        }
    }

    /// Hands every delivery that is due to its destination.
    fn deliver(&mut self) {
        while let Some(entry) = self.wire.first_entry() {
            if entry.key().0 > self.now {
                break;
            }
            match entry.remove() {
                Delivery::Start { owner, charter } => {
                    let roots = self.checkout(charter.repositories.len());
                    let charter = translate::charter(charter, &roots);
                    self.run_in.push_back(run::Event::Start { reply_to: ReplyTo::new(owner), worker: owner, charter });
                }
                Delivery::Cancel { run } => self.run_in.push_back(run::Event::Cancel { run }),
                Delivery::Host(event) => self.run_in.push_back(event),
                Delivery::Admitted { owner, run } => self.worker_in.push_back(worker::Event::Admitted { owner, run }),
                Delivery::Answered { owner, answer } => {
                    self.worker_in.push_back(worker::Event::Answered { owner, answer });
                }
                Delivery::Checking { job, deadline } => {
                    self.worker_in.push_back(worker::Event::Checking { job, deadline });
                }
                Delivery::Push { owner, job, change } => {
                    self.worker_in.push_back(worker::Event::Push { reply_to: ReplyTo::new(owner), job, change });
                }
                Delivery::Open { conversation, opening } => {
                    let mut out = Vec::new();
                    self.partner.open(self.now, conversation, &opening, &mut out);
                    self.partner_out(out);
                }
                Delivery::Say { peer } => {
                    let mut out = Vec::new();
                    self.partner.say(self.now, peer, &mut out);
                    self.partner_out(out);
                }
                Delivery::Close { peer } => {
                    let mut out = Vec::new();
                    self.partner.close(self.now, peer, &mut out);
                    self.partner_out(out);
                }
                Delivery::Return { call, result } => {
                    let mut out = Vec::new();
                    self.partner.returned(self.now, call, &result, &mut out);
                    self.partner_out(out);
                }
                Delivery::Wake { peer, wake } => {
                    let mut out = Vec::new();
                    self.partner.woken(self.now, peer, wake, &mut out);
                    self.partner_out(out);
                }
                Delivery::Event(event) => {
                    self.check_conversation(&event);
                    self.run_in.push_back(event);
                }
                Delivery::Io(event) => {
                    let (run::Event::Read { owner, .. }
                    | run::Event::Probed { owner, .. }
                    | run::Event::Checked { owner, .. }
                    | run::Event::Aborted { owner }) = &event
                    else {
                        unreachable!("io answers reads, probes and checks");
                    };
                    assert!(self.io.remove(owner), "io answers each operation once");
                    self.checks.remove(owner);
                    self.run_in.push_back(event);
                }
            }
        }
    }

    /// What the partner asked for: events on their way to the run, and wakes.
    fn partner_out(&mut self, out: Vec<Out>) {
        for item in out {
            match item {
                Out::Event(event) => self.send(Lane::Run, Delivery::Event(event)),
                Out::Wake { at, peer, wake } => {
                    self.schedule(at, Delivery::Wake { peer, wake });
                }
            }
        }
    }

    /// The conversations' contract, as the run receives it: `Started` at most
    /// once and first, one `Ended` per open once every call it made has
    /// returned, nothing after it; a withdraw only of a call it made.
    fn check_conversation(&mut self, event: &run::Event) {
        let (conversation, started, ended) = match event {
            run::Event::Started { conversation, .. } => (conversation, true, false),
            run::Event::Ended { conversation, .. } => (conversation, false, true),
            run::Event::Yielded { conversation, .. } | run::Event::Used { conversation, .. } => {
                (conversation, false, false)
            }
            run::Event::Delegated { conversation, call, .. } => {
                let fresh = self.calls.insert(*call, Call { conversation: *conversation, returned: false }).is_none();
                assert!(fresh, "calls have distinct names");
                (conversation, false, false)
            }
            run::Event::Withdraw { conversation, call } => {
                let ledger = self.calls.get(call).expect("a withdraw is of a call that was made");
                assert_eq!(ledger.conversation, *conversation, "a conversation withdraws its own calls");
                (conversation, false, false)
            }
            run::Event::Start { .. }
            | run::Event::Cancel { .. }
            | run::Event::Read { .. }
            | run::Event::Probed { .. }
            | run::Event::Checked { .. }
            | run::Event::Aborted { .. }
            | run::Event::Pushed { .. }
            | run::Event::HostCancelled { .. } => unreachable!("not a conversation's event"),
        };
        let open = self.opens.get_mut(conversation).expect("events are about conversations the run opened");
        assert!(!open.ended, "nothing comes after a conversation's end");
        if started {
            assert!(!open.started, "a conversation starts once");
            open.started = true;
        }
        if !started && !ended {
            assert!(open.started, "a conversation starts before anything else");
        }
        open.ended = ended;
        if ended {
            for ledger in self.calls.values() {
                assert!(
                    ledger.conversation != *conversation || ledger.returned,
                    "a conversation ends once its calls have returned"
                );
            }
        }
    }

    fn has_work_now(&self) -> bool {
        !self.run_in.is_empty()
            || !self.worker_in.is_empty()
            || self.run.is_due(self.now)
            || self.worker.is_due(self.now)
            || self.wire.first_key_value().is_some_and(|((at, _), _)| *at <= self.now)
    }

    fn next_time(&self) -> Option<Time> {
        let wire = self.wire.first_key_value().map(|((at, _), _)| *at);
        [wire, self.run.next_deadline(), self.worker.next_deadline()].into_iter().flatten().min()
    }

    /// The invariants of a world where nothing is left to happen.
    fn assert_settled(&self) {
        assert_eq!(self.run.runs(), 0, "every run has answered and been reclaimed");
        assert_eq!(self.run.conversations(), 0, "every conversation has ended and been reclaimed");
        assert_eq!(self.run.calls(), 0, "every call has returned and been reclaimed");
        assert_eq!(self.run.next_deadline(), None, "no alarm outlives its run");
        assert_eq!(self.worker.jobs(), 0, "the worker took every answer");
        assert_eq!(self.worker.pushes(), 0, "the worker answered every push");
        assert_eq!(self.worker.answered(), self.settings.worker.jobs, "every job was started and answered");
        assert_eq!(self.partner.live(), 0, "every conversation has ended");
        assert!(self.io.is_empty() && self.checks.is_empty(), "io answered every operation");
        assert!(self.pushes.is_empty(), "every push was answered");
        assert!(self.wire.is_empty() && self.run_in.is_empty() && self.worker_in.is_empty(), "nothing is on its way");
        let mut answered = run::Spend::ZERO;
        for (owner, start) in &self.starts {
            let answer = start.answer.as_ref().unwrap_or_else(|| panic!("start {owner:?} was answered"));
            match answer {
                run::Answer::Failed { spent, .. } | run::Answer::Accepted { spent, .. } => {
                    answered = answered.saturating_add(*spent);
                }
                run::Answer::Refused(_) => {}
            }
        }
        for (conversation, open) in &self.opens {
            assert!(open.ended, "conversation {conversation:?} ended");
        }
        for (call, ledger) in &self.calls {
            assert!(ledger.returned, "call {call:?} returned");
        }
        assert_eq!(answered, self.partner.spent(), "every turn spent is in exactly one answer");
    }

    /// Sends on `lane`, after its latency, and after whatever it carries
    /// already.
    fn send(&mut self, lane: Lane, delivery: Delivery) {
        let span = match lane {
            Lane::Agent | Lane::Worker => self.settings.network,
            Lane::Conversations | Lane::Run => self.settings.hop,
        };
        let latency = self.draw(span);
        let last = &mut self.lanes[lane as usize];
        let at = self.now.saturating_add(latency).max(*last);
        *last = at;
        self.schedule(at, delivery);
    }

    fn schedule(&mut self, at: Time, delivery: Delivery) -> (Time, u64) {
        self.serial += 1;
        let key = (at, self.serial);
        self.wire.insert(key, delivery);
        key
    }

    fn draw(&mut self, span: Span) -> Duration {
        Duration::from_nanos(self.rng.between(span.min.as_nanos(), span.max.as_nanos()))
    }

    fn log(&mut self, line: &str) {
        self.trace.push(format!("{:>16} {line}", self.now.as_nanos()));
    }
}

/// A run spends within its budget, give or take one turn in flight: a
/// conversation keeps to its share of turns, and may go past its share of
/// tokens by the turn that crossed it.
fn assert_within(budget: &run::Budget, answer: &run::Answer, turn: run::Spend) {
    let (run::Answer::Failed { spent, .. } | run::Answer::Accepted { spent, .. }) = answer else { return };
    assert!(spent.turns <= budget.turns, "{spent:?} keeps to the turns of {budget:?}");
    let over = |spent: u64, budget: u64, turn: u64| spent <= budget.saturating_add(turn);
    assert!(
        over(spent.input, budget.input, turn.input)
            && over(spent.output, budget.output, turn.output)
            && over(spent.cache_read, budget.cache_read, turn.cache_read)
            && over(spent.cache_write, budget.cache_write, turn.cache_write),
        "{spent:?} is within {budget:?} and one turn of at most {turn:?}"
    );
}
