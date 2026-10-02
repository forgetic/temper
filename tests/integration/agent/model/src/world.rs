use std::collections::{BTreeMap, BTreeSet};

use temper_agent_model::run::facts::{self as run_facts, Return};
use temper_agent_model::run::outcome::Declared;
use temper_agent_model::run::{self, Spend};
use temper_agent_model::tools::{Done, Fault, Op};
use temper_agent_model::{self as agent, Event, Fact, Limits, Request, session, tools};
use temper_agent_model_tools_tests::translate as io;
use temper_checkout_fake::{self as fake, Checkout};
use temper_fake_worker_model as worker;
use temper_lib::{Duration, Env, Queue, ReplyTo, Rng, Time, Token};
use temper_llm_model as provider;
use temper_world::{Key, Ledger, Schedule, Span, Stage, Trace};

use crate::fixture;
use crate::script::{self, Job};
use crate::translate;

/// Room in each model's output queue beyond what one step may emit. Small, so
/// the loop's flow control (take an event only while there is room for what
/// it may produce) is exercised.
const SLACK: u32 = 3;

/// The most a charter may ask for in the calm world, which every session may
/// take.
pub const BUDGET: run::Budget = run::Budget {
    turns: 64,
    input: 1 << 24,
    output: 1 << 24,
    cache_read: 1 << 26,
    cache_write: 1 << 24,
    time: Duration::from_secs(3600),
};

const CEILING: session::Budget = session::Budget {
    turns: BUDGET.turns,
    input: BUDGET.input,
    output: BUDGET.output,
    cache_read: BUDGET.cache_read,
    cache_write: BUDGET.cache_write,
    time: BUDGET.time,
};

/// The agent's limits in the calm world: room for a few runs, each with a
/// few conversations, sub-agents nested two deep beneath main.
pub const LIMITS: Limits = Limits {
    run: run::Limits {
        runs: 4,
        conversations: 16,
        run_bytes: 1 << 16,
        repositories: 2,
        outlets: 2,
        verdicts: 2,
        calls: 16,
        budget: BUDGET,
        max_tokens: 4096,
        models: 2,
        depth: 2,
        run_conversations: 6,
        answer_bytes: 1024,
        nudges: 1,
        guide_bytes: 1024,
        io_timeout: Duration::from_secs(5),
        outcome_bytes: 4096,
        check_timeout: Duration::from_secs(60),
        check_tail: 512,
        facts: 1024,
    },
    session: session::Limits {
        sessions: 16,
        messages: 64,
        session_bytes: 1 << 20,
        budget: CEILING,
        max_tokens: 4096,
        retries: 3,
        backoff_base: Duration::from_millis(200),
        backoff_max: Duration::from_secs(5),
        call_timeout: Duration::from_secs(60),
        tool_timeout: Duration::from_secs(60),
        facts: 1024,
        parallel_tools: 4,
        tools: tools::Limits {
            kits: 16,
            calls: 4,
            repos: 2,
            path_bytes: 256,
            known_files: 16,
            file_bytes: 1 << 16,
            read_bytes: 4096,
            list_entries: 64,
            match_lines: 8,
            file_timeout: Duration::from_secs(30),
            env_bytes: 256,
            shell_timeout: Duration::from_secs(30),
            shell_timeout_max: Duration::from_secs(120),
            shell_head: 256,
            shell_tail: 256,
            search_hits: 16,
            search_bytes: 1024,
            search_timeout: Duration::from_secs(30),
            facts: 1024,
        },
    },
};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Settings {
    /// Seeds the world, which seeds the models.
    pub seed: u64,
    pub limits: Limits,
    pub provider: provider::Config,
    pub worker: worker::Config,
    /// The jobs the runs do, one drawn for each.
    pub jobs: &'static [Job],
    /// One-way latency between the agent and the provider, and between the
    /// agent and the worker.
    pub network: Span,
    /// How long io takes over an operation of the tools, a command's
    /// included, and over a look of a run's in its checkout.
    pub tool: Span,
    pub look: Span,
    /// The chance, per mille, that io fails an operation of the tools, or a
    /// look of a run's.
    pub io_errors: u32,
    /// How long a run's checks take.
    pub check: Span,
    /// The chance, per mille, that a cancel loses its race: the call, the
    /// operation or the checks it was for end of themselves, and that is their
    /// terminal event. A cancel that wins is told after a network draw.
    pub cancels_lost: u32,
    /// The grid every delivery is rounded up to, so that some come at the
    /// same instant; zero for none.
    pub granule: Duration,
}

impl Settings {
    /// A world where nothing goes wrong: one coding run, on a charter that
    /// grants everything and wants a change that passes its checks, answered
    /// well within every deadline.
    #[must_use]
    pub const fn calm(seed: u64) -> Settings {
        Settings {
            seed,
            limits: LIMITS,
            provider: provider::Config {
                calls: 64,
                latency_min: Duration::from_millis(100),
                latency_max: Duration::from_millis(1000),
                overloaded: 0,
                rate_limited: 0,
                retry_after: Duration::from_secs(1),
                unavailable: 0,
                too_long: 0,
                unauthorized: 0,
                refused: 0,
                no_calls: 0,
                answer_tokens: 20,
                calls_per_answer: 2,
                malformed: 0,
                tool_rounds: 2,
            },
            worker: worker::Config {
                jobs: 1,
                window: Duration::from_millis(1),
                cancels: 0,
                cancel_min: Duration::from_secs(1),
                cancel_max: Duration::from_secs(60),
                recancels: 0,
                late_cancels: 0,
                brief_min: 64,
                brief_max: 64,
                turns_min: 64,
                turns_max: 64,
                tokens_min: 1 << 20,
                tokens_max: 1 << 20,
                time_min: Duration::from_secs(3600),
                time_max: Duration::from_secs(3600),
                max_tokens: 1024,
                writable: 1000,
                changes: 1000,
                checks: 1000,
                verdicts: 0,
                agents: 1000,
                push_min: Duration::from_millis(100),
                push_max: Duration::from_millis(500),
                moved: 0,
                push_failures: 0,
            },
            jobs: &[Job::Coding],
            network: Span::millis(1, 20),
            tool: Span::millis(10, 200),
            look: Span::millis(1, 50),
            io_errors: 0,
            check: Span::millis(500, 2000),
            cancels_lost: 0,
            granule: Duration::ZERO,
        }
    }
}

impl Settings {
    /// These settings, with the first seed from theirs whose first run the
    /// worker starts on a charter that is `wanted`: the fake draws its
    /// charters, and a scenario picks one it can tell a story about.
    #[must_use]
    pub fn drawing(self, wanted: impl Fn(&worker::api::Charter) -> bool) -> Settings {
        let seeds = self.seed..self.seed + 10_000;
        let mut found = seeds.map(|seed| Settings { seed, ..self }).filter(|settings| wanted(&first_charter(settings)));
        found.next().expect("a seed draws such a charter")
    }
}

/// The seeds the world draws for the agent, the provider and the worker from
/// its own.
fn seeds(seed: u64) -> [u64; 3] {
    let mut rng = Rng::new(seed);
    [rng.next_u64(), rng.next_u64(), rng.next_u64()]
}

/// The charter of the first run the worker of `settings` starts.
fn first_charter(settings: &Settings) -> worker::api::Charter {
    let [_, _, seed] = seeds(settings.seed);
    let mut model = worker::Model::new(&settings.worker, seed);
    let env = Env { now: Time::ZERO.saturating_add(settings.worker.window), limits: settings.worker };
    let mut out = Queue::with_capacity(worker::MAX_OUT);
    worker::fire(&mut model, &env, &mut out);
    let Some(worker::Request::Start { charter, .. }) = out.pop() else {
        panic!("the worker starts a run once its window has passed");
    };
    charter
}

/// What the world counted, as it crossed the boundary.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct Stats {
    /// Runs the worker started, those admitted, and the answers.
    pub starts: u32,
    pub admitted: u32,
    pub answers: u32,
    /// Calls the agent made of the provider; those that reached it; their
    /// terminals: answers, failures (a deadline included), cancels; and
    /// answers that came after their call had ended, and were dropped.
    pub calls: u32,
    pub provider_calls: u32,
    pub completed: u32,
    pub failed: u32,
    pub cancelled: u32,
    pub timeouts: u32,
    pub late_answers: u32,
    /// Cancels of calls, operations and checks that lost their race, and those
    /// that came once what they were for had ended in the iteration they were
    /// sent.
    pub cancels_lost: u32,
    pub cancels_crossed: u32,
    /// Operations the tools asked of io; those a cancel ended; those that ran
    /// out of time; those io failed.
    pub ops: u32,
    pub op_cancels: u32,
    pub op_timeouts: u32,
    pub op_faults: u32,
    /// The runs' looks in their checkouts: reads and probes.
    pub reads: u32,
    pub probes: u32,
    /// Checks run; those that passed, failed, ran out of time, were aborted.
    pub checks: u32,
    pub checks_passed: u32,
    pub checks_failed: u32,
    pub check_timeouts: u32,
    pub aborts: u32,
    /// Notices that checks are running.
    pub checking: u32,
    /// Pushes asked for, how they went, and those abandoned.
    pub pushes: u32,
    pub pushed: u32,
    pub moved: u32,
    pub unpushed: u32,
    pub host_cancels: u32,
    pub pushes_cancelled: u32,
    /// Cancels the worker sent.
    pub worker_cancels: u32,
}

/// The facts the agent told, by kind, as the loop drained them.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct Told {
    pub admitted: u32,
    pub prepared: u32,
    /// Conversations the runs opened, the deepest of them, and their ends.
    pub opened: u32,
    pub deepest: u32,
    pub ended: u32,
    /// Calls the LLMs made of the runs: finishes and sub-agents; and what
    /// came of them.
    pub finishes: u32,
    pub sub_agents: u32,
    pub returned: u32,
    pub accepted: u32,
    pub rejected: u32,
    pub checks_failed: u32,
    pub answered: u32,
    pub refused: u32,
    pub returns_cancelled: u32,
    pub checks_started: u32,
    pub checks_finished: u32,
    pub pushed: u32,
    pub runs_answered: u32,
    /// The sessions: opened, their completions started and how each ended,
    /// the usage told, calls delegated to the run, yields and ends.
    pub sessions: u32,
    pub completions_started: u32,
    pub completions_answered: u32,
    pub completions_failed: u32,
    pub completions_cancelled: u32,
    pub used: u32,
    pub delegated: u32,
    pub yielded: u32,
    pub sessions_ended: u32,
    /// What the sessions' tools told: calls started and answered.
    pub tools_started: u32,
    pub tools_answered: u32,
}

/// A run, as the worker's side of the world saw it, by the worker's name for
/// it.
#[derive(Debug)]
pub struct Run {
    pub job: Job,
    /// What its charter allowed, and what it may spend.
    pub allowed: Allowed,
    pub budget: run::Budget,
    /// The agent's name for it, once admitted.
    pub run: Option<Token>,
    /// The worker cancelled it.
    pub cancelled: bool,
    /// What came of its checks, and its pushes, in order.
    pub checked: Vec<bool>,
    pub pushes: Vec<run::Push>,
    /// Its answer.
    pub answer: Option<run::Answer>,
    /// What its conversations used, by the facts, and where it stood when it
    /// first spent past its budget: how many of its conversations were live
    /// then, and how many completions they used after.
    pub used: Spend,
    pub crossed: Option<(u32, u32)>,
    /// io's name for its first repository, if it has one.
    root: Option<u64>,
}

/// What a run may finish with: a change, which must pass the checks its
/// first repository has if `checks`; a verdict.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Allowed {
    pub change: bool,
    pub checks: bool,
    pub verdicts: bool,
}

/// Something on its way, delivered at its time.
enum Delivery {
    /// An event for the agent.
    Agent(Event),
    /// An event for the worker.
    Worker(worker::Event),
    /// A call arrives at the provider.
    Query { call: u64, query: provider::api::Query },
    /// The provider's answer arrives back at the agent's side.
    Answer { call: u64, result: Result<provider::api::Answer, provider::api::Error> },
    /// The agent's side gives up on a call.
    Deadline { call: u64 },
    /// An operation of the tools ends.
    Ran { owner: Token },
    /// A look of a run's in its checkout ends so.
    Looked { owner: Token, event: Event },
    /// A run's checks end.
    Checked { owner: Token },
}

/// A call of the agent in flight, as its protocol layer would keep it.
struct Call {
    owner: Token,
    /// The deadline's delivery, withdrawn when the call ends first.
    deadline: Key,
}

/// An operation of the tools in flight, as io keeps it.
struct Pending {
    /// Its end's delivery, moved up when a cancel wins the race.
    delivery: Key,
    work: Work,
}

/// What an operation does when it ends.
enum Work {
    /// A file operation, run on the checkout then.
    File(Op),
    /// A command started, finished then.
    Command(io::Started),
    /// Nothing more: it ends so (it failed to start, timed out, or was
    /// cancelled).
    Ending(Done),
}

/// A run's checks in flight, as io keeps them.
struct Checking {
    delivery: Key,
    work: Checks,
}

enum Checks {
    /// Running in the repository at `root`, keeping the last `tail` bytes of
    /// what they write.
    Running { root: u64, tail: u32 },
    /// Ending so: they ran out of time, or were aborted.
    Ending(Event),
}

pub struct World {
    now: Time,
    rng: Rng,
    settings: Settings,

    agent: agent::Model,
    agent_stage: Stage<Limits, Event, Request>,
    provider: provider::Model,
    provider_stage: Stage<provider::Config, provider::Event, provider::Request>,
    worker: worker::Model,
    worker_stage: Stage<worker::Config, worker::Event, worker::Request>,

    /// Deliveries in flight, whose count names calls too.
    wire: Schedule<Delivery>,
    /// The runs, by the worker's name for each, and the worker's name for each
    /// by the agent's.
    runs: BTreeMap<Token, Run>,
    workers: BTreeMap<Token, Token>,
    /// Starts not answered yet.
    starts: Ledger<Token, ()>,
    /// The agent's calls of the provider in flight, the call each session has
    /// in flight, the sessions whose cancel lost its race, and the calls the
    /// provider has not answered yet.
    calls: Ledger<u64, Call>,
    calling: BTreeMap<Token, u64>,
    cancel_lost: BTreeSet<Token>,
    serving: Ledger<u64, ()>,
    /// The checkout, a directory for each repository of each run, and the
    /// worker's name for the run of each root.
    checkout: Checkout,
    roots: BTreeMap<u64, Token>,
    /// The tools' operations in flight, every token one has had, and those
    /// whose cancel lost its race.
    ops: Ledger<Token, Pending>,
    owners: BTreeSet<Token>,
    op_cancel_lost: BTreeSet<Token>,
    /// The runs' looks in flight, their checks, those whose abort lost its
    /// race, and the finishing calls whose checks passed last.
    looks: Ledger<Token, ()>,
    checks: Ledger<Token, Checking>,
    abort_lost: BTreeSet<Token>,
    passed: BTreeSet<Token>,
    /// Pushes in flight, and the worker's name for the run of each.
    pushes: Ledger<Token, Token>,

    /// The run of each conversation, and those that live, by the facts.
    conversations: BTreeMap<Token, Token>,
    live: BTreeSet<Token>,

    stats: Stats,
    told: Told,
    trace: Trace,
}

impl World {
    #[must_use]
    pub fn new(settings: Settings) -> World {
        assert!(agent::worst_case(&settings.limits).is_some(), "the shell refuses limits it cannot provision");
        let [agent_seed, provider_seed, worker_seed] = seeds(settings.seed);
        let agent = agent::Model::new(&settings.limits, agent_seed);
        let provider = provider::Model::scripted(&settings.provider, provider_seed, script::all());
        let worker = worker::Model::new(&settings.worker, worker_seed);
        let max_out = agent::max_out(&settings.limits);
        let mut checkout = Checkout::new();
        fixture::script(&mut checkout);
        World {
            now: Time::ZERO,
            rng: Rng::new(settings.seed.rotate_left(32)),
            settings,
            agent,
            agent_stage: Stage::new(settings.limits, max_out, max_out + SLACK),
            provider,
            provider_stage: Stage::new(settings.provider, provider::MAX_OUT, provider::MAX_OUT + SLACK),
            worker,
            worker_stage: Stage::new(settings.worker, worker::MAX_OUT, worker::MAX_OUT + SLACK),
            wire: Schedule::new(),
            runs: BTreeMap::new(),
            workers: BTreeMap::new(),
            starts: Ledger::new("start"),
            calls: Ledger::new("call to the provider"),
            calling: BTreeMap::new(),
            cancel_lost: BTreeSet::new(),
            serving: Ledger::new("call the provider serves"),
            checkout,
            roots: BTreeMap::new(),
            ops: Ledger::new("operation of the tools"),
            owners: BTreeSet::new(),
            op_cancel_lost: BTreeSet::new(),
            looks: Ledger::new("look in a checkout"),
            checks: Ledger::new("run of the checks"),
            abort_lost: BTreeSet::new(),
            passed: BTreeSet::new(),
            pushes: Ledger::new("push"),
            conversations: BTreeMap::new(),
            live: BTreeSet::new(),
            stats: Stats::default(),
            told: Told::default(),
            trace: Trace::default(),
        }
    }

    #[must_use]
    pub fn now(&self) -> Time {
        self.now
    }

    #[must_use]
    pub fn stats(&self) -> Stats {
        self.stats
    }

    /// The facts the agent told, and how many it dropped for want of room.
    #[must_use]
    pub fn told(&self) -> (Told, u64) {
        (self.told, self.agent.facts_lost())
    }

    /// What crossed between the models and the world, in order, with times.
    #[must_use]
    pub fn trace(&self) -> &[String] {
        self.trace.lines()
    }

    /// The runs the worker started, in the order it named them.
    pub fn runs(&self) -> impl Iterator<Item = &Run> {
        self.runs.values()
    }

    /// The content of `path` in the first repository of `run`, as it was
    /// left.
    #[must_use]
    pub fn file(&self, run: &Run, path: &[u8]) -> Option<Vec<u8>> {
        self.checkout.load(run.root?, path, u64::MAX).ok().map(|(content, _)| content)
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
        self.agent_stage.tick(self.now);
        self.provider_stage.tick(self.now);
        self.worker_stage.tick(self.now);
        self.deliver();

        // Each stage resumes what is ready, then takes its events, then fires
        // its alarms, while it has room for what one more may produce.
        while self.agent_stage.has_room() && self.agent.is_ready() {
            self.log("agent ready");
            agent::resume(&mut self.agent, &self.agent_stage.env, &mut self.agent_stage.out);
        }
        while let Some(event) = self.agent_stage.next_event() {
            self.log(&format!("agent <- {}", describe_event(&event)));
            agent::step(&mut self.agent, &self.agent_stage.env, event, &mut self.agent_stage.out);
        }
        while self.agent_stage.has_room() && self.agent.is_due(self.now) {
            self.log("agent alarm");
            agent::fire(&mut self.agent, &self.agent_stage.env, &mut self.agent_stage.out);
        }
        // The facts, drained as the shell would write them out.
        while let Some(fact) = self.agent.pop_fact() {
            self.tell(fact);
        }
        while let Some(event) = self.provider_stage.next_event() {
            provider::step(&mut self.provider, &self.provider_stage.env, event, &mut self.provider_stage.out);
        }
        while self.provider_stage.has_room() && self.provider.is_due(self.now) {
            provider::fire(&mut self.provider, &self.provider_stage.env, &mut self.provider_stage.out);
        }
        while let Some(event) = self.worker_stage.next_event() {
            worker::step(&mut self.worker, &self.worker_stage.env, event, &mut self.worker_stage.out);
        }
        while self.worker_stage.has_room() && self.worker.is_due(self.now) {
            worker::fire(&mut self.worker, &self.worker_stage.env, &mut self.worker_stage.out);
        }

        // What the steps asked for, submitted at the end of the iteration.
        while let Some(request) = self.agent_stage.out.pop() {
            self.agent_request(request);
        }
        while let Some(request) = self.provider_stage.out.pop() {
            self.provider_request(request);
        }
        while let Some(request) = self.worker_stage.out.pop() {
            self.worker_request(request);
        }

        // The reclaim point.
        self.agent.reclaim();
        self.provider.reclaim();
        self.worker.reclaim();
        self.assert_bounded();
    }

    /// What holds after every reclaim point: every slab within its limits.
    fn assert_bounded(&self) {
        let limits = &self.settings.limits;
        let run = self.agent.run();
        let sessions = self.agent.session();
        assert!(run.runs() <= limits.run.runs, "runs stay within their slots");
        assert!(run.conversations() <= limits.run.conversations, "conversations stay within their slots");
        assert!(run.calls() <= limits.run.calls, "the run's calls stay within their slots");
        assert!(sessions.sessions() <= limits.session.sessions, "sessions stay within their slots");
        assert!(sessions.kits() <= limits.session.tools.kits, "a kit at most for each session");
        assert!(self.agent.peers() <= limits.run.conversations, "a peer at most for each conversation");
        let flights = limits.run.conversations * limits.session.parallel_tools;
        assert!(self.agent.flights() <= flights, "delegated calls stay within a batch of each session");
        assert!(self.agent.flights() <= sessions.runs(), "the top level holds a flight only for a call in flight");
        assert!(self.provider.calls() <= self.settings.provider.calls, "the provider's calls stay within its slots");
    }

    /// The agent's requests, carried out the way its neighbours and the layers
    /// below would.
    fn agent_request(&mut self, request: Request) {
        self.log(&format!("agent -> {}", describe_request(&request)));
        match request {
            Request::Admitted { worker, run } => {
                let state = self.runs.get_mut(&worker).expect("the agent admits runs the worker started");
                assert!(state.run.is_none() && state.answer.is_none(), "a run is admitted once, before its answer");
                state.run = Some(run);
                assert!(self.workers.insert(run, worker).is_none(), "the agent names its runs apart");
                self.stats.admitted += 1;
                self.send(Delivery::Worker(worker::Event::Admitted { owner: worker, run }));
            }
            Request::Answer { to, answer } => {
                let worker = to.into_token();
                self.starts.end(worker);
                self.answered(worker, &answer);
                let answered = translate::answer(&answer);
                self.runs.get_mut(&worker).expect("checked above").answer = Some(answer);
                self.stats.answers += 1;
                self.send(Delivery::Worker(worker::Event::Answered { owner: worker, answer: answered }));
            }
            Request::Checking { worker, deadline } => {
                assert!(self.starts.contains(worker), "checks run for a run that has not answered");
                self.stats.checking += 1;
                self.send(Delivery::Worker(worker::Event::Checking { job: worker, deadline }));
            }
            Request::Push { worker, owner, change } => {
                let state = self.runs.get(&worker).expect("a run pushes once the worker started it");
                assert!(state.answer.is_none(), "a run pushes before it answers");
                assert!(state.allowed.change, "a run pushes only a change its charter allows");
                assert_eq!(&*change.title, script::TITLE, "the change pushed is the one the LLM declared");
                if state.allowed.checks {
                    assert!(self.passed.contains(&owner), "a change is pushed only once its checks passed");
                }
                self.pushes.open(owner, worker);
                self.stats.pushes += 1;
                let change = translate::change(change);
                let push = worker::Event::Push { reply_to: ReplyTo::new(owner), owner, job: worker, change };
                self.send(Delivery::Worker(push));
            }
            Request::CancelHost { owner } => {
                self.stats.host_cancels += 1;
                self.send(Delivery::Worker(worker::Event::CancelPush { owner }));
            }
            Request::Complete { owner, prompt, timeout } => {
                let call = self.wire.name();
                let deadline = self.schedule(self.now.saturating_add(timeout), Delivery::Deadline { call });
                self.calls.open(call, Call { owner, deadline });
                assert!(self.calling.insert(owner, call).is_none(), "a session has one call in flight");
                let query = translate::query(prompt);
                self.send(Delivery::Query { call, query });
                self.stats.calls += 1;
            }
            Request::Cancel { owner } => {
                // A call that has already ended has its terminal event on the
                // way: the cancel lost the race and changes nothing. One still
                // in flight may end of itself all the same.
                let Some(&call) = self.calling.get(&owner) else {
                    self.stats.cancels_crossed += 1;
                    return;
                };
                if self.rng.chance(self.settings.cancels_lost) {
                    self.cancel_lost.insert(owner);
                    self.stats.cancels_lost += 1;
                } else {
                    self.end_call(call);
                    self.stats.cancelled += 1;
                    self.send(Delivery::Agent(Event::Cancelled { owner }));
                }
            }
            Request::Io { owner, op, deadline } => self.start_op(owner, op, deadline),
            Request::CancelIo { owner } => self.cancel_op(owner),
            Request::Read { owner, at, max, deadline } => self.read(owner, &at, max, deadline),
            Request::Probe { owner, at, deadline } => self.probe(owner, &at, deadline),
            Request::Check { owner, program, deadline, tail } => self.check(owner, &program, deadline, tail),
            Request::Abort { owner } => self.abort(owner),
        }
    }

    /// Checks the answer to the start of the worker's `worker` against what
    /// happened to its run.
    fn answered(&mut self, worker: Token, answer: &run::Answer) {
        let state = self.runs.get(&worker).expect("the agent answers starts the worker made");
        assert!(state.answer.is_none(), "one answer per start");
        assert!(self.pushes.values().all(|of| *of != worker), "a run answers once its pushes have ended");
        let spent = match answer {
            run::Answer::Refused(_) => {
                assert!(state.used == Spend::ZERO, "a refused run spends nothing");
                return;
            }
            run::Answer::Accepted { outcome: Declared::Change(change), spent } => {
                assert!(state.allowed.change, "a change is accepted only if the charter allows one");
                assert_eq!(state.pushes.last(), Some(&run::Push::Done), "a change is accepted once it is pushed");
                assert_eq!(&*change.title, script::TITLE, "the change accepted is the one the LLM declared");
                spent
            }
            run::Answer::Accepted { outcome: Declared::Verdict(_), spent } => {
                assert!(state.allowed.verdicts, "a verdict is accepted only if the charter allows one");
                assert!(state.pushes.is_empty(), "a verdict pushes nothing");
                spent
            }
            run::Answer::Failed { failure, spent } => {
                match failure {
                    run::Failure::Stale => {
                        assert!(state.pushes.contains(&run::Push::Moved), "a run is stale once a push found it so");
                    }
                    run::Failure::Cancelled => assert!(state.cancelled, "a run is cancelled only by the worker"),
                    run::Failure::Model(_) | run::Failure::Budget(_) | run::Failure::Policy(_) => {}
                }
                spent
            }
        };
        if self.agent.facts_lost() == 0 {
            assert_eq!(*spent, state.used, "a run's answer adds up what its conversations used");
            if let Some((live, after)) = state.crossed {
                assert!(after <= live + 1, "past its budget, a run's conversations finish what they had started");
            }
        }
    }

    /// io reads the file at `at` for a run, its first `max` bytes as text.
    fn read(&mut self, owner: Token, at: &run::Place, max: u32, deadline: Time) {
        let (ends, found) = self.look(at, deadline);
        let read = match found {
            Some(Ok(content)) => text(&content, max),
            Some(Err(fake::Failure::Missing | fake::Failure::NotFile | fake::Failure::NotDirectory)) => {
                run::Read::Missing
            }
            Some(Err(_)) | None => run::Read::Failed,
        };
        self.stats.reads += 1;
        self.looked(owner, ends, Event::Read { owner, read });
    }

    /// io finds out for a run whether an executable is at `at`.
    fn probe(&mut self, owner: Token, at: &run::Place, deadline: Time) {
        let (ends, found) = self.look(at, deadline);
        let executable = match found {
            Some(Ok(content)) => fixture::executable(&content),
            Some(Err(_)) | None => false,
        };
        self.stats.probes += 1;
        self.looked(owner, ends, Event::Probed { owner, executable });
    }

    /// io runs a run's checks, `program`, to end by `deadline`: after a draw,
    /// on the checkout as it is then, or stopped at the deadline.
    fn check(&mut self, owner: Token, program: &run::Place, deadline: Time, tail: u32) {
        let root = program.root.raw();
        let worker = *self.roots.get(&root).expect("checks run in a run's checkout");
        assert!(self.starts.contains(worker), "checks run for a run that has not answered");
        assert_eq!(&*program.path, fixture::CHECKS, "the run runs the checks it probed for");
        self.passed.remove(&owner);
        let ends = self.now.saturating_add(self.draw(self.settings.check));
        let (at, work) = if ends > deadline {
            let ran = run::Ran { exit: run::Exit::TimedOut, output: Box::new([]), cut: 0 };
            let noticed = deadline.saturating_add(self.draw(self.settings.network));
            self.stats.check_timeouts += 1;
            (noticed, Checks::Ending(Event::Checked { owner, ran }))
        } else {
            (ends, Checks::Running { root, tail })
        };
        let delivery = self.schedule(at, Delivery::Checked { owner });
        self.checks.open(owner, Checking { delivery, work });
        self.stats.checks += 1;
    }

    /// io is asked to stop the checks of `owner`: they end aborted after a
    /// network draw, unless they end of themselves first.
    fn abort(&mut self, owner: Token) {
        let Some(checking) = self.checks.get(owner) else {
            self.stats.cancels_crossed += 1;
            return;
        };
        if matches!(checking.work, Checks::Ending(_)) || self.rng.chance(self.settings.cancels_lost) {
            self.abort_lost.insert(owner);
            self.stats.cancels_lost += 1;
            return;
        }
        let at = self.now.saturating_add(self.draw(self.settings.network));
        let delivery = self.schedule(at, Delivery::Checked { owner });
        let checking = self.checks.get_mut(owner).expect("looked up above");
        self.wire.withdraw(checking.delivery).expect("checks in flight have their end on the way");
        (checking.delivery, checking.work) = (delivery, Checks::Ending(Event::Aborted { owner }));
        self.stats.aborts += 1;
    }

    /// io looks at `at` for a run, by `deadline`: when it ends, and the
    /// content of the file there or why there is none; or nothing, if io
    /// failed, or the deadline passed first (which io says a moment after).
    fn look(&mut self, at: &run::Place, deadline: Time) -> (Time, Option<Result<Vec<u8>, fake::Failure>>) {
        let root = at.root.raw();
        let worker = *self.roots.get(&root).expect("a run looks in its own checkout");
        assert!(self.starts.contains(worker), "a run looks in its checkout before it answers");
        let ends = self.now.saturating_add(self.draw(self.settings.look));
        if ends > deadline {
            return (deadline.saturating_add(self.draw(self.settings.network)), None);
        }
        if self.rng.chance(self.settings.io_errors) {
            return (ends, None);
        }
        (ends, Some(self.checkout.load(root, &at.path, u64::MAX).map(|(content, _)| content)))
    }

    /// The look of `owner` ends with `event` at `at`.
    fn looked(&mut self, owner: Token, at: Time, event: Event) {
        self.looks.open(owner, ());
        self.schedule(at, Delivery::Looked { owner, event });
    }

    /// io starts the tools' operation `op` for `owner`, to end by
    /// `deadline`: after a draw, or at the deadline if that comes first.
    fn start_op(&mut self, owner: Token, op: Op, deadline: Time) {
        // A call's operations, one after another, carry its token.
        self.owners.insert(owner);
        let at = match &op {
            Op::Load { at, .. } | Op::Scan { at, .. } | Op::Store { at, .. } | Op::Search { at, .. } => at.root,
            Op::Spawn { cwd, .. } => cwd.root,
        };
        let worker = *self.roots.get(&at.raw()).expect("an operation is in a run's checkout");
        assert!(self.starts.contains(worker), "the tools work in a run's checkout before it answers");
        let mut ends = self.now.saturating_add(self.draw(self.settings.tool));
        let mut work = if self.rng.chance(self.settings.io_errors) {
            self.stats.op_faults += 1;
            Work::Ending(Done::Failed { fault: Fault::Other })
        } else {
            match op {
                Op::Spawn { cwd, command, env, roots, head, tail } => {
                    match io::spawn(&self.checkout, &cwd, &command, &env, &roots, (head, tail)) {
                        Ok(started) => {
                            let runs = Duration::from_nanos(
                                u64::try_from(started.process.program.duration.as_nanos()).unwrap_or(u64::MAX),
                            );
                            ends = ends.saturating_add(runs);
                            Work::Command(started)
                        }
                        Err(done) => Work::Ending(done),
                    }
                }
                op @ (Op::Load { .. } | Op::Scan { .. } | Op::Store { .. } | Op::Search { .. }) => Work::File(op),
            }
        };
        // io runs the race with the deadline, and says it lost a moment after
        // the deadline passes: a command killed then tells what it wrote by
        // then (nothing, here).
        if ends > deadline {
            let done = match work {
                Work::Command(started) => io::exited(None, &[], started.head, started.tail),
                Work::File(_) | Work::Ending(_) => Done::TimedOut,
            };
            let noticed = deadline.saturating_add(self.draw(self.settings.network));
            (work, ends) = (Work::Ending(done), noticed);
            self.stats.op_timeouts += 1;
        }
        let delivery = self.schedule(ends, Delivery::Ran { owner });
        self.ops.open(owner, Pending { delivery, work });
        self.stats.ops += 1;
    }

    /// io is asked to cancel the operation of `owner`: it ends cancelled
    /// after a network draw, unless it ends of itself first.
    fn cancel_op(&mut self, owner: Token) {
        assert!(self.owners.contains(&owner), "a cancel names an operation io was asked for");
        if !self.ops.contains(owner) {
            // It ended in the iteration the cancel was sent.
            self.stats.cancels_crossed += 1;
            return;
        }
        if self.rng.chance(self.settings.cancels_lost) {
            self.op_cancel_lost.insert(owner);
            self.stats.cancels_lost += 1;
            return;
        }
        let at = self.now.saturating_add(self.draw(self.settings.network));
        let delivery = self.schedule(at, Delivery::Ran { owner });
        let pending = self.ops.get_mut(owner).expect("looked up above");
        self.wire.withdraw(pending.delivery).expect("an operation in flight has its end on the way");
        (pending.delivery, pending.work) = (delivery, Work::Ending(Done::Cancelled));
        self.stats.op_cancels += 1;
    }

    /// The operation of `owner` ends, and io tells the tools how.
    fn ran(&mut self, owner: Token) {
        let pending = self.ops.end(owner);
        let done = match pending.work {
            Work::File(op) => io::perform(&mut self.checkout, op),
            Work::Command(started) => {
                self.checkout.finish(&started.process);
                let program = &started.process.program;
                io::exited(Some(program.exit), &program.output, started.head, started.tail)
            }
            Work::Ending(done) => done,
        };
        self.op_cancel_lost.remove(&owner);
        self.agent_stage.push(Event::Done { owner, done });
    }

    /// The checks of `owner` end, and io tells the run how.
    fn checked(&mut self, owner: Token) {
        let checking = self.checks.end(owner);
        self.abort_lost.remove(&owner);
        let event = match checking.work {
            Checks::Running { root, tail } => {
                let program = self.checkout.load(root, fixture::CHECKS, u64::MAX).expect("the checks probed for").0;
                let (passed, output) = fixture::check(&self.checkout, root, &program);
                let keep = output.len().min(usize::try_from(tail).expect("a small tail"));
                let cut = u64::try_from(output.len() - keep).expect("a small output");
                let output = output[output.len() - keep..].into();
                let exit = run::Exit::Code { code: u8::from(!passed) };
                if passed {
                    self.passed.insert(owner);
                    self.stats.checks_passed += 1;
                } else {
                    self.stats.checks_failed += 1;
                }
                let worker = *self.roots.get(&root).expect("checks run in a run's checkout");
                self.runs.get_mut(&worker).expect("a run of the worker's").checked.push(passed);
                Event::Checked { owner, ran: run::Ran { exit, output, cut } }
            }
            Checks::Ending(event) => event,
        };
        self.agent_stage.push(event);
    }

    /// The provider's requests, carried back the way its protocol layer would.
    fn provider_request(&mut self, request: provider::Request) {
        match request {
            provider::Request::Reply { to, result } => {
                let call = to.into_token().raw();
                self.serving.end(call);
                self.send(Delivery::Answer { call, result });
            }
        }
    }

    /// The worker's requests, carried the way its protocol layer, and the
    /// worker's own io preparing checkouts, would.
    fn worker_request(&mut self, request: worker::Request) {
        match request {
            worker::Request::Start { owner, charter } => self.start(owner, charter),
            worker::Request::Cancel { run } => {
                let worker = *self.workers.get(&run).expect("the worker cancels runs the agent admitted");
                self.runs.get_mut(&worker).expect("a run of the worker's").cancelled = true;
                self.stats.worker_cancels += 1;
                self.log(&format!("worker cancels {}", run.raw()));
                self.send(Delivery::Agent(Event::Cancel { run }));
            }
            worker::Request::Pushed { to, pushed } => {
                let owner = to.into_token();
                let worker = self.pushes.end(owner);
                let push = translate::push(pushed);
                match push {
                    run::Push::Done => self.stats.pushed += 1,
                    run::Push::Moved => self.stats.moved += 1,
                    run::Push::Failed => self.stats.unpushed += 1,
                }
                self.runs.get_mut(&worker).expect("a run of the worker's").pushes.push(push);
                self.send(Delivery::Agent(Event::Pushed { owner, push }));
            }
            worker::Request::PushCancelled { to } => {
                let owner = to.into_token();
                self.pushes.end(owner);
                self.stats.pushes_cancelled += 1;
                self.send(Delivery::Agent(Event::HostCancelled { owner }));
            }
        }
    }

    /// The worker starts a run on `charter` for its job `owner`: it checks out
    /// the run's repositories, each a root of its own, the first seeded for
    /// the job drawn for it.
    fn start(&mut self, owner: Token, charter: temper_fake_worker_model::api::Charter) {
        let jobs = self.settings.jobs;
        let job = jobs[usize::try_from(self.rng.below(u64::try_from(jobs.len()).expect("few"))).expect("few")];
        let cue = script::cue(job);
        let mut roots: Vec<Token> = Vec::new();
        for (index, repository) in charter.repositories.iter().enumerate() {
            let root = fixture::seed(&mut self.checkout, owner.raw(), &repository.name, index == 0, cue);
            self.roots.insert(root, owner);
            roots.push(io::token(root));
        }
        let outcome = &charter.outcome;
        let first = charter.repositories.first();
        let allowed = Allowed {
            change: outcome.change,
            checks: outcome.change && outcome.checks && first.is_some_and(|first| first.writable),
            verdicts: !outcome.verdicts.is_empty(),
        };
        let state = Run {
            job,
            allowed,
            budget: translate::budget(charter.budget),
            run: None,
            cancelled: false,
            checked: Vec::new(),
            pushes: Vec::new(),
            answer: None,
            used: Spend::ZERO,
            crossed: None,
            root: roots.first().map(|root| root.raw()),
        };
        assert!(self.runs.insert(owner, state).is_none(), "the worker names its jobs apart");
        self.starts.open(owner, ());
        self.stats.starts += 1;
        self.log(&format!("worker starts {} as {job:?}", owner.raw()));
        let charter = translate::charter(charter, &roots);
        self.send(Delivery::Agent(Event::Start { reply_to: ReplyTo::new(owner), worker: owner, charter }));
    }

    /// Hands every delivery that is due to its destination.
    fn deliver(&mut self) {
        while let Some(delivery) = self.wire.next(self.now) {
            match delivery {
                Delivery::Agent(event) => self.agent_stage.push(event),
                Delivery::Worker(event) => self.worker_stage.push(event),
                Delivery::Query { call, query } => {
                    self.serving.open(call, ());
                    let reply_to = ReplyTo::new(Token::new(call));
                    self.provider_stage.push(provider::Event::Call { reply_to, query });
                    self.stats.provider_calls += 1;
                }
                Delivery::Answer { call, result } => {
                    // The fake refuses a transcript a real provider would: a
                    // call without its result, a result without its call.
                    assert!(result != Err(provider::api::Error::InvalidRequest), "the agent sends well-formed queries");
                    let Some(Call { owner, .. }) = self.end_call(call) else {
                        self.stats.late_answers += 1;
                        continue;
                    };
                    self.cancel_lost.remove(&owner);
                    let event = match result {
                        Ok(answer) => {
                            self.stats.completed += 1;
                            Event::Completed { owner, completion: translate::completion(answer) }
                        }
                        Err(error) => {
                            self.stats.failed += 1;
                            Event::Failed { owner, failure: translate::failure(error) }
                        }
                    };
                    self.agent_stage.push(event);
                }
                Delivery::Deadline { call } => {
                    let Call { owner, .. } =
                        self.end_call(call).expect("a deadline is withdrawn when its call ends first");
                    self.cancel_lost.remove(&owner);
                    self.agent_stage.push(Event::Failed { owner, failure: agent::llm::Failure::TimedOut });
                    self.stats.failed += 1;
                    self.stats.timeouts += 1;
                }
                Delivery::Ran { owner } => self.ran(owner),
                Delivery::Looked { owner, event } => {
                    self.looks.end(owner);
                    self.agent_stage.push(event);
                }
                Delivery::Checked { owner } => self.checked(owner),
            }
        }
    }

    /// Ends the agent's call `call` if it is still in flight, withdrawing its
    /// deadline, and returns it.
    fn end_call(&mut self, call: u64) -> Option<Call> {
        let ended = self.calls.take(call)?;
        self.calling.remove(&ended.owner);
        self.wire.withdraw(ended.deadline);
        Some(ended)
    }

    /// Counts a fact, and follows the runs' conversations and what they use.
    fn tell(&mut self, fact: Fact) {
        let told = &mut self.told;
        match fact {
            Fact::Run { fact } => match fact {
                run_facts::Fact::Admitted { .. } => told.admitted += 1,
                run_facts::Fact::Prepared { .. } => told.prepared += 1,
                run_facts::Fact::Opened { run, conversation, depth } => {
                    told.opened += 1;
                    told.deepest = told.deepest.max(depth);
                    self.conversations.insert(conversation, run);
                    self.live.insert(conversation);
                    let worker = self.workers.get(&run).expect("a run opens conversations once admitted");
                    let state = self.runs.get(worker).expect("a run of the worker's");
                    assert!(state.crossed.is_none(), "a run past its budget opens no conversation");
                }
                run_facts::Fact::Ended { conversation, .. } => {
                    told.ended += 1;
                    assert!(self.live.remove(&conversation), "a conversation ends once, after it opened");
                }
                run_facts::Fact::Called { ask, .. } => match ask {
                    run_facts::Asked::Finish => told.finishes += 1,
                    run_facts::Asked::SubAgent => told.sub_agents += 1,
                },
                run_facts::Fact::Returned { result, .. } => {
                    told.returned += 1;
                    match result {
                        Return::Accepted => told.accepted += 1,
                        Return::Rejected => told.rejected += 1,
                        Return::ChecksFailed => told.checks_failed += 1,
                        Return::Answered => told.answered += 1,
                        Return::Refused => told.refused += 1,
                        Return::Cancelled => told.returns_cancelled += 1,
                        Return::Moved | Return::Unpushed | Return::TimedOut | Return::Busy | Return::Unanswered => {}
                    }
                }
                run_facts::Fact::CheckStarted { .. } => told.checks_started += 1,
                run_facts::Fact::CheckFinished { .. } => told.checks_finished += 1,
                run_facts::Fact::Pushed { .. } => told.pushed += 1,
                run_facts::Fact::Answered { .. } => told.runs_answered += 1,
            },
            Fact::Session { fact } => match fact {
                session::Fact::Opened { .. } => told.sessions += 1,
                session::Fact::CompletionStarted { .. } => told.completions_started += 1,
                session::Fact::CompletionAnswered { .. } => told.completions_answered += 1,
                session::Fact::CompletionFailed { .. } => told.completions_failed += 1,
                session::Fact::CompletionCancelled { .. } => told.completions_cancelled += 1,
                session::Fact::CompletionRetried { .. }
                | session::Fact::DelegateAnswered { .. }
                | session::Fact::DelegateCancelled { .. } => {}
                session::Fact::Tools { fact } => match fact {
                    tools::Fact::Started { .. } => told.tools_started += 1,
                    tools::Fact::Answered { .. } => told.tools_answered += 1,
                    tools::Fact::Opened { .. }
                    | tools::Fact::Refused { .. }
                    | tools::Fact::Closing { .. }
                    | tools::Fact::Closed { .. } => {}
                },
                session::Fact::DelegateStarted { .. } => told.delegated += 1,
                session::Fact::Yielded { .. } => told.yielded += 1,
                session::Fact::Used { opener, usage } => {
                    told.used += 1;
                    self.used(opener, usage);
                }
                session::Fact::Ended { .. } => told.sessions_ended += 1,
            },
        }
    }

    /// A completion of the conversation `conversation` used `usage`: its run
    /// adds it up, and notes when it first spends past its budget.
    fn used(&mut self, conversation: Token, usage: session::llm::Usage) {
        let run = self.conversations.get(&conversation).expect("a session's opener is a conversation of a run");
        let worker = self.workers.get(run).expect("a run of the agent's");
        let live = u32::try_from(self.live.iter().filter(|live| self.conversations.get(live) == Some(run)).count())
            .expect("a few conversations");
        let state = self.runs.get_mut(worker).expect("a run of the worker's");
        let session::llm::Usage { input_tokens, output_tokens, cache_read_tokens, cache_write_tokens } = usage;
        let spend = Spend {
            turns: 1,
            input: input_tokens,
            output: output_tokens,
            cache_read: cache_read_tokens,
            cache_write: cache_write_tokens,
        };
        state.used = state.used.saturating_add(spend);
        match &mut state.crossed {
            Some((_, after)) => *after += 1,
            None if overspent(&state.budget, state.used) => state.crossed = Some((live, 0)),
            None => {}
        }
    }

    /// What the facts must add up to when none were dropped: what the world
    /// saw cross the boundary.
    fn assert_told(&self) {
        let (told, stats) = (&self.told, &self.stats);
        assert_eq!(told.admitted, stats.admitted, "a fact for every run admitted");
        assert_eq!(told.runs_answered, stats.answers, "a fact for every answer");
        assert_eq!(told.opened, told.ended, "a conversation opened ends");
        assert_eq!(told.sessions_ended, told.opened, "each conversation's session ends, or is refused");
        assert!(told.sessions <= told.opened, "a session opens for a conversation");
        assert_eq!(told.completions_started, stats.calls, "a fact for every call");
        let ended = told.completions_answered + told.completions_failed + told.completions_cancelled;
        assert_eq!(ended, stats.completed + stats.failed + stats.cancelled, "a fact for the end of every call");
        assert_eq!(told.used, told.completions_answered, "every completion answered is used");
        assert_eq!(told.delegated, told.finishes + told.sub_agents, "every delegated call reaches the run");
        assert_eq!(told.returned, told.delegated, "every call the run took returns");
        assert_eq!(told.checks_started, stats.checks, "a fact for every run of the checks");
        assert_eq!(
            told.checks_finished,
            stats.checks_passed + stats.checks_failed + stats.check_timeouts,
            "a fact for every end of the checks"
        );
        assert_eq!(told.pushed, stats.pushed + stats.moved + stats.unpushed, "a fact for every push's end");
        assert!(told.tools_answered >= told.tools_started, "a fact for the answer to every call started");
    }

    /// The invariants of a world where nothing is left to happen.
    fn assert_settled(&self) {
        let run = self.agent.run();
        let sessions = self.agent.session();
        assert_eq!((run.runs(), run.conversations(), run.calls()), (0, 0, 0), "every run has ended, and its calls");
        assert_eq!((sessions.sessions(), sessions.runs()), (0, 0), "every session has ended, and its calls");
        assert_eq!((sessions.kits(), sessions.jobs()), (0, 0), "every kit has closed, its calls answered");
        assert_eq!((self.agent.peers(), self.agent.flights()), (0, 0), "every peer and flight is freed");
        assert_eq!(self.agent.tickets(), 0, "every ticket is freed");
        assert!(!self.agent.is_ready(), "the ready list is drained");
        assert_eq!(self.agent.next_deadline(), None, "no alarm outlives what it was for");
        assert_eq!(self.provider.calls(), 0, "the provider holds no call");
        assert_eq!((self.worker.jobs(), self.worker.pushes()), (0, 0), "the worker has every answer");
        self.starts.assert_settled();
        self.calls.assert_settled();
        self.serving.assert_settled();
        self.ops.assert_settled();
        self.looks.assert_settled();
        self.checks.assert_settled();
        self.pushes.assert_settled();
        assert!(self.calling.is_empty() && self.cancel_lost.is_empty(), "no call is in flight");
        assert!(self.op_cancel_lost.is_empty() && self.abort_lost.is_empty(), "every lost cancel's race ended");
        assert!(
            self.wire.is_empty()
                && !self.agent_stage.has_events()
                && !self.provider_stage.has_events()
                && !self.worker_stage.has_events(),
            "nothing is on its way"
        );
        assert_eq!(self.worker.answered(), self.stats.starts, "every start was answered");
        for (worker, state) in &self.runs {
            assert!(state.answer.is_some(), "the run of job {} answered", worker.raw());
        }
        if self.agent.facts_lost() == 0 {
            assert!(self.live.is_empty(), "every conversation has ended");
            self.assert_told();
        }
    }

    fn has_work_now(&self) -> bool {
        self.agent_stage.has_events()
            || self.provider_stage.has_events()
            || self.worker_stage.has_events()
            || self.agent.is_ready()
            || self.agent.is_due(self.now)
            || self.provider.is_due(self.now)
            || self.worker.is_due(self.now)
            || self.wire.is_due(self.now)
    }

    fn next_time(&self) -> Option<Time> {
        [self.wire.next_time(), self.agent.next_deadline(), self.provider.next_deadline(), self.worker.next_deadline()]
            .into_iter()
            .flatten()
            .min()
    }

    fn send(&mut self, delivery: Delivery) {
        let at = self.now.saturating_add(self.draw(self.settings.network));
        self.schedule(at, delivery);
    }

    fn schedule(&mut self, at: Time, delivery: Delivery) -> Key {
        // On a coarse grid, if the world has one, so that deliveries coincide.
        let granule = self.settings.granule.as_nanos();
        let at =
            if granule > 0 { Time::from_nanos(at.as_nanos().div_ceil(granule).saturating_mul(granule)) } else { at };
        self.wire.send(at, delivery)
    }

    fn draw(&mut self, span: Span) -> Duration {
        span.draw(&mut self.rng)
    }

    fn log(&mut self, line: &str) {
        self.trace.log(self.now, line);
    }
}

/// Whether `spent` is past `budget` in any part but time.
fn overspent(budget: &run::Budget, spent: Spend) -> bool {
    spent.turns > budget.turns
        || spent.input > budget.input
        || spent.output > budget.output
        || spent.cache_read > budget.cache_read
        || spent.cache_write > budget.cache_write
}

/// What a run's read finds in `content`: its first characters in at most
/// `max` bytes, cut where a character ends, if it is text.
fn text(content: &[u8], max: u32) -> run::Read {
    let Ok(text) = std::str::from_utf8(content) else {
        return run::Read::NotText;
    };
    let mut end = text.len().min(usize::try_from(max).expect("a small read"));
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    run::Read::Text { text: content[..end].into(), whole: end == content.len() }
}

fn describe_event(event: &Event) -> String {
    match event {
        Event::Start { worker, charter, .. } => format!("start {} {:?}", worker.raw(), charter.grants.tools),
        Event::Cancel { run } => format!("cancel {}", run.raw()),
        Event::Pushed { owner, push } => format!("pushed {} {push:?}", owner.raw()),
        Event::HostCancelled { owner } => format!("host cancelled {}", owner.raw()),
        Event::Completed { owner, completion } => {
            format!("completed {} {:?} with {} parts", owner.raw(), completion.stop, completion.content.len())
        }
        Event::Failed { owner, failure } => format!("failed {} {failure:?}", owner.raw()),
        Event::Cancelled { owner } => format!("cancelled {}", owner.raw()),
        Event::Done { owner, done } => format!("done {} {}", owner.raw(), done_kind(done)),
        Event::Read { owner, read } => match read {
            run::Read::Text { text, whole } => format!("read {} {} bytes, whole {whole}", owner.raw(), text.len()),
            run::Read::Missing | run::Read::NotText | run::Read::Failed => format!("read {} {read:?}", owner.raw()),
        },
        Event::Probed { owner, executable } => format!("probed {} {executable}", owner.raw()),
        Event::Checked { owner, ran } => format!("checked {} {:?}", owner.raw(), ran.exit),
        Event::Aborted { owner } => format!("aborted {}", owner.raw()),
    }
}

fn describe_request(request: &Request) -> String {
    match request {
        Request::Admitted { worker, run } => format!("admitted {} as {}", worker.raw(), run.raw()),
        Request::Answer { to, answer } => format!("answer {to:?} {}", answer_kind(answer)),
        Request::Checking { worker, deadline } => format!("checking {} until {}", worker.raw(), deadline.as_nanos()),
        Request::Push { worker, owner, .. } => format!("push {} for {}", owner.raw(), worker.raw()),
        Request::CancelHost { owner } => format!("cancel host {}", owner.raw()),
        Request::Complete { owner, prompt, .. } => {
            format!("complete {} with {} messages, {} served", owner.raw(), prompt.messages.len(), prompt.served.len())
        }
        Request::Cancel { owner } => format!("cancel {}", owner.raw()),
        Request::Io { owner, op, .. } => format!("io {} {}", owner.raw(), op_kind(op)),
        Request::CancelIo { owner } => format!("cancel io {}", owner.raw()),
        Request::Read { owner, at, .. } => format!("read {} {}", owner.raw(), String::from_utf8_lossy(&at.path)),
        Request::Probe { owner, at, .. } => format!("probe {} {}", owner.raw(), String::from_utf8_lossy(&at.path)),
        Request::Check { owner, .. } => format!("check {}", owner.raw()),
        Request::Abort { owner } => format!("abort {}", owner.raw()),
    }
}

fn answer_kind(answer: &run::Answer) -> String {
    match answer {
        run::Answer::Refused(refusal) => format!("refused {refusal:?}"),
        run::Answer::Accepted { outcome: Declared::Change(_), spent } => format!("accepted a change, {spent:?}"),
        run::Answer::Accepted { outcome: Declared::Verdict(verdict), spent } => {
            format!("accepted {:?}, {spent:?}", String::from_utf8_lossy(&verdict.name))
        }
        run::Answer::Failed { failure, spent } => format!("failed {failure:?}, {spent:?}"),
    }
}

fn op_kind(op: &Op) -> &'static str {
    match op {
        Op::Load { .. } => "load",
        Op::Scan { .. } => "scan",
        Op::Store { .. } => "store",
        Op::Search { .. } => "search",
        Op::Spawn { .. } => "spawn",
    }
}

fn done_kind(done: &Done) -> String {
    match done {
        Done::Loaded { content, .. } => format!("loaded {} bytes", content.len()),
        Done::Scanned { entries, .. } => format!("scanned {} entries", entries.len()),
        Done::Found { hits, .. } => format!("found {} hits", hits.len()),
        Done::Exited { exit, .. } => format!("exited {exit:?}"),
        done @ (Done::Stored { .. }
        | Done::Missing
        | Done::NotDirectory
        | Done::Escapes
        | Done::Failed { .. }
        | Done::Conflict { .. }
        | Done::NotFile
        | Done::Linked
        | Done::TooLarge { .. }
        | Done::TimedOut
        | Done::Cancelled) => format!("{done:?}"),
    }
}
