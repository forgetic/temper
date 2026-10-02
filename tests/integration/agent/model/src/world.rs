use std::collections::{BTreeMap, BTreeSet, VecDeque};

use temper_agent_model::run::outcome::Declared;
use temper_agent_model::run::{self, Spend};
use temper_agent_model::{self as agent, Event, Fact, Limits, Request, session, tools};
use temper_checkout_fake::Checkout;
use temper_checkout_fake::git::Tree as Files;
use temper_engine_model::plan::Budget;
use temper_engine_model::{self as engine, Item};
use temper_engine_model_tests::deployment::{self, MAIN, REPOSITORIES};
use temper_engine_model_tests::mirror::Mirror;
use temper_engine_model_tests::referee as engine_referee;
use temper_engine_model_tests::store::{self, Store};
use temper_engine_model_tests::translate::Asked;
use temper_forge_model::{self as forge, Skew};
use temper_lib::{Duration, Env, Queue, Rng, Time, Token};
use temper_llm_model as provider;
use temper_worker_model::checkout::git::Op;
use temper_worker_model::{self as worker, host};
use temper_worker_model_checkout_tests::translate as io;
use temper_world::{Key, Ledger, Referee, Schedule, Span, Stage, Trace, Verdict};

use crate::desk::{self, CODING, Hand, Reviewer};
use crate::fixture;
use crate::forge as shared;
use crate::people;
use crate::referee::{Meeting, Repository, Seen};
use crate::script::{self, Job};

mod agents;
mod engine_side;
mod hosting;

/// The world's own bounds: lines of its trace and deliveries scheduled at
/// once. A world past one fails with its seed, rather than grow.
const TRACE: usize = 400_000;
const DELIVERIES: usize = 20_000;

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

/// The most a step's run may ask for, in the engine's terms: past the
/// agent's budget in turns, so that some charters are refused.
pub const MOST: Budget = Budget { tokens: 1 << 20, turns: 72, time: Duration::from_secs(3600) };

/// What a calm run is given.
pub const CALM: Budget = Budget { tokens: 1 << 20, turns: 64, time: Duration::from_secs(3600) };

/// The engine's limits: the engine world's, with room for what an agent's run
/// may ask, and few rebases of a change.
pub const ENGINE: engine::Limits = engine::Limits {
    plan: engine::plan::Limits { budget: MOST, rebases: 2, ..deployment::LIMITS.plan },
    ..deployment::LIMITS
};

/// The deployment's configuration: the engine world's, with the models the
/// fake provider plays, and room in the rules for what a run may spend.
#[must_use]
pub fn config() -> engine::Config {
    let mut config = deployment::config();
    config.models = b"fake-1 fake-2 fake-3".as_slice().into();
    config.rules.run_spend = MOST.tokens;
    config
}

/// The fake forge's limits: room for every commit and file the runs make,
/// whatever their LLMs write, and for every call, webhook and CI verdict.
/// Nothing fills: a forge that refuses a write, a call, a webhook or a
/// verdict for want of room fails the world.
pub const FORGE: forge::Limits = forge::Limits {
    repositories: 2,
    users: 16,
    labels: 8,
    items: 64,
    comments: 1_024,
    reviews: 64,
    dependencies: 8,
    branches: 64,
    commits: 4_096,
    files: 256,
    statuses: 256,
    contexts: 2,
    pages: 16,
    name_bytes: 256,
    title_bytes: 64,
    body_bytes: 16_384,
    content_bytes: 65_536,
    page_size: 64,
    calls: 64,
    hooks: 64,
    observations: 4_096,
};

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Settings {
    /// Seeds the world, which seeds the models.
    pub seed: u64,
    /// Each agent process's limits, the worker's and the engine's.
    pub limits: Limits,
    pub worker: worker::Limits,
    pub engine: engine::Limits,
    pub forge: forge::Config,
    pub provider: provider::Config,
    /// What people hand in, and when.
    pub hands: Vec<Hand>,
    /// One-way latency between an agent and the provider, and between the
    /// worker and the engine.
    pub network: Span,
    /// Between the reviewer's looks at the forge.
    pub people: Span,
    pub store: store::Script,
    /// The engine's forge protocol layer's deadline for a call.
    pub timeout: Duration,
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
    /// look of a run's; and that a git operation of the worker's cannot reach
    /// the forge.
    pub io_errors: u32,
    pub git_errors: u32,
    /// How long a run's checks take.
    pub check: Span,
    /// The chance, per mille, that a cancel loses its race: the call, the
    /// operation or the checks it was for end of themselves, and that is their
    /// terminal event. A cancel that wins is told after a network draw.
    pub cancels_lost: u32,
    /// The chance, per mille, that a person stops an item's first run, a
    /// drawn `stop_after` after the engine assigned it.
    pub stops: u32,
    pub stop_after: Span,
    /// The chance, per mille, that another party moves the push branch of a
    /// run, a drawn `move_after` after the run started; and that a repository
    /// on the forge refuses every push.
    pub moved: u32,
    pub move_after: Span,
    pub refusing: u32,
    /// The grid every delivery is rounded up to, so that some come at the
    /// same instant; zero for none.
    pub granule: Duration,
    /// Whether the engine may hold an item for its writes once the forge
    /// refused one of its merges for a conflict, as it does today, where the
    /// change is its run's to repair (engine-model.md, 5.1).
    pub conflicts_held: bool,
}

impl Settings {
    /// A world where nothing goes wrong: one issue handed in, a change in the
    /// first repository that must pass its checks and that a person reviews,
    /// on a charter that grants sub-agents, answered well within every
    /// deadline.
    #[must_use]
    pub fn calm(seed: u64) -> Settings {
        let hand = Hand {
            at: Duration::ZERO,
            repository: 0,
            job: Job::Coding,
            work: desk::Work::Change { checks: true, reviewer: Reviewer::Person },
            grants: CODING,
            budget: CALM,
        };
        Settings {
            seed,
            limits: LIMITS,
            worker: WORKER,
            engine: ENGINE,
            forge: forge::Config {
                limits: FORGE,
                latency_min: Duration::from_millis(20),
                latency_max: Duration::from_millis(400),
                late: 0,
                late_min: Duration::from_secs(5),
                late_max: Duration::from_secs(20),
                unavailable: 0,
                timeouts: 0,
                landing: 0,
                land_min: Duration::from_millis(100),
                land_max: Duration::from_secs(8),
                rate_limit: 0,
                rate_window: Duration::from_secs(60),
                ci: deployment::CI,
                hook_min: Duration::from_millis(50),
                hook_max: Duration::from_millis(500),
                hooks_late: 0,
                hooks_lost: 0,
                resolution: Duration::from_secs(1),
                skew: Skew::None,
                status_updates: false,
                edit_updates: false,
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
            hands: vec![hand],
            network: Span::millis(1, 20),
            people: Span::millis(1_000, 10_000),
            store: store::Script { latency: Span::millis(1, 50), failures: 0, done_anyway: 0 },
            timeout: Duration::from_secs(10),
            spawn: Span::millis(10, 100),
            pipe: Span::millis(1, 5),
            git: Span::millis(5, 200),
            tool: Span::millis(10, 200),
            look: Span::millis(1, 50),
            io_errors: 0,
            git_errors: 0,
            check: Span::millis(500, 2000),
            cancels_lost: 0,
            stops: 0,
            stop_after: Span::millis(0, 12_000),
            moved: 0,
            move_after: Span::millis(0, 1000),
            refusing: 0,
            granule: Duration::ZERO,
            conflicts_held: false,
        }
    }

    /// These settings, with the one issue handed in playing `job` for a step
    /// of `work`.
    #[must_use]
    pub fn doing(self, job: Job, work: desk::Work) -> Settings {
        let hands = self.hands.iter().map(|hand| Hand { job, work, ..hand.clone() }).collect();
        Settings { hands, ..self }
    }

    /// These settings, each issue handed in on a budget of `budget`.
    #[must_use]
    pub fn spending(self, budget: Budget) -> Settings {
        let hands = self.hands.iter().map(|hand| Hand { budget, ..hand.clone() }).collect();
        Settings { hands, ..self }
    }

    /// A world of its own for `seed`: issues handed in over a window, in both
    /// repositories, of every job, as agent steps and as changes reviewed by
    /// people and by agents, some of whose checkouts they may write, on
    /// charters of every kind and budgets some of
    /// which are beyond an agent's, in tight limits; with people who stop
    /// runs, a forge that is slow, fails the engine's calls, loses webhooks,
    /// refuses pushes and whose branches move, an LLM provider that fails,
    /// and io that fails and races, at chances drawn from the seed.
    #[must_use]
    pub fn random(seed: u64) -> Settings {
        let mut rng = Rng::new(seed ^ 0x5EED_0F0A_0B0D_D1CE);
        let calm = Settings::calm(seed);
        let provider = provider::Config {
            calls: 16,
            latency_min: Duration::from_millis(50),
            latency_max: Duration::from_millis(500 + u64::from(chance(&mut rng, 4500))),
            overloaded: chance(&mut rng, 50),
            rate_limited: chance(&mut rng, 30),
            unavailable: chance(&mut rng, 20),
            too_long: chance(&mut rng, 5),
            unauthorized: chance(&mut rng, 3),
            refused: chance(&mut rng, 20),
            no_calls: chance(&mut rng, 20),
            malformed: chance(&mut rng, 50),
            calls_per_answer: 1 + chance(&mut rng, 2),
            tool_rounds: 1 + chance(&mut rng, 2),
            ..calm.provider
        };
        let skew = match rng.below(3) {
            0 => Skew::None,
            1 => Skew::Ahead(Duration::from_millis(rng.below(60_000))),
            _ => Skew::Behind(Duration::from_millis(rng.below(60_000))),
        };
        let forge = forge::Config {
            late: chance(&mut rng, 40),
            unavailable: chance(&mut rng, 30),
            timeouts: chance(&mut rng, 30),
            landing: chance(&mut rng, 20),
            rate_limit: if rng.chance(300) { 40 } else { 0 },
            hooks_late: chance(&mut rng, 400),
            hooks_lost: chance(&mut rng, 1_000),
            skew,
            ..calm.forge
        };
        let window = 1 + rng.below(60_000);
        let count = 4 + rng.below(9);
        let hands = (0..count).map(|_| hand(&mut rng, window)).collect();
        // A grace some runs outstay as they wind down, and in some worlds a
        // watchdog and a wall time that some runs trip; and charters that do
        // not fit.
        let watched = chance(&mut rng, 1) == 1;
        let agent = worker::agent::Limits {
            grace: Duration::from_millis(1000 + u64::from(chance(&mut rng, 4000))),
            kill_after: Duration::from_millis(500 + u64::from(chance(&mut rng, 1500))),
            no_progress: if watched {
                Duration::from_millis(2000 + u64::from(chance(&mut rng, 8000)))
            } else {
                WORKER.agent.no_progress
            },
            wall_time: if watched {
                Duration::from_secs(20 + u64::from(chance(&mut rng, 180)))
            } else {
                WORKER.agent.wall_time
            },
            ..WORKER.agent
        };
        let charters = if rng.chance(250) { 150 + u64::from(chance(&mut rng, 250)) } else { WORKER.host.charter_bytes };
        let host = host::Limits { charter_bytes: charters, ..WORKER.host };
        let worker =
            worker::Limits { host, agent: worker::agent::Limits { charter_bytes: charters, ..agent }, ..WORKER };
        let granule = match chance(&mut rng, 2) {
            0 => Duration::ZERO,
            1 => Duration::from_millis(50),
            _ => Duration::from_secs(1),
        };
        let mut settings = Settings {
            seed,
            limits: TIGHT,
            worker,
            forge,
            provider,
            hands,
            store: store::Script { latency: Span::millis(1, 2_000), failures: chance(&mut rng, 60), done_anyway: 300 },
            network: Span::millis(1, 10 + u64::from(chance(&mut rng, 190))),
            git: Span::millis(5, 200 + u64::from(chance(&mut rng, 1800))),
            tool: Span::millis(1, 100 + u64::from(chance(&mut rng, 1900))),
            look: Span::millis(1, 10 + u64::from(chance(&mut rng, 90))),
            io_errors: chance(&mut rng, 30),
            git_errors: chance(&mut rng, 100),
            check: Span::millis(100, 1000 + u64::from(chance(&mut rng, 89_000))),
            cancels_lost: chance(&mut rng, 300),
            stops: chance(&mut rng, 300),
            stop_after: Span::millis(0, 30_000),
            moved: chance(&mut rng, 300),
            move_after: Span::millis(0, 5_000),
            refusing: chance(&mut rng, 150),
            granule,
            conflicts_held: true,
            ..calm
        };
        // In some worlds the forge and the store never fail, so that nothing
        // the engine writes fails for good.
        if rng.chance(300) {
            let forge =
                forge::Config { late: 0, unavailable: 0, timeouts: 0, landing: 0, rate_limit: 0, ..settings.forge };
            settings.forge = forge;
            settings.store.failures = 0;
        }
        settings
    }

    /// Whether the forge or the store may fail the engine's calls, so that a
    /// write, or a record, may fail for good.
    #[must_use]
    pub fn faults(&self) -> bool {
        let forge = &self.forge;
        forge.late + forge.unavailable + forge.timeouts + forge.landing + forge.rate_limit + self.store.failures > 0
    }
}

/// A chance per mille, drawn up to `most`.
fn chance(rng: &mut Rng, most: u64) -> u32 {
    u32::try_from(rng.below(most + 1)).expect("a chance per mille")
}

/// An issue a random world's person hands in, within `window` milliseconds
/// of the world's start.
fn hand(rng: &mut Rng, window: u64) -> Hand {
    let jobs = [Job::Coding, Job::Reporting, Job::Delegating, Job::Spending, Job::Wandering];
    let job = jobs[usize::try_from(rng.below(5)).expect("small")];
    let change = match job {
        Job::Coding | Job::Delegating => true,
        Job::Reporting | Job::Review => false,
        Job::Spending | Job::Wandering => rng.chance(300),
    };
    let reviewer = match rng.below(6) {
        0 => Reviewer::Agent,
        1 => Reviewer::Editor,
        _ => Reviewer::Person,
    };
    let work = if change { desk::Work::Change { checks: rng.chance(700), reviewer } } else { desk::Work::Agent };
    let grants = engine::plan::Grants {
        modify: change || rng.chance(500),
        shell: rng.chance(700),
        forge: rng.chance(500),
        subagents: rng.chance(700),
        note: rng.chance(300),
    };
    let budget = Budget {
        tokens: rng.between(2_000, MOST.tokens),
        turns: u32::try_from(rng.between(6, u64::from(MOST.turns))).expect("a few turns"),
        time: Duration::from_millis(rng.between(10_000, 600_000)),
    };
    Hand {
        at: Duration::from_millis(rng.below(window)),
        repository: u32::try_from(rng.below(2)).expect("two repositories"),
        job,
        work,
        grants,
        budget,
    }
}

/// What the world counted, as it crossed the boundaries.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct Stats {
    /// Issues people handed in.
    pub handed: u32,
    /// Assignments the engine made; the worker's answers, those that said a
    /// run ended, and the repositories they say landed a change.
    pub assigned: u32,
    pub reported: u32,
    pub ended: u32,
    pub landed: u32,
    /// Answers for runs whose workspaces could not be prepared, and refusals
    /// of assignments that do not fit the worker.
    pub unprepared: u32,
    pub invalid: u32,
    /// Pull requests the engine merged, and those a person approved.
    pub merged: u32,
    pub reviews: u32,
    /// Runs people asked the engine to stop, and those it did.
    pub stops: u32,
    pub stopped: u32,
    /// How the issues handed in ended, once the world settled: closed, or
    /// held for a person, by why.
    pub closed: u32,
    pub held_plan: u32,
    pub held_failures: u32,
    pub held_stopped: u32,
    pub held_acceptance: u32,
    pub held_writes: u32,
    pub held_record: u32,
    /// The engine's merges the forge refused for a conflict.
    pub conflicts: u32,
    /// The engine's calls of the forge, those past the protocol layer's
    /// deadline, and those the forge limited; webhooks; the store's
    /// operations that failed; the engine's facts.
    pub forge_calls: u32,
    pub timed_out: u32,
    pub limited: u32,
    pub hints: u32,
    pub store_failed: u32,
    pub engine_facts: u32,
    /// git operations that could not reach the forge.
    pub unreachable: u32,
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
    /// The item it is for, and the script its main conversation plays.
    pub item: Item,
    pub job: Job,
    /// The channel's name for the attempt it is.
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
/// writable repositories have if `checks`; a verdict. And whether any
/// repository of its workspace is writable.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[expect(clippy::struct_excessive_bools, reason = "independent permissions of a charter, not states")]
pub struct Allowed {
    pub change: bool,
    pub checks: bool,
    pub verdicts: bool,
    pub writable: bool,
}

/// An agent model's token for something of its own, with the process it is
/// in: agents in different processes name their things alike.
type Owner = (u64, Token);

/// Something on its way, delivered at its time.
enum Delivery {
    /// An event for the engine, up the worker's channel `channel` if it names
    /// one: dropped if the channel has closed since.
    Engine { channel: Option<Token>, event: engine::Event },
    /// An event for the worker.
    Worker(worker::Event),
    /// The engine's protocol layer's deadline for its forge call `name`.
    Timeout(u64),
    /// The store ends the engine's operation `owner` so.
    Stored { owner: Token, stored: engine::Stored },
    /// A person hands in the issue `hands[at]`.
    Hand(usize),
    /// The reviewer looks at the forge.
    Look,
    /// A person asks the engine to stop the item's run.
    Stop(Item),
    /// io has spawned the agent process the worker's `owner` asked for, in
    /// `workspace`.
    Spawned { owner: Token, workspace: Token },
    /// `message` comes through the pipe down to the agent of `process`.
    Down { process: u64, message: temper_worker_model::agent::channel::Down },
    /// The worker reads `message` from the pipe of `process`.
    Read { process: u64, message: temper_worker_model::agent::channel::Up },
    /// A signal reaches the tree of `process`.
    Signal { process: u64 },
    /// The agent of `process`, its run answered, exits.
    Exit { process: u64 },
    /// A git operation of the worker's ends.
    Git { owner: Token },
    /// Another party moves `branch` of `remote`, unless the attempt it was
    /// drawn for has been answered.
    Advance { attempt: Token, remote: Vec<u8>, branch: Vec<u8> },
    /// An event for the agent of `process`.
    Agent { process: u64, event: Event },
    /// A call arrives at the provider.
    Query { call: u64, query: provider::api::Query },
    /// The provider's answer arrives back at the agent's side.
    Answer { call: u64, result: Result<provider::api::Answer, provider::api::Error> },
    /// The agent's side gives up on a call.
    Deadline { call: u64 },
    /// An operation of the tools ends.
    Ran { owner: Owner },
    /// A look of a run's in its checkout ends so.
    Looked { owner: Owner, event: Event },
    /// A run's checks end.
    Checked { owner: Owner },
}

/// An assignment the engine made, as the world keeps it.
#[derive(Debug)]
struct Attempt {
    item: Item,
    /// The script its runs play.
    job: Job,
    repositories: Vec<Repository>,
    /// The deployment's index for each repository of its workspace.
    places: Vec<u32>,
    /// The agent process that carries its run, once started.
    process: Option<u64>,
    /// The worker has answered for it, and refused it.
    answered: bool,
    refused: bool,
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
    heard: VecDeque<temper_worker_model::agent::channel::Down>,
    /// The channel, as the agent's protocol layer has it, once the start came
    /// down.
    link: Option<crate::channel::Link>,
    run: Option<Run>,
    /// The facts the agent told in this iteration's steps, each with the
    /// number of requests submitted before its step's.
    facts: VecDeque<(u64, Fact)>,
    submitted: u64,
    /// What the agent wrote up the channel and the worker has yet to read,
    /// each with when it is through the pipe; and what the worker waits for.
    written: VecDeque<(Time, temper_worker_model::agent::channel::Up)>,
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

/// An engine call the protocol layer has out on the forge.
#[derive(Debug)]
struct Out {
    call: Token,
    asked: Asked,
    deadline: Key,
    expired: bool,
}

pub struct World {
    now: Time,
    rng: Rng,
    /// Seeds each agent model as its process is spawned.
    agent_rng: Rng,
    settings: Settings,

    engine: engine::Model,
    engine_stage: Stage<engine::Limits, engine::Event, engine::Request>,
    worker: worker::Model,
    worker_stage: Stage<worker::Limits, worker::Event, worker::Request>,
    provider: provider::Model,
    provider_stage: Stage<provider::Config, provider::Event, provider::Request>,
    /// The agent processes io spawned, by its name for each, in the order it
    /// spawned them; those that have exited too.
    processes: BTreeMap<u64, Process>,

    /// Deliveries in flight, whose count names calls and processes too.
    wire: Schedule<Delivery>,
    /// The channel between the worker and the engine: the engine's name for
    /// it while it is open, and when the last message each way arrives, so
    /// that the channel keeps their order.
    channel: Option<Token>,
    up_lane: Time,
    down_lane: Time,
    /// The engine's assignments, by the channel's names for their attempts.
    attempts: BTreeMap<Token, Attempt>,
    save_branches: BTreeSet<Vec<u8>>,

    /// The forge, everyone's; its configuration and what it emitted; the
    /// direct calls made of it, and what it emitted for others meanwhile.
    forge: forge::Model,
    forge_env: Env<forge::Config>,
    forge_out: Queue<forge::Request>,
    direct: u64,
    stray: Vec<forge::Request>,
    /// The engine's calls on the forge, by the protocol layer's names for
    /// them, and by the engine's; the reviewer's calls; people's asks of the
    /// engine; the store's operations.
    calls: Ledger<u64, Out>,
    owned: Ledger<Token, ()>,
    theirs: Ledger<u64, (usize, u64, u64)>,
    asks: Ledger<u64, Item>,
    stores: Ledger<Token, ()>,
    store: Store,
    /// The forge as observed, which people read; the reviewer; the items
    /// handed in, by the hand each is.
    mirror: Mirror,
    reviewer: people::Reviewer,
    looking: bool,
    items: BTreeMap<Item, usize>,
    /// The items whose first run a person may have stopped.
    stopping: BTreeSet<Item>,

    /// The disk: the working trees the worker prepares are the roots the
    /// agents' tools work in. The workspaces io holds, the git operations in
    /// flight, and the process of each root io named for an agent.
    disk: Checkout,
    spaces: BTreeMap<Token, Space>,
    git: Ledger<Token, Op>,
    roots: BTreeMap<u64, u64>,

    /// The agents' calls of the provider in flight, the call each session has
    /// in flight, the sessions whose cancel lost its race, and the calls the
    /// provider has not answered yet.
    calls_out: Ledger<u64, Call>,
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

    /// What the scenario expects of the worker and the agent together, and
    /// of the engine.
    referee: Referee<Meeting>,
    engine_referee: Referee<engine_referee::Engine>,

    stats: Stats,
    told: Told,
    trace: Trace,
}

impl World {
    #[must_use]
    pub fn new(settings: Settings) -> World {
        assert!(agent::worst_case(&settings.limits).is_some(), "the shell refuses limits it cannot provision");
        assert!(worker::worst_case(&settings.worker).is_some(), "the shell refuses limits it cannot provision");
        assert!(engine::worst_case(&settings.engine).is_some(), "the shell refuses limits it cannot provision");
        let [engine_seed, worker_seed, provider_seed, agent_seed] = seeds(settings.seed);
        let mut rng = Rng::new(settings.seed.rotate_left(32));
        let mut forge = forge::Model::new(&settings.forge, rng.next_u64());
        shared::setup(&mut forge, &settings.forge);
        for name in REPOSITORIES {
            if rng.chance(settings.refusing) {
                forge::set_refusing(&mut forge, name, true);
            }
        }
        let mut disk = Checkout::new();
        fixture::script(&mut disk);
        let worker_max = worker::max_out(&settings.worker);
        let engine_max = engine::max_out(&settings.engine);
        let bounds =
            engine_referee::Bounds { story: Duration::from_secs(4 * 3_600), message: Duration::from_secs(3_600) };
        let mut world = World {
            now: Time::ZERO,
            rng,
            agent_rng: Rng::new(agent_seed),
            engine: engine::Model::new(config(), &settings.engine, engine_seed, Time::ZERO),
            engine_stage: Stage::new(settings.engine, engine_max, engine_max + SLACK),
            worker: worker::Model::new(&settings.worker, worker_seed),
            worker_stage: Stage::new(settings.worker, worker_max, worker_max + SLACK),
            provider: provider::Model::scripted(&settings.provider, provider_seed, script::all()),
            provider_stage: Stage::new(settings.provider, provider::MAX_OUT, provider::MAX_OUT + SLACK),
            processes: BTreeMap::new(),
            wire: Schedule::new(),
            channel: None,
            up_lane: Time::ZERO,
            down_lane: Time::ZERO,
            attempts: BTreeMap::new(),
            save_branches: BTreeSet::new(),
            forge,
            forge_env: Env { now: Time::ZERO, limits: settings.forge },
            forge_out: Queue::with_capacity(forge::MAX_OUT),
            direct: 0,
            stray: Vec::new(),
            calls: Ledger::new("engine's forge call"),
            owned: Ledger::new("engine's call, by its name"),
            theirs: Ledger::new("review"),
            asks: Ledger::new("person's ask"),
            stores: Ledger::new("store operation"),
            store: Store::new(settings.store, settings.seed ^ 0x5704_E5ED, 10_000),
            mirror: Mirror::default(),
            reviewer: people::Reviewer::default(),
            looking: false,
            items: BTreeMap::new(),
            stopping: BTreeSet::new(),
            disk,
            spaces: BTreeMap::new(),
            git: Ledger::new("git operation"),
            roots: BTreeMap::new(),
            calls_out: Ledger::new("call to the provider"),
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
            referee: Referee::new(Meeting::new(
                answered_within(&settings.worker),
                settings.faults(),
                settings.conflicts_held,
            )),
            engine_referee: Referee::new(engine_referee::Engine::new(bounds, 0)),
            stats: Stats::default(),
            told: Told::default(),
            trace: Trace::default(),
            settings,
        };
        world.observe_engine(engine_referee::Seen::Start);
        // What the setup made, and the issues handed in at the start.
        world.observe_forge();
        for (at, hand) in world.settings.hands.clone().iter().enumerate() {
            let when = Time::ZERO.saturating_add(hand.at);
            if when == Time::ZERO {
                world.hand_in(at);
            } else {
                world.send_at(when, Delivery::Hand(at));
            }
        }
        world
    }

    #[must_use]
    pub fn now(&self) -> Time {
        self.now
    }

    #[must_use]
    pub fn stats(&self) -> Stats {
        Stats { reviews: self.reviewer.reviews, ..self.stats }
    }

    /// The facts the agents told, and how many they dropped for want of room.
    #[must_use]
    pub fn told(&self) -> (Told, u64) {
        (self.told, self.facts_lost())
    }

    /// How many safety checks the referees made, and how many liveness
    /// expectations they saw met.
    #[must_use]
    pub fn judged(&self) -> (u64, u64) {
        let (checked, met) = self.referee.judged();
        let (engine_checked, engine_met) = self.engine_referee.judged();
        (checked + engine_checked, met + engine_met)
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

    /// The forge as observed.
    #[must_use]
    pub fn mirror(&self) -> &Mirror {
        &self.mirror
    }

    /// The items handed in, in the order of the hands.
    #[must_use]
    pub fn items(&self) -> Vec<Item> {
        let mut items: Vec<(usize, Item)> = self.items.iter().map(|(item, at)| (*at, *item)).collect();
        items.sort_unstable();
        items.into_iter().map(|(_, item)| item).collect()
    }

    /// The content of `path` in the commit `run` landed first, as it is on
    /// the forge.
    #[must_use]
    pub fn landed(&self, run: &Run, path: &[u8]) -> Option<Vec<u8>> {
        let commit = run.landed.first()?;
        shared::tree(&self.forge, *commit).get(path).cloned()
    }

    /// The content of `path` on the default branch of `item`'s repository,
    /// where its change lands.
    #[must_use]
    pub fn merged(&self, item: Item, path: &[u8]) -> Option<Vec<u8>> {
        let repository = deployment::name(item.repository);
        let main = shared::branch(&self.forge, &self.settings.forge, repository, MAIN)?;
        shared::tree(&self.forge, main).get(path).cloned()
    }

    /// Runs until the issues handed in have all ended or are held and nothing
    /// is in flight, then checks the invariants of a settled world. Panics if
    /// it takes more than `iterations`.
    pub fn run(&mut self, iterations: u32) {
        for _ in 0..iterations {
            self.iterate();
            if self.has_work_now() {
                continue;
            }
            if self.is_quiet() {
                self.assert_settled();
                return;
            }
            let next = self.next_time().expect("the engine polls, so there is always a next time");
            assert!(next > self.now, "time moves forward");
            self.now = next;
        }
        panic!(
            "seed {}: the world did not settle in {iterations} iterations, at {:?}: {:?}, unsettled {:?}",
            self.settings.seed,
            self.now,
            self.referee.verdict(),
            self.unsettled()
        );
    }

    /// One iteration of the loop, as the shells would run it.
    fn iterate(&mut self) {
        let now = self.now;
        self.engine_stage.tick(now);
        self.worker_stage.tick(now);
        self.provider_stage.tick(now);
        self.forge_env.now = now;
        for process in self.processes.values_mut() {
            process.stage.tick(now);
        }
        self.deliver();
        while self.forge.is_due(now) {
            forge::fire(&mut self.forge, &self.forge_env, &mut self.forge_out);
            self.drain_forge();
        }
        self.observe_forge();
        // The referees fire what is due: a deadline that passes fails the
        // test.
        for due in [self.referee.is_due(now), self.engine_referee.is_due(now)] {
            if due {
                let mut stimuli = Vec::new();
                self.referee.fire(now, &mut stimuli);
                self.referee.assert_holding(self.settings.seed);
                let mut injected = Vec::new();
                self.engine_referee.fire(now, &mut injected);
                self.engine_referee.assert_holding(self.settings.seed);
                assert!(stimuli.is_empty() && injected.is_empty(), "the referees inject nothing in this world");
            }
        }

        // Each stage resumes what is ready, then takes its events, then fires
        // its alarms, while it has room for what one more may produce.
        while self.engine_stage.has_room() && self.engine.is_ready() {
            engine::resume(&mut self.engine, &self.engine_stage.env, &mut self.engine_stage.out);
        }
        while let Some(event) = self.engine_stage.next_event() {
            self.log(&format!("engine <- {}", engine_side::describe_event(&event)));
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
            let event = crate::protocol::told(told);
            self.send_up(event);
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
            self.engine_request(request);
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
        self.observe_forge();
        while self.engine.pop_fact().is_some() {
            self.stats.engine_facts += 1;
        }

        // The reclaim point.
        self.engine.reclaim();
        self.worker.reclaim();
        self.provider.reclaim();
        self.forge.reclaim();
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
                Delivery::Engine { channel, event } => {
                    if channel.is_none() || channel == self.channel {
                        self.engine_stage.push(event);
                    }
                }
                Delivery::Worker(event) => self.worker_stage.push(event),
                Delivery::Timeout(name) => self.timed_out(name),
                Delivery::Stored { owner, stored } => {
                    self.stores.end(owner);
                    self.engine_stage.push(engine::Event::Stored { owner, stored });
                }
                Delivery::Hand(at) => self.hand_in(at),
                Delivery::Look => self.review(),
                Delivery::Stop(item) => self.stop(item),
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
                Delivery::Advance { attempt, remote, branch } => {
                    if self.attempts.get(&attempt).is_some_and(|attempt| !attempt.answered) {
                        self.advance(&remote, &branch);
                    }
                }
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
        let seed = self.settings.seed;
        assert!(self.wire.len() <= DELIVERIES, "seed {seed}: more deliveries scheduled than the world holds");
        assert!(self.trace.lines().len() <= TRACE, "seed {seed}: the trace grew past its bound");
    }

    /// How many facts the agents dropped for want of room.
    fn facts_lost(&self) -> u64 {
        self.processes
            .values()
            .map(|process| process.agent.as_ref().map_or(process.lost, agent::Model::facts_lost))
            .sum()
    }

    /// Whether the world has settled: every issue handed in, and each ended
    /// or held; the worker idle, every agent gone and the provider idle;
    /// nothing in flight anywhere but the reviewer's next look; and the
    /// referees expecting nothing more.
    fn is_quiet(&self) -> bool {
        let looks = usize::from(self.looking);
        self.items.len() == self.settings.hands.len()
            && self.wire.len() == looks
            && !self.engine_stage.has_events()
            && !self.worker_stage.has_events()
            && !self.provider_stage.has_events()
            && self.worker.host().hosted() == 0
            && self.worker.held() == 0
            && self.worker.agent().agents() == 0
            && self.processes.values().all(|process| process.agent.is_none() && process.reaped)
            && self.git.is_empty()
            && self.provider.calls() == 0
            && self.calls_out.is_empty()
            && self.serving.is_empty()
            && self.ops.is_empty()
            && self.looks.is_empty()
            && self.checks.is_empty()
            && self.pushes.is_empty()
            && self.calls.is_empty()
            && self.theirs.is_empty()
            && self.asks.is_empty()
            && self.forge.calls() == 0
            && self.forge.deliveries() == 0
            && self.reviewer.is_idle()
            && self.unsettled().is_empty()
            && [self.referee.verdict(), self.engine_referee.verdict()].iter().all(|verdict| match verdict {
                Verdict::Open { .. } => false,
                Verdict::Passed | Verdict::Stopped { .. } | Verdict::Failed(_) => true,
            })
    }

    /// The open items the engine tracks, with a record, that are not held.
    fn unsettled(&self) -> Vec<u64> {
        let mut unsettled = Vec::new();
        for ((repository, number), issue) in &self.mirror.issues {
            let tracked = issue.labels.iter().any(|label| **label == *deployment::TRACKING);
            if !tracked || !issue.open || deployment::index(repository).is_none() {
                continue;
            }
            let Some(record) = self.mirror.record(repository, *number) else { continue };
            match record.lifecycle.phase {
                engine::work::Phase::Held { .. } => {}
                engine::work::Phase::Waiting
                | engine::work::Phase::Parked
                | engine::work::Phase::Retrying(_)
                | engine::work::Phase::Claimed
                | engine::work::Phase::Applying { .. }
                | engine::work::Phase::Done => unsettled.push(*number),
            }
        }
        unsettled
    }

    /// The invariants of a world where nothing is left to happen.
    fn assert_settled(&mut self) {
        let seed = self.settings.seed;
        // The engine's neighbours: every call ended, and each assignment was
        // answered once.
        self.calls.assert_settled();
        self.owned.assert_settled();
        self.theirs.assert_settled();
        self.asks.assert_settled();
        self.stores.assert_settled();
        let tally = self.forge.tally();
        assert_eq!(tally.forgotten, 0, "seed {seed}: the forge kept every call it took: {tally:?}");
        assert_eq!(
            (tally.busy, tally.full, tally.hooks_dropped, tally.unreported),
            (0, 0, 0, 0),
            "seed {seed}: nothing filled the forge: {tally:?}"
        );
        assert!(self.attempts.values().all(|attempt| attempt.answered), "seed {seed}: every attempt was answered");
        assert_eq!(self.stats.reported, self.stats.assigned, "seed {seed}: the worker answered every assignment");
        // The worker.
        let host = self.worker.host();
        assert_eq!(host.hosted(), 0, "every slot is free");
        assert_eq!(host.calls(), 0, "no host call is open");
        assert_eq!(self.worker.checkout().holds(), 0, "no workspace is held");
        assert_eq!(self.worker.workspaces(), 0, "every workspace asked for was released");
        assert_eq!(self.worker.agent().agents(), 0, "no agent is left");
        assert_eq!(self.worker.held(), 0, "no answer is held: the engine acknowledged every one");
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
        self.calls_out.assert_settled();
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
        self.count_endings();
        self.observe_engine(engine_referee::Seen::Settled);
        self.referee.assert_passed(seed);
        self.engine_referee.assert_passed(seed);
    }

    /// Counts how each issue handed in ended: closed, or held, by why.
    fn count_endings(&mut self) {
        let seed = self.settings.seed;
        for item in self.items.keys() {
            let repository = deployment::name(item.repository);
            if self.mirror.issue(repository, item.number).is_some_and(|issue| !issue.open) {
                self.stats.closed += 1;
                continue;
            }
            let phase = self.mirror.record(repository, item.number).map(|record| record.lifecycle.phase);
            let count = match phase {
                Some(engine::work::Phase::Held { why, .. }) => match why {
                    engine::work::Hold::Plan { .. } => &mut self.stats.held_plan,
                    engine::work::Hold::Failures(_) => &mut self.stats.held_failures,
                    engine::work::Hold::Stopped => &mut self.stats.held_stopped,
                    engine::work::Hold::Acceptance => &mut self.stats.held_acceptance,
                    engine::work::Hold::Writes => &mut self.stats.held_writes,
                    engine::work::Hold::Record => &mut self.stats.held_record,
                },
                Some(
                    engine::work::Phase::Waiting
                    | engine::work::Phase::Parked
                    | engine::work::Phase::Retrying(_)
                    | engine::work::Phase::Claimed
                    | engine::work::Phase::Applying { .. }
                    | engine::work::Phase::Done,
                )
                | None => panic!("seed {seed}: {item:?} settled open and not held, {phase:?}"),
            };
            *count += 1;
        }
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
            || self.forge.is_due(self.now)
            || self.wire.is_due(self.now)
            || self.referee.is_due(self.now)
            || self.engine_referee.is_due(self.now)
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
            self.forge.next_deadline(),
            self.referee.next_deadline(),
            self.engine_referee.next_deadline(),
        ]
        .into_iter()
        .flatten()
        .chain(agents)
        .min()
    }

    /// Sends `delivery` after a draw of the network's latency.
    fn send(&mut self, delivery: Delivery) {
        let at = self.now.saturating_add(self.draw(self.settings.network));
        self.schedule(at, delivery);
    }

    /// Sends `delivery` at `at`, as it is.
    fn send_at(&mut self, at: Time, delivery: Delivery) -> Key {
        self.wire.send(at, delivery)
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
        assert!(stimuli.is_empty(), "the referee injects nothing in this world");
    }

    /// The engine's referee observes `seen`.
    fn observe_engine(&mut self, seen: engine_referee::Seen) {
        let mut stimuli = Vec::new();
        self.engine_referee.observe(self.now, seen, &mut stimuli);
        self.engine_referee.assert_holding(self.settings.seed);
        assert!(stimuli.is_empty(), "the engine's referee injects nothing in this world");
    }

    fn log(&mut self, line: &str) {
        self.trace.log(self.now, line);
    }
}

/// The seeds the world draws for the engine, the worker, the provider and the
/// agents from its own.
fn seeds(seed: u64) -> [u64; 4] {
    let mut rng = Rng::new(seed);
    [rng.next_u64(), rng.next_u64(), rng.next_u64(), rng.next_u64()]
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
