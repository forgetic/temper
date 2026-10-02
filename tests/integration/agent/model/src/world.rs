use std::collections::{BTreeMap, BTreeSet, VecDeque};

use temper_agent_model::run::outcome::Declared;
use temper_agent_model::run::{self, Spend};
use temper_agent_model::{self as agent, Event, Fact, Limits, Request, session, tools};
use temper_checkout_fake::Checkout;
use temper_checkout_fake::git::{Remote, Tree as Files};
use temper_fake_engine_model::{self as engine, Config, Origin, api};
use temper_lib::{Duration, Env, Queue, Rng, Time, Token};
use temper_llm_model as provider;
use temper_worker_model::agent::channel::{Down, Up};
use temper_worker_model::checkout::git::{Commit, Op};
use temper_worker_model::{self as worker, host};
use temper_worker_model_checkout_tests::forge::Forge;
use temper_worker_model_checkout_tests::translate as io;
use temper_world::{Key, Ledger, Referee, Schedule, Span, Stage, Trace};

use crate::channel::{self, Link};
use crate::fixture;
use crate::referee::{Meeting, Repository, Seen, Stimulus};
use crate::script::{self, Job};

mod agents;
mod hosting;

/// Room in each model's output queue beyond what one step may emit. Small, so
/// the loop's flow control (take an event only while there is room for what
/// it may produce) is exercised.
const SLACK: u32 = 3;

/// The world's name for the worker, in the engine's vocabulary.
const NAME: Token = Token::new(1);

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

/// An agent process's limits in the calm world: room for the one run it
/// carries, with a few conversations, sub-agents nested two deep beneath
/// main.
pub const LIMITS: Limits = Limits {
    run: run::Limits {
        runs: 1,
        conversations: 6,
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
        sessions: 6,
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
            kits: 6,
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

/// An agent process's limits in random worlds: room for fewer conversations,
/// sessions and calls than its run may ask for, so that some are refused.
pub const TIGHT: Limits = Limits {
    run: run::Limits {
        conversations: 4,
        calls: 6,
        run_conversations: 4,
        answer_bytes: 256,
        guide_bytes: 256,
        check_tail: 128,
        ..LIMITS.run
    },
    session: session::Limits {
        sessions: 4,
        messages: 32,
        session_bytes: 1 << 16,
        retries: 2,
        call_timeout: Duration::from_secs(20),
        tool_timeout: Duration::from_secs(30),
        parallel_tools: 3,
        tools: tools::Limits { kits: 4, calls: 3, ..LIMITS.session.tools },
        ..LIMITS.session
    },
};

/// The worker's limits in the calm world: three runs at once, of up to two
/// repositories each, with room for the largest charter and outcome an
/// agent's run takes, and a watchdog no run of the scripts trips.
pub const WORKER: worker::Limits = worker::Limits {
    host: host::Limits {
        slots: 3,
        repositories: 2,
        name_bytes: 32,
        charter_bytes: 2048,
        snapshot_bytes: 64,
        outcome_bytes: 8192,
        detail_bytes: 64,
        held: 2,
        event_bytes: 64,
        run_calls: 2,
        facts: 256,
    },
    checkout: worker::checkout::Limits {
        workspaces: 4,
        repositories: 2,
        name_bytes: 32,
        message_bytes: 8192,
        remote_timeout: Duration::from_secs(60),
        local_timeout: Duration::from_secs(10),
        facts: 256,
    },
    agent: worker::agent::Limits {
        agents: 3,
        charter_bytes: 2048,
        snapshot_bytes: 64,
        event_bytes: 64,
        events: 2,
        calls: 2,
        call_bytes: 8192,
        answer_bytes: 64,
        fact_bytes: 32,
        outcome_bytes: 8192,
        detail_bytes: 64,
        spawn_timeout: Duration::from_secs(10),
        no_progress: Duration::from_secs(300),
        long_span: Duration::from_secs(120),
        wall_time: Duration::from_secs(7200),
        grace: Duration::from_secs(10),
        kill_after: Duration::from_secs(5),
        facts: 256,
    },
    grace: Duration::from_secs(120),
    redial: Duration::from_secs(1),
    redial_max: Duration::from_secs(8),
    told: 256,
    stalled: 8,
};

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Settings {
    /// Seeds the world, which seeds the models.
    pub seed: u64,
    /// Each agent process's limits, and the worker's.
    pub limits: Limits,
    pub worker: worker::Limits,
    pub engine: Config,
    pub provider: provider::Config,
    /// One-way latency between an agent and the provider, and between the
    /// worker and the engine.
    pub network: Span,
    /// How long io takes to spawn an agent process; how long a message takes
    /// through its pipes, and io to tell of a signal, an exit or a reap; and
    /// how long a git operation takes.
    pub spawn: Span,
    pub pipe: Span,
    pub git: Span,
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
    /// The chance, per mille, that another party moves the push branch of a
    /// run's first writable repository, a drawn `move_after` after the run
    /// started; and that a repository on the forge refuses every push.
    pub moved: u32,
    pub move_after: Span,
    pub refusing: u32,
    /// The grid every delivery is rounded up to, so that some come at the
    /// same instant; zero for none.
    pub granule: Duration,
}

impl Settings {
    /// A world where nothing goes wrong: one coding run, on a charter that
    /// grants sub-agents and wants a change that passes its checks, answered
    /// well within every deadline.
    #[must_use]
    pub fn calm(seed: u64) -> Settings {
        Settings {
            seed,
            limits: LIMITS,
            worker: WORKER,
            engine: Config {
                items: 1,
                window: Duration::from_millis(1),
                workers: 1,
                workstreams: Box::new([b"parser".as_slice().into()]),
                repositories: Box::new([fixture::origin(fixture::name(Job::Coding))]),
                spread_min: 1,
                spread_max: 1,
                commits: 0,
                branches: 0,
                writable: 1000,
                saves: 0,
                invalid: 0,
                brief_min: 64,
                brief_max: 64,
                turns_min: 64,
                turns_max: 64,
                tokens_min: 1 << 20,
                tokens_max: 1 << 20,
                time_min: Duration::from_secs(3600),
                time_max: Duration::from_secs(3600),
                max_tokens: 1024,
                changes: 1000,
                checks: 1000,
                verdicts: 0,
                agents: 1000,
                attempts: 1,
                transient: 0,
                permanent: 0,
                backoff_min: Duration::from_secs(1),
                backoff_max: Duration::from_secs(5),
                wakes: 0,
                wake_min: Duration::from_secs(1),
                wake_max: Duration::from_secs(1),
                resumes: 0,
                overbook: 0,
                inbound: 0,
                inbound_min: Duration::from_secs(1),
                inbound_max: Duration::from_secs(1),
                event_min: 1,
                event_max: 1,
                resends: 0,
                cancels: 0,
                late_cancels: 0,
                stale: 0,
                cancel_min: Duration::from_secs(1),
                cancel_max: Duration::from_secs(60),
                calls: 16,
                relay_min: Duration::from_secs(1),
                relay_max: Duration::from_secs(1),
                relay_errors: 0,
                answer_min: 1,
                answer_max: 1,
                grace: Duration::from_secs(120),
                keeps: 1000,
            },
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
            network: Span::millis(1, 20),
            spawn: Span::millis(10, 100),
            pipe: Span::millis(1, 5),
            git: Span::millis(5, 200),
            tool: Span::millis(10, 200),
            look: Span::millis(1, 50),
            io_errors: 0,
            check: Span::millis(500, 2000),
            cancels_lost: 0,
            moved: 0,
            move_after: Span::millis(0, 1000),
            refusing: 0,
            granule: Duration::ZERO,
        }
    }

    /// These settings, with every workspace the engine draws holding `job`'s
    /// repository alone.
    #[must_use]
    pub fn doing(self, job: Job) -> Settings {
        let repositories = Box::new([fixture::origin(fixture::name(job))]);
        Settings { engine: Config { repositories, ..self.engine }, ..self }
    }

    /// A world of its own for `seed`: runs side by side on every job, in
    /// workspaces of one or two repositories, on charters of every kind, some
    /// beyond an agent's limits, in tight limits; with an engine that retries,
    /// overbooks and cancels, a forge that refuses pushes and whose branches
    /// move, an LLM provider that fails, and io that fails and races, at
    /// chances drawn from the seed.
    #[must_use]
    pub fn random(seed: u64) -> Settings {
        let mut rng = Rng::new(seed ^ 0x5EED_0F0A_0B0D_D1CE);
        let mut chance = |most: u64| u32::try_from(rng.below(most + 1)).expect("a chance per mille");
        let calm = Settings::calm(seed);
        let provider = provider::Config {
            calls: 16,
            latency_min: Duration::from_millis(50),
            latency_max: Duration::from_millis(500 + u64::from(chance(4500))),
            overloaded: chance(50),
            rate_limited: chance(30),
            unavailable: chance(20),
            too_long: chance(5),
            unauthorized: chance(3),
            refused: chance(20),
            no_calls: chance(20),
            malformed: chance(50),
            calls_per_answer: 1 + chance(2),
            tool_rounds: 1 + chance(2),
            ..calm.provider
        };
        // Each job's repository, followed by documentation, so that a
        // workspace of two holds one beside the other, either way round.
        let repositories: Vec<Origin> = script::JOBS
            .into_iter()
            .flat_map(|job| [fixture::origin(fixture::name(job)), fixture::origin(fixture::DOCS)])
            .collect();
        let engine = Config {
            items: 4 + chance(8),
            window: Duration::from_millis(1 + u64::from(chance(30_000))),
            workstreams: Box::new([b"parser".as_slice().into(), b"lexer".as_slice().into(), b"site".as_slice().into()]),
            repositories: repositories.into(),
            spread_max: 2,
            branches: chance(300),
            writable: 900,
            saves: chance(800),
            invalid: chance(50),
            brief_min: 16,
            brief_max: 256,
            turns_min: 6,
            // Past the agent's budget now and then: a run it refuses.
            turns_max: 72,
            tokens_min: 2000,
            tokens_max: 1 << 20,
            time_min: Duration::from_secs(10),
            time_max: Duration::from_secs(600),
            max_tokens: 256 + chance(768),
            changes: 800,
            checks: 700,
            verdicts: 500,
            agents: 700,
            attempts: 3,
            transient: 500,
            permanent: 200,
            overbook: chance(300),
            cancels: chance(300),
            late_cancels: chance(500),
            cancel_min: Duration::ZERO,
            cancel_max: Duration::from_secs(30),
            ..calm.engine
        };
        // A grace some runs outstay as they wind down, and in some worlds a
        // watchdog and a wall time that some runs trip.
        let watched = chance(1) == 1;
        let agent = worker::agent::Limits {
            grace: Duration::from_millis(1000 + u64::from(chance(4000))),
            kill_after: Duration::from_millis(500 + u64::from(chance(1500))),
            no_progress: if watched {
                Duration::from_millis(2000 + u64::from(chance(8000)))
            } else {
                WORKER.agent.no_progress
            },
            wall_time: if watched { Duration::from_secs(20 + u64::from(chance(180))) } else { WORKER.agent.wall_time },
            ..WORKER.agent
        };
        let worker = worker::Limits { agent, ..WORKER };
        let granule = match chance(2) {
            0 => Duration::ZERO,
            1 => Duration::from_millis(50),
            _ => Duration::from_secs(1),
        };
        Settings {
            seed,
            limits: TIGHT,
            worker,
            engine,
            provider,
            network: Span::millis(1, 10 + u64::from(chance(190))),
            git: Span::millis(5, 200 + u64::from(chance(1800))),
            tool: Span::millis(1, 100 + u64::from(chance(1900))),
            look: Span::millis(1, 10 + u64::from(chance(90))),
            io_errors: chance(30),
            check: Span::millis(100, 1000 + u64::from(chance(89_000))),
            cancels_lost: chance(300),
            moved: chance(500),
            move_after: Span::millis(0, 5_000),
            refusing: chance(200),
            granule,
            ..calm
        }
    }

    /// These settings, with the first seed from theirs whose first run the
    /// engine assigns on a charter that is `wanted`: the fake draws its
    /// charters, and a scenario picks one it can tell a story about.
    #[must_use]
    pub fn drawing(self, wanted: impl Fn(&run::Charter) -> bool) -> Settings {
        let mut seed = self.seed;
        loop {
            let settings = Settings { seed, ..self.clone() };
            if wanted(&first_charter(&settings)) {
                return settings;
            }
            seed += 1;
            assert!(seed < self.seed + 10_000, "a seed draws such a charter");
        }
    }
}

/// The seeds the world draws for the engine, the worker, the provider and the
/// agents from its own.
fn seeds(seed: u64) -> [u64; 4] {
    let mut rng = Rng::new(seed);
    [rng.next_u64(), rng.next_u64(), rng.next_u64(), rng.next_u64()]
}

/// The charter of the first run the engine of `settings` assigns, as an agent
/// decodes it, less its repositories.
fn first_charter(settings: &Settings) -> run::Charter {
    let [seed, ..] = seeds(settings.seed);
    let mut model = engine::Model::new(&settings.engine, seed);
    let mut env = Env { now: Time::ZERO, limits: settings.engine.clone() };
    let mut out = Queue::with_capacity(engine::MAX_OUT);
    let hello = api::Hello { slots: settings.worker.host.slots, workstreams: Box::new([]), hosting: Box::new([]) };
    engine::step(&mut model, &env, engine::Event::Hello { worker: NAME, hello }, &mut out);
    loop {
        if model.is_ready() {
            engine::resume(&mut model, &env, &mut out);
        } else {
            env.now = model.next_deadline().expect("an item falls due");
            engine::fire(&mut model, &env, &mut out);
        }
        if let Some(engine::Request::Assign { assignment, .. }) = out.pop() {
            let checkout = run::charter::Checkout { repositories: Box::new([]) };
            return channel::charter(&assignment.charter, checkout);
        }
    }
}

/// What the world counted, as it crossed the boundaries.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct Stats {
    /// Assignments the engine made; the worker's answers, those that said a
    /// run ended, and the repositories they say landed a change.
    pub assigned: u32,
    pub reported: u32,
    pub ended: u32,
    pub landed: u32,
    /// Answers for runs whose workspaces could not be prepared.
    pub unprepared: u32,
    /// Agent processes spawned, those that exited of themselves once their
    /// runs had answered, and those a signal ended.
    pub spawns: u32,
    pub exits: u32,
    pub kills: u32,
    /// git operations io ran for the worker, the cancels it was asked for,
    /// and the pushes that landed a run's change and saved its work; and the
    /// branches another party moved.
    pub git: u32,
    pub git_cancels: u32,
    pub pushes_landed: u32,
    pub saves: u32,
    pub advanced: u32,
    /// The worker's facts.
    pub worker_facts: u32,
    /// Runs the worker started, those admitted, and the answers.
    pub starts: u32,
    pub admitted: u32,
    pub answers: u32,
    /// Calls the agents made of the provider; those that reached it; their
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
    /// Cancels the worker sent that reached a run.
    pub cancels: u32,
    /// Results of the run's tools that went back to the LLMs: sub-agents'
    /// answers, and the rest.
    pub sub_answers: u32,
    pub served: u32,
}

/// The facts the agents told, by kind, as the loop drained them.
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
    pub unpushed: u32,
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

/// A run an agent process carried, as the world saw it, in the order the
/// worker spawned their processes.
#[derive(Debug)]
pub struct Run {
    pub job: Job,
    /// The engine's name for the attempt it is.
    pub attempt: Token,
    /// What its charter allowed, and what it may spend.
    pub allowed: Allowed,
    pub budget: run::Budget,
    /// The agent's name for it, once admitted.
    pub run: Option<Token>,
    /// A cancel reached it.
    pub cancelled: bool,
    /// Whether it found checks in its checkout (a probe that failed finds
    /// none), what came of them, and of its pushes, in order.
    pub found: bool,
    pub checked: Vec<bool>,
    pub pushes: Vec<run::Push>,
    /// Its answer, and when the start reached the agent and the answer left
    /// it.
    pub answer: Option<run::Answer>,
    pub started: Option<Time>,
    pub answered: Option<Time>,
    /// What its conversations used, by the facts, and where it stood when it
    /// first spent past its budget: how many of its conversations were live
    /// then, and how many completions they used after.
    pub used: Spend,
    pub crossed: Option<(u32, u32)>,
    /// The most of its conversations that lived at once.
    pub widest: u32,
    /// A signal ended its agent's process before it had answered.
    pub killed: bool,
    /// How the worker answered the engine for its attempt, by kind, and the
    /// commits it says landed, by the forge's names.
    pub reported: Option<&'static str>,
    pub landed: Vec<u64>,
}

/// What a run may finish with: a change, which must pass the checks its
/// writable repositories have if `checks`; a verdict.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Allowed {
    pub change: bool,
    pub checks: bool,
    pub verdicts: bool,
}

/// An agent model's token for something of its own, with the process it is
/// in: agents in different processes name their things alike.
type Owner = (u64, Token);

/// Something on its way, delivered at its time.
enum Delivery {
    /// An event for the engine, or for the worker.
    Engine(engine::Event),
    Worker(worker::Event),
    /// io has spawned the agent process the worker's `owner` asked for, in
    /// `workspace`.
    Spawned {
        owner: Token,
        workspace: Token,
    },
    /// `message` comes through the pipe down to the agent of `process`.
    Down {
        process: u64,
        message: Down,
    },
    /// The worker reads `message` from the pipe of `process`.
    Read {
        process: u64,
        message: Up,
    },
    /// A signal reaches the tree of `process`.
    Signal {
        process: u64,
    },
    /// The agent of `process`, its run answered, exits.
    Exit {
        process: u64,
    },
    /// A git operation of the worker's ends.
    Git {
        owner: Token,
    },
    /// Another party moves `branch` of `remote`.
    Advance {
        remote: Vec<u8>,
        branch: Vec<u8>,
    },
    /// An event for the agent of `process`.
    Agent {
        process: u64,
        event: Event,
    },
    /// A call arrives at the provider.
    Query {
        call: u64,
        query: provider::api::Query,
    },
    /// The provider's answer arrives back at the agent's side.
    Answer {
        call: u64,
        result: Result<provider::api::Answer, provider::api::Error>,
    },
    /// The agent's side gives up on a call.
    Deadline {
        call: u64,
    },
    /// An operation of the tools ends.
    Ran {
        owner: Owner,
    },
    /// A look of a run's in its checkout ends so.
    Looked {
        owner: Owner,
        event: Event,
    },
    /// A run's checks end.
    Checked {
        owner: Owner,
    },
}

/// An assignment the engine made, as the world keeps it.
#[derive(Debug)]
struct Attempt {
    /// The job its first repository of a job is for.
    job: Job,
    repositories: Vec<Repository>,
    /// The agent process that carries its run, once started.
    process: Option<u64>,
    /// The worker has answered for it.
    answered: bool,
}

/// An agent process, as io and the agent's protocol layer keep it, and what
/// the world follows of its run.
struct Process {
    /// The worker's token for it, which io's terminals echo.
    owner: Token,
    workspace: Token,
    /// The agent model, while the process runs, driven on a stage of its own.
    agent: Option<agent::Model>,
    stage: Stage<Limits, Event, Request>,
    /// What came down the channel, in order, that the agent has yet to take.
    heard: VecDeque<Down>,
    /// The channel, as the agent's protocol layer has it, once the start came
    /// down.
    link: Option<Link>,
    run: Option<Run>,
    /// The facts the agent told in this iteration's steps, each with the
    /// number of requests submitted before its step's.
    facts: VecDeque<(u64, Fact)>,
    submitted: u64,
    /// What the agent wrote up the channel and the worker has yet to read,
    /// each with when it is through the pipe; and what the worker waits for.
    written: VecDeque<(Time, Up)>,
    demands: Demands,
    /// Whether the worker has read how the run finishes.
    finish_read: bool,
    /// When it exited; when io tells the worker so; and whether its tree has
    /// been reaped.
    exited: Option<Time>,
    told_exit: Option<Time>,
    reaped: bool,
    /// How many facts its agent dropped for want of room, once it has exited.
    lost: u64,
    /// The conversations of its run that live, by the facts, and how many
    /// messages of each session's prompts have had their results counted.
    live: BTreeSet<Token>,
    counted: BTreeMap<Token, usize>,
}

/// What the worker waits for of an agent process: to read the next message
/// up, for it to exit, and for its tree to be empty.
#[derive(Default, Debug)]
struct Demands {
    read: bool,
    wait: bool,
    reap: bool,
}

/// A workspace io holds, by its name for it.
#[derive(Default, Debug)]
struct Space {
    /// The agent process spawned in it last.
    process: Option<u64>,
}

/// A call of an agent in flight, as its protocol layer would keep it.
struct Call {
    owner: Owner,
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
    /// A file operation, run on the disk then.
    File(tools::Op),
    /// A command started, finished then.
    Command(temper_agent_model_tools_tests::translate::Started),
    /// Nothing more: it ends so (it failed to start, timed out, or was
    /// cancelled).
    Ending(tools::Done),
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
    /// Seeds each agent model as its process is spawned.
    agent_rng: Rng,
    settings: Settings,

    engine: engine::Model,
    engine_stage: Stage<Config, engine::Event, engine::Request>,
    worker: worker::Model,
    worker_stage: Stage<worker::Limits, worker::Event, worker::Request>,
    provider: provider::Model,
    provider_stage: Stage<provider::Config, provider::Event, provider::Request>,
    /// The agent processes io spawned, by its name for each, in the order it
    /// spawned them; those that have exited too.
    processes: BTreeMap<u64, Process>,

    /// Deliveries in flight, whose count names calls and processes too.
    wire: Schedule<Delivery>,
    /// The channel between the worker and the engine: whether the worker has
    /// dialled, and when the last message each way arrives, so that the
    /// channel keeps their order.
    dialled: bool,
    up_lane: Time,
    down_lane: Time,
    /// What the protocol layers keep for the engine's traffic: the places of
    /// inbound events, and the commit the engine's one hash stands for.
    places: BTreeMap<Token, u64>,
    commit: Commit,
    /// The engine's assignments, by its names for their attempts.
    attempts: BTreeMap<Token, Attempt>,
    save_branches: BTreeSet<Vec<u8>>,

    /// The forge and the disk: the working trees the worker prepares are the
    /// roots the agents' tools work in.
    forge: Forge,
    disk: Checkout,
    /// The workspaces io holds, the git operations in flight, and the process
    /// of each root io named for an agent.
    spaces: BTreeMap<Token, Space>,
    git: Ledger<Token, Op>,
    roots: BTreeMap<u64, u64>,

    /// The agents' calls of the provider in flight, the call each session has
    /// in flight, the sessions whose cancel lost its race, and the calls the
    /// provider has not answered yet.
    calls: Ledger<u64, Call>,
    calling: BTreeMap<Owner, u64>,
    cancel_lost: BTreeSet<Owner>,
    serving: Ledger<u64, ()>,
    /// The tools' operations in flight, every token one has had, and those
    /// whose cancel lost its race.
    ops: Ledger<Owner, Pending>,
    owners: BTreeSet<Owner>,
    op_cancel_lost: BTreeSet<Owner>,
    /// The runs' looks in flight, their checks, those whose abort lost its
    /// race, and the finishing calls whose checks passed last.
    looks: Ledger<Owner, Key>,
    checks: Ledger<Owner, Checking>,
    abort_lost: BTreeSet<Owner>,
    passed: BTreeSet<Owner>,
    /// The runs' pushes in flight.
    pushes: Ledger<Owner, ()>,

    /// What the scenario expects of the worker and the agent together.
    referee: Referee<Meeting>,

    stats: Stats,
    told: Told,
    trace: Trace,
}

impl World {
    #[must_use]
    pub fn new(settings: Settings) -> World {
        assert!(agent::worst_case(&settings.limits).is_some(), "the shell refuses limits it cannot provision");
        assert!(worker::worst_case(&settings.worker).is_some(), "the shell refuses limits it cannot provision");
        let [engine_seed, worker_seed, provider_seed, agent_seed] = seeds(settings.seed);
        let mut rng = Rng::new(settings.seed.rotate_left(32));
        let mut forge = Forge::new(settings.seed);
        let first = fixture::seed(&mut forge, &settings.engine.repositories);
        let mut seen = BTreeSet::new();
        for origin in &settings.engine.repositories {
            if seen.insert(origin.remote.clone()) && rng.chance(settings.refusing) {
                forge.set_refusing(&origin.remote, true);
            }
        }
        let mut disk = Checkout::new();
        fixture::script(&mut disk);
        let worker_max = worker::max_out(&settings.worker);
        let mut world = World {
            now: Time::ZERO,
            rng,
            agent_rng: Rng::new(agent_seed),
            engine: engine::Model::new(&settings.engine, engine_seed),
            engine_stage: Stage::new(settings.engine.clone(), engine::MAX_OUT, engine::MAX_OUT + SLACK),
            worker: worker::Model::new(&settings.worker, worker_seed),
            worker_stage: Stage::new(settings.worker, worker_max, worker_max + SLACK),
            provider: provider::Model::scripted(&settings.provider, provider_seed, script::all()),
            provider_stage: Stage::new(settings.provider, provider::MAX_OUT, provider::MAX_OUT + SLACK),
            processes: BTreeMap::new(),
            wire: Schedule::new(),
            dialled: false,
            up_lane: Time::ZERO,
            down_lane: Time::ZERO,
            places: BTreeMap::new(),
            commit: io::commit(first),
            attempts: BTreeMap::new(),
            save_branches: BTreeSet::new(),
            forge,
            disk,
            spaces: BTreeMap::new(),
            git: Ledger::new("git operation"),
            roots: BTreeMap::new(),
            calls: Ledger::new("call to the provider"),
            calling: BTreeMap::new(),
            cancel_lost: BTreeSet::new(),
            serving: Ledger::new("call the provider serves"),
            ops: Ledger::new("operation of the tools"),
            owners: BTreeSet::new(),
            op_cancel_lost: BTreeSet::new(),
            looks: Ledger::new("look in a checkout"),
            checks: Ledger::new("run of the checks"),
            abort_lost: BTreeSet::new(),
            passed: BTreeSet::new(),
            pushes: Ledger::new("push"),
            referee: Referee::new(Meeting::new(answered_within(&settings.worker))),
            stats: Stats::default(),
            told: Told::default(),
            trace: Trace::default(),
            settings,
        };
        // The branches the fixture made.
        world.observe_moves();
        world
    }

    #[must_use]
    pub fn now(&self) -> Time {
        self.now
    }

    #[must_use]
    pub fn stats(&self) -> Stats {
        self.stats
    }

    /// The facts the agents told, and how many they dropped for want of room.
    #[must_use]
    pub fn told(&self) -> (Told, u64) {
        (self.told, self.facts_lost())
    }

    /// What the engine has done and heard.
    #[must_use]
    pub fn tally(&self) -> engine::Tally {
        self.engine.tally()
    }

    /// How many safety checks the referee made, and how many liveness
    /// expectations it saw met.
    #[must_use]
    pub fn judged(&self) -> (u64, u64) {
        self.referee.judged()
    }

    /// What crossed between the models and the world, in order, with times.
    #[must_use]
    pub fn trace(&self) -> &[String] {
        self.trace.lines()
    }

    /// The runs the agents carried, in the order the worker spawned their
    /// processes.
    pub fn runs(&self) -> impl Iterator<Item = &Run> {
        self.processes.values().filter_map(|process| process.run.as_ref())
    }

    /// The content of `path` in the first repository `run` landed a change
    /// in, as it is on the forge.
    #[must_use]
    pub fn landed(&self, run: &Run, path: &[u8]) -> Option<Vec<u8>> {
        let commit = run.landed.first()?;
        self.forge.tree(*commit).get(path).cloned()
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

    /// One iteration of the loop, as the shells would run it.
    fn iterate(&mut self) {
        let now = self.now;
        self.engine_stage.tick(now);
        self.worker_stage.tick(now);
        self.provider_stage.tick(now);
        for process in self.processes.values_mut() {
            process.stage.tick(now);
        }
        self.deliver();
        // The referee fires what is due: a deadline that passes fails the
        // test, and the stimuli due are injected.
        if self.referee.is_due(now) {
            let mut stimuli = Vec::new();
            self.referee.fire(now, &mut stimuli);
            self.referee.assert_holding(self.settings.seed);
            for stimulus in &stimuli {
                inject(stimulus);
            }
        }

        // Each stage resumes what is ready, then takes its events, then fires
        // its alarms, while it has room for what one more may produce.
        while self.engine_stage.has_room() && self.engine.is_ready() {
            engine::resume(&mut self.engine, &self.engine_stage.env, &mut self.engine_stage.out);
        }
        while let Some(event) = self.engine_stage.next_event() {
            engine::step(&mut self.engine, &self.engine_stage.env, event, &mut self.engine_stage.out);
        }
        while self.engine_stage.has_room() && self.engine.is_due(now) {
            engine::fire(&mut self.engine, &self.engine_stage.env, &mut self.engine_stage.out);
        }
        while self.worker_stage.has_room() && self.worker.is_ready() {
            worker::resume(&mut self.worker, &self.worker_stage.env, &mut self.worker_stage.out);
        }
        while let Some(event) = self.worker_stage.next_event() {
            self.log(&format!("worker <- {}", hosting::describe_event(&event)));
            worker::step(&mut self.worker, &self.worker_stage.env, event, &mut self.worker_stage.out);
        }
        while self.worker_stage.has_room() && self.worker.is_due(now) {
            self.log("worker alarm");
            worker::fire(&mut self.worker, &self.worker_stage.env, &mut self.worker_stage.out);
        }
        // The worker's facts, drained as its shell would write them out, and
        // the runs' facts it forwards to the engine.
        while self.worker.pop_fact().is_some() {
            self.stats.worker_facts += 1;
        }
        while let Some(told) = self.worker.pop_told() {
            self.send_up(engine::Event::Fact { worker: NAME, run: told.run, attempt: told.attempt });
        }
        let live: Vec<u64> =
            self.processes.iter().filter(|(_, process)| process.agent.is_some()).map(|(id, _)| *id).collect();
        for id in &live {
            self.drive(*id);
        }
        while let Some(event) = self.provider_stage.next_event() {
            provider::step(&mut self.provider, &self.provider_stage.env, event, &mut self.provider_stage.out);
        }
        while self.provider_stage.has_room() && self.provider.is_due(now) {
            provider::fire(&mut self.provider, &self.provider_stage.env, &mut self.provider_stage.out);
        }

        // What the steps asked for, submitted at the end of the iteration.
        while let Some(request) = self.engine_stage.out.pop() {
            self.send_down(request);
        }
        while let Some(request) = self.worker_stage.out.pop() {
            self.worker_request(request);
        }
        for id in &live {
            self.submit(*id);
        }
        while let Some(request) = self.provider_stage.out.pop() {
            self.provider_request(request);
        }

        // The reclaim point.
        self.engine.reclaim();
        self.worker.reclaim();
        self.provider.reclaim();
        for process in self.processes.values_mut() {
            if let Some(agent) = &mut process.agent {
                agent.reclaim();
            }
        }
        self.assert_bounded();
    }

    /// Hands every delivery that is due to its destination.
    fn deliver(&mut self) {
        while let Some(delivery) = self.wire.next(self.now) {
            match delivery {
                Delivery::Engine(event) => self.engine_stage.push(event),
                Delivery::Worker(event) => self.worker_stage.push(event),
                Delivery::Spawned { owner, workspace } => self.spawned(owner, workspace),
                Delivery::Down { process, message } => self.heard(process, message),
                Delivery::Read { process, message } => self.read_up(process, message),
                Delivery::Signal { process } => self.signalled(process),
                Delivery::Exit { process } => {
                    if self.processes.get(&process).is_some_and(|process| process.agent.is_some()) {
                        self.end(process, false);
                    }
                }
                Delivery::Git { owner } => self.ran_git(owner),
                Delivery::Advance { remote, branch } => self.advance(&remote, &branch),
                Delivery::Agent { process, event } => self.agent_event(process, event),
                Delivery::Query { call, query } => {
                    self.serving.open(call, ());
                    let reply_to = temper_lib::ReplyTo::new(Token::new(call));
                    self.provider_stage.push(provider::Event::Call { reply_to, query });
                    self.stats.provider_calls += 1;
                }
                Delivery::Answer { call, result } => self.answer(call, result),
                Delivery::Deadline { call } => {
                    let Call { owner, .. } =
                        self.end_call(call).expect("a deadline is withdrawn when its call ends first");
                    self.cancel_lost.remove(&owner);
                    let (process, owner) = owner;
                    self.agent_event(process, Event::Failed { owner, failure: agent::llm::Failure::TimedOut });
                    self.stats.failed += 1;
                    self.stats.timeouts += 1;
                }
                Delivery::Ran { owner } => self.ran(owner),
                Delivery::Looked { owner, event } => {
                    self.looks.end(owner);
                    self.agent_event(owner.0, event);
                }
                Delivery::Checked { owner } => self.checked(owner),
            }
        }
    }

    /// What holds after every reclaim point: every slab within its limits.
    fn assert_bounded(&self) {
        let limits = &self.settings.limits;
        for process in self.processes.values() {
            let Some(agent) = &process.agent else {
                continue;
            };
            let run = agent.run();
            let sessions = agent.session();
            assert!(run.runs() <= limits.run.runs, "runs stay within their slots");
            assert!(run.conversations() <= limits.run.conversations, "conversations stay within their slots");
            assert!(run.calls() <= limits.run.calls, "the run's calls stay within their slots");
            assert!(sessions.sessions() <= limits.session.sessions, "sessions stay within their slots");
            assert!(sessions.kits() <= limits.session.tools.kits, "a kit at most for each session");
            assert!(agent.peers() <= limits.run.conversations, "a peer at most for each conversation");
            let flights = limits.run.conversations * limits.session.parallel_tools;
            assert!(agent.flights() <= flights, "delegated calls stay within a batch of each session");
            assert!(agent.flights() <= sessions.runs(), "the top level holds a flight only for a call in flight");
        }
        assert!(self.provider.calls() <= self.settings.provider.calls, "the provider's calls stay within its slots");
        let host = self.worker.host();
        assert!(host.hosted() <= self.settings.worker.host.slots, "the worker hosts runs within its slots");
        assert!(self.worker.agent().agents() <= self.settings.worker.agent.agents, "agents stay within their slots");
    }

    /// How many facts the agents dropped for want of room.
    fn facts_lost(&self) -> u64 {
        self.processes
            .values()
            .map(|process| process.agent.as_ref().map_or(process.lost, agent::Model::facts_lost))
            .sum()
    }

    /// The invariants of a world where nothing is left to happen.
    fn assert_settled(&self) {
        assert!(
            self.wire.is_empty()
                && !self.engine_stage.has_events()
                && !self.worker_stage.has_events()
                && !self.provider_stage.has_events(),
            "nothing is on its way"
        );
        // The engine took each attempt's answer once: none was lost, and none
        // came twice.
        let tally = self.engine.tally();
        assert_eq!(self.engine.outstanding(), 0, "the engine has no attempt out");
        assert_eq!(self.engine.next_deadline(), None, "the engine has nothing left due");
        assert_eq!(self.engine.items(), 0, "every item closed");
        assert_eq!((tally.lost, tally.late, tally.duplicates), (0, 0, 0), "every answer came once, in time");
        let taken = tally.busy + tally.invalid + tally.ended + tally.parked + tally.failed;
        assert_eq!(taken, tally.assigned, "the engine took one answer for each attempt");
        assert_eq!(tally.assigned, self.stats.assigned, "the engine assigned what reached the worker");
        assert_eq!(self.stats.reported, self.stats.assigned, "the worker answered every attempt");
        assert_eq!(tally.ended, self.stats.ended, "the engine recorded every run that ended");
        assert_eq!(tally.landed, self.stats.landed, "the engine heard of every change that landed");
        assert!(self.attempts.values().all(|attempt| attempt.answered), "every attempt was answered");
        // The worker.
        let host = self.worker.host();
        assert_eq!(host.hosted(), 0, "every slot is free");
        assert_eq!(host.calls(), 0, "no host call is open");
        assert_eq!(self.worker.checkout().holds(), 0, "no workspace is held");
        assert_eq!(self.worker.workspaces(), 0, "every workspace asked for was released");
        assert_eq!(self.worker.agent().agents(), 0, "no agent is left");
        assert_eq!(self.worker.held(), 0, "no answer is held");
        assert_eq!(self.worker.next_deadline(), None, "no alarm is armed");
        self.git.assert_settled();
        // The agents and their neighbours.
        for (id, process) in &self.processes {
            assert!(process.agent.is_none(), "the agent of process {id} has exited");
            assert!(process.reaped && process.written.is_empty(), "process {id} was reaped and read to its end");
            if let Some(run) = &process.run {
                assert!(run.answer.is_some() || run.killed, "the run of process {id} answered, or was killed");
            }
        }
        assert_eq!(self.provider.calls(), 0, "the provider holds no call");
        self.calls.assert_settled();
        self.serving.assert_settled();
        self.ops.assert_settled();
        self.looks.assert_settled();
        self.checks.assert_settled();
        self.pushes.assert_settled();
        assert!(self.calling.is_empty() && self.cancel_lost.is_empty(), "no call is in flight");
        assert!(self.op_cancel_lost.is_empty() && self.abort_lost.is_empty(), "every lost cancel's race ended");
        if self.stats.kills == 0 && self.facts_lost() == 0 {
            self.assert_told();
        }
        self.referee.assert_passed(self.settings.seed);
    }

    fn has_work_now(&self) -> bool {
        self.engine_stage.has_events()
            || self.worker_stage.has_events()
            || self.provider_stage.has_events()
            || self.engine.is_ready()
            || self.engine.is_due(self.now)
            || self.worker.is_ready()
            || self.worker.is_due(self.now)
            || self.provider.is_due(self.now)
            || self.wire.is_due(self.now)
            || self.referee.is_due(self.now)
            || self.processes.values().any(|process| match &process.agent {
                Some(agent) => {
                    process.stage.has_events()
                        || !process.heard.is_empty()
                        || agent.is_ready()
                        || agent.is_due(self.now)
                }
                None => false,
            })
    }

    fn next_time(&self) -> Option<Time> {
        let agents = self.processes.values().filter_map(|process| process.agent.as_ref()?.next_deadline());
        [
            self.wire.next_time(),
            self.engine.next_deadline(),
            self.worker.next_deadline(),
            self.provider.next_deadline(),
            self.referee.next_deadline(),
        ]
        .into_iter()
        .flatten()
        .chain(agents)
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

    /// The referee observes `seen`, which ends the test if it breaks an
    /// expectation.
    fn observe(&mut self, seen: Seen) {
        let mut stimuli = Vec::new();
        self.referee.observe(self.now, seen, &mut stimuli);
        self.referee.assert_holding(self.settings.seed);
        for stimulus in &stimuli {
            inject(stimulus);
        }
    }

    fn log(&mut self, line: &str) {
        self.trace.log(self.now, line);
    }
}

/// What the referee injects: nothing, in this world.
fn inject(stimulus: &Stimulus) {
    match *stimulus {}
}

/// How long the worker may take to answer an assignment: the wall time its
/// watchdog gives a run, and five minutes to prepare the run's workspace and
/// to wind the run down.
fn answered_within(worker: &worker::Limits) -> Duration {
    worker.agent.wall_time.saturating_add(Duration::from_secs(300))
}

/// What a repository of `workspace` holds, less its git directory.
fn files(disk: &Checkout, workspace: Token, repository: &[u8]) -> Files {
    let at = temper_worker_model::checkout::git::Place { workspace, repository: repository.into() };
    let mut files = disk.tree(&io::path(&at));
    files.retain(|path, _| !temper_checkout_fake::in_git(path));
    files
}

/// The kind of `answer`, for the trace.
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
