use std::collections::{BTreeMap, BTreeSet};

use temper_checkout_fake::Checkout;
use temper_checkout_fake::git::{Forge, Tree as Files};
use temper_fake_engine_model::{self as engine, Config, IDENTITY, Origin};
use temper_lib::{Duration, Rng, Time, Token};
use temper_worker_model::agent::{self, channel::Down, channel::Reply};
use temper_worker_model::checkout::git::{Commit, Place};
use temper_worker_model::{self as worker, Event, Hello, Limits, Model, Phase, Request, host};
use temper_worker_model_agent_tests::script::{self, Fates, Said, Sizes};
use temper_worker_model_agent_tests::tree::{self, Tree};
use temper_worker_model_checkout_tests::translate as io;
use temper_world::{Ledger, Schedule, Span, Stage, Trace};

use crate::translate;

mod git;
mod network;

pub use git::Git;
pub use network::Network;

use git::{Pending, files, seed_forge};
use network::Channel;

/// The world's name for the worker, the same across its channels.
const WORKER: Token = Token::new(1);

/// Room in each model's output queue beyond what one step may emit. Small, so
/// the loop's flow control is exercised.
const SPARE: u32 = 2;

/// Where the fake engine pushes a workstream's work: `temper/` and its key.
const PUSH_PREFIX: &[u8] = b"temper/";

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Settings {
    /// Seeds the world, which seeds the engine, the worker, the process trees
    /// and the forge.
    pub seed: u64,
    pub worker: Limits,
    pub engine: Config,
    pub network: Network,
    pub git: Git,
    pub tree: tree::Script,
    pub script: script::Script,
    /// The chance, per mille, that an agent edits its working trees before it
    /// asks to push, and as it tells a fact: only while no push of its own is
    /// under way, so that what it asked to push is what lands.
    pub edits: u32,
    pub scribbles: u32,
    /// The chance, per mille, that the shell tells the worker to shut down, at
    /// a moment drawn from `shutdown_at`.
    pub shutdowns: u32,
    pub shutdown_at: Span,
}

impl Settings {
    /// A world where nothing goes wrong: an engine with a few items, which
    /// it neither cancels nor overbooks; a channel that never drops; a forge
    /// that has every starting point and never fails; agents that work, call,
    /// push what they edited, wait for events, park and end, and exit in good
    /// time.
    #[must_use]
    pub fn calm(seed: u64) -> Settings {
        Settings {
            seed,
            worker: LIMITS,
            engine: engine(),
            network: Network {
                hop: Span::millis(1, 20),
                dial: Span::millis(10, 100),
                drops: 0,
                drop: 0,
                life: Span::millis(10_000, 60_000),
                outage: Span::millis(1_000, 10_000),
            },
            git: GIT,
            tree: TREE,
            script: SCRIPT,
            edits: 800,
            scribbles: 200,
            shutdowns: 0,
            shutdown_at: Span::millis(10_000, 120_000),
        }
    }

    /// A world where everything that can go wrong does, now and then: an
    /// engine that overbooks, cancels, sends stale traffic, assigns beyond
    /// the worker's limits and starts from what the forge may not have; a
    /// channel that drops, for less and more than the worker's grace; a
    /// forge that fails, refuses and moves branches; process trees that fail
    /// to spawn and leave children; agents that misbehave every way the
    /// script knows; and, in some worlds, a shutdown. A world for the random
    /// sweep.
    #[must_use]
    pub fn rough(seed: u64) -> Settings {
        let calm = Settings::calm(seed);
        Settings {
            worker: Limits {
                agent: agent::Limits { wall_time: Duration::from_secs(300), ..calm.worker.agent },
                ..calm.worker
            },
            engine: Config {
                items: 10,
                window: Duration::from_secs(120),
                commits: 50,
                branches: 300,
                saves: 700,
                invalid: 50,
                brief_max: 320,
                permanent: 300,
                overbook: 200,
                inbound: 3,
                event_max: 96,
                resends: 500,
                cancels: 200,
                late_cancels: 300,
                stale: 300,
                relay_errors: 200,
                keeps: 700,
                ..calm.engine
            },
            network: Network {
                hop: Span::millis(1, 300),
                drops: 3,
                drop: 700,
                outage: Span::millis(1_000, 150_000),
                ..calm.network
            },
            git: Git {
                stalls: 20,
                broken: 20,
                ambiguous: 20,
                unreachable: 30,
                refusing: 30,
                cancels_lost: 200,
                advance: 200,
                trunks: 300,
                branched: 700,
                ..calm.git
            },
            tree: tree::Script { unspawned: 30, children: 2, lingering: 200, holding: 200, stubborn: 200, ..calm.tree },
            script: script::Script {
                fates: Fates {
                    ended: 8,
                    parked: 4,
                    failed: 3,
                    crash: 1,
                    hang: 1,
                    overrun: 1,
                    garbage: 1,
                    duplicate: 1,
                    trailing: 1,
                    oversized: 1,
                    deaf: 1,
                    mute: 1,
                },
                slow_exits: 100,
                deaf_to_cancel: 100,
                mute: 50,
                stubborn: 100,
                ..calm.script
            },
            shutdowns: 300,
            ..calm
        }
    }
}

/// The calm worker's limits: room for three runs of up to three
/// repositories, with charters, events and outcomes of a few hundred bytes.
pub const LIMITS: Limits = Limits {
    host: host::Limits {
        slots: 3,
        repositories: 3,
        name_bytes: 32,
        charter_bytes: 512,
        snapshot_bytes: 64,
        outcome_bytes: 64,
        detail_bytes: 32,
        held: 2,
        event_bytes: 96,
        run_calls: 2,
        facts: 256,
    },
    checkout: worker::checkout::Limits {
        workspaces: 4,
        repositories: 3,
        name_bytes: 32,
        message_bytes: 128,
        remote_timeout: Duration::from_secs(60),
        local_timeout: Duration::from_secs(10),
        facts: 256,
    },
    agent: agent::Limits {
        agents: 3,
        charter_bytes: 512,
        snapshot_bytes: 64,
        event_bytes: 96,
        events: 2,
        calls: 2,
        call_bytes: 64,
        answer_bytes: 128,
        fact_bytes: 32,
        outcome_bytes: 64,
        detail_bytes: 32,
        spawn_timeout: Duration::from_secs(1),
        no_progress: Duration::from_secs(10),
        long_span: Duration::from_secs(120),
        wall_time: Duration::from_secs(900),
        grace: Duration::from_secs(5),
        kill_after: Duration::from_secs(2),
        facts: 256,
    },
    grace: Duration::from_secs(60),
    redial: Duration::from_secs(1),
    redial_max: Duration::from_secs(8),
    told: 16,
    stalled: 8,
};

/// The calm engine: six items over a minute, in three workstreams over four
/// repositories, which it neither cancels nor overbooks, and every one of
/// whose starting points the forge has.
fn engine() -> Config {
    Config {
        items: 6,
        window: Duration::from_secs(60),
        workers: 1,
        workstreams: Box::new([
            b"parser".as_slice().into(),
            b"lexer".as_slice().into(),
            b"docs".as_slice().into(),
            b"site".as_slice().into(),
            b"tools".as_slice().into(),
            b"build".as_slice().into(),
        ]),
        repositories: Box::new([
            origin(b"temper", b"ai/temper"),
            origin(b"docs", b"ai/docs"),
            origin(b"site", b"ai/site"),
            origin(b"tools", b"ai/tools"),
        ]),
        spread_min: 1,
        spread_max: 3,
        commits: 0,
        branches: 0,
        writable: 800,
        saves: 500,
        invalid: 0,
        brief_min: 32,
        brief_max: 96,
        turns_min: 4,
        turns_max: 8,
        tokens_min: 1_000,
        tokens_max: 2_000,
        time_min: Duration::from_secs(60),
        time_max: Duration::from_secs(120),
        max_tokens: 1024,
        changes: 800,
        checks: 0,
        verdicts: 200,
        agents: 0,
        attempts: 4,
        transient: 1000,
        permanent: 0,
        backoff_min: Duration::from_secs(1),
        backoff_max: Duration::from_secs(5),
        wakes: 2,
        wake_min: Duration::from_secs(5),
        wake_max: Duration::from_secs(20),
        resumes: 500,
        overbook: 0,
        inbound: 2,
        inbound_min: Duration::from_secs(1),
        inbound_max: Duration::from_secs(20),
        event_min: 16,
        event_max: 48,
        resends: 0,
        cancels: 0,
        late_cancels: 0,
        stale: 0,
        cancel_min: Duration::from_secs(1),
        cancel_max: Duration::from_secs(30),
        calls: 16,
        relay_min: Duration::from_millis(50),
        relay_max: Duration::from_secs(2),
        relay_errors: 0,
        answer_min: 8,
        answer_max: 64,
        grace: Duration::from_secs(120),
        keeps: 1000,
    }
}

/// Calm git: no faults, operations well within their deadlines.
const GIT: Git = Git {
    local: Span::millis(1, 50),
    remote: Span::millis(10, 500),
    network: Span::millis(1, 20),
    stalls: 0,
    stall: Span::millis(5_000, 120_000),
    broken: 0,
    ambiguous: 0,
    unreachable: 0,
    refusing: 0,
    cancels_lost: 0,
    advance: 0,
    advance_after: Span::millis(0, 10_000),
    trunks: 0,
    branched: 0,
};

/// Calm process trees: no failures, no children.
const TREE: tree::Script = tree::Script {
    spawn: Span::millis(10, 200),
    unspawned: 0,
    pipe: Span::millis(1, 20),
    children: 0,
    lingering: 0,
    holding: 0,
    stubborn: 0,
    child_life: Span::millis(1_000, 30_000),
    term: Span::millis(10, 500),
    detail: 64,
};

/// Calm agents: they work, call, push, wait for events, and end, park or fail
/// as they say, then exit in good time.
const SCRIPT: script::Script = script::Script {
    steps: 8,
    step: Span::millis(100, 3_000),
    calls: 300,
    longs: 100,
    waits: 150,
    pushes: 400,
    blocking: 300,
    call_deadline: Span::millis(5_000, 20_000),
    long: Span::millis(5_000, 60_000),
    idle: Span::millis(5_000, 30_000),
    fates: Fates {
        ended: 4,
        parked: 1,
        failed: 1,
        crash: 0,
        hang: 0,
        overrun: 0,
        garbage: 0,
        duplicate: 0,
        trailing: 0,
        oversized: 0,
        deaf: 0,
        mute: 0,
    },
    exit: Span::millis(10, 500),
    slow_exits: 0,
    slow_exit: Span::millis(6_000, 20_000),
    deaf_to_cancel: 0,
    mute: 0,
    wind: Span::millis(100, 2_000),
    stubborn: 0,
    term: Span::millis(10, 500),
};

fn origin(name: &[u8], remote: &[u8]) -> Origin {
    Origin { name: name.into(), remote: remote.into() }
}

/// What the world counted.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct Stats {
    /// Answers the worker made, by kind; sent, taken by the engine, and lost
    /// in flight with their channel; listed held in a hello; and given up.
    pub answers: BTreeMap<&'static str, u32>,
    pub answers_sent: u32,
    pub answers_taken: u32,
    pub answers_lost: u32,
    pub held: u32,
    /// Answers sent again, the engine's acknowledgement not heard, and the
    /// acknowledgements the worker heard.
    pub resent: u32,
    pub acknowledgements: u32,
    /// Answers a worker shutting down kept past its grace, out of reach, and
    /// delivered once the channel opened again.
    pub kept_past_grace: u32,
    pub abandoned: u64,
    /// Dials, those that opened a channel and those that failed; channels
    /// dropped; hellos; and what the engine sent that was lost in flight, and
    /// the relays, bounces and facts the worker sent that were.
    pub dials: u32,
    pub connects: u32,
    pub failed_dials: u32,
    pub drops: u32,
    pub hellos: u32,
    pub lost_down: u32,
    pub lost_up: u32,
    /// The longest the worker went without a channel while it hosted runs.
    pub longest_outage: Option<Duration>,
    /// Agents spawned, started (from a snapshot among them, and in
    /// repositories started from saved work), and edits they made.
    pub spawns: u32,
    pub starts: u32,
    pub resumed: u32,
    pub from_saved: u32,
    pub edits: u32,
    /// What the agents asked of the host and heard back.
    pub push_calls: u32,
    pub relay_calls: u32,
    pub withdraws: u32,
    pub relays: u32,
    pub relayed: u32,
    pub pushed: BTreeMap<&'static str, u32>,
    pub unavailable: u32,
    pub withdrawn: u32,
    pub busy: u32,
    pub bounces: u32,
    pub events: u32,
    pub cancels: u32,
    /// Git: operations, and how they went.
    pub ops: u32,
    pub op_timeouts: u32,
    pub op_broken: u32,
    pub ambiguous: u32,
    pub op_cancels: u32,
    pub cancels_lost: u32,
    pub unreachable: u32,
    pub refusals: u32,
    pub rejected: u32,
    pub advanced: u32,
    /// Repositories that landed a change, and saved work, each checked
    /// against what the agent left.
    pub landed: u32,
    pub saved: u32,
    /// The facts drained, and those dropped; the run's facts sent to the
    /// engine, and those the worker dropped.
    pub facts: u64,
    pub facts_lost: u64,
    pub told: u32,
    pub told_lost: u64,
    /// The worker was told to shut down, and was done.
    pub shutdown: bool,
    pub done: bool,
    /// The most runs hosted at once.
    pub peak: u32,
}

enum Delivery {
    /// An event for the worker: an io terminal, or the channel lost.
    Worker(Event),
    /// A dial ends.
    Dialled {
        epoch: u64,
    },
    /// What the worker sent up the channel `epoch`, and what the engine sent
    /// down it.
    Up {
        epoch: u64,
        event: engine::Event,
    },
    Down {
        epoch: u64,
        event: Event,
    },
    /// An event for the engine off the channel: the channel lost.
    Engine(engine::Event),
    /// The channel `epoch` drops.
    Drop {
        epoch: u64,
    },
    Tree(tree::Due),
    /// A git operation ends.
    Ran {
        owner: Token,
    },
    /// Another party moves a branch.
    Advance {
        remote: Vec<u8>,
        branch: Vec<u8>,
    },
    Shutdown,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Lane {
    Reads,
    Exits,
    Up,
    Down,
}

/// An attempt the worker was assigned.
#[derive(Debug)]
struct Attempt {
    run: Token,
    at: Time,
    repositories: Vec<Repository>,
    snapshot: Option<Box<[u8]>>,
    agent: Option<Token>,
    /// Its answer, as the worker first sent it.
    answer: Option<String>,
}

#[derive(Debug)]
struct Repository {
    name: Vec<u8>,
    remote: Vec<u8>,
    /// The saved-work branch it starts from, if it does.
    saved: Option<Vec<u8>>,
    /// Its push branch, if it may be written.
    push: Option<Vec<u8>>,
}

/// An agent the worker spawned.
#[derive(Debug)]
struct Agent {
    /// The worker's name for it.
    owner: Token,
    workspace: Token,
    attempt: Option<Token>,
    spawned: bool,
    unspawned: bool,
    /// Its push calls not answered yet.
    pushes: BTreeSet<u64>,
}

/// A workspace on io's disk, by io's name for it.
#[derive(Debug, Default)]
struct Space {
    /// The hold that ran an operation in it last.
    hold: Option<Token>,
    /// The agent spawned in it last, until its run has answered.
    agent: Option<Token>,
    /// Each repository's commit when it was last checked out.
    checked: BTreeMap<Vec<u8>, u64>,
    /// What each repository's agent left: its tree as checked out, then as
    /// each edit left it; and as it was when it asked to push.
    left: BTreeMap<Vec<u8>, Files>,
    asked: BTreeMap<Vec<u8>, Files>,
}

pub struct World {
    now: Time,
    /// Draws the latencies, the faults and the agents' edits.
    rng: Rng,
    settings: Settings,

    worker: Model,
    stage: Stage<Limits, Event, Request>,
    /// The worker was told to shut down; it was done, and its shell stopped.
    shut: bool,
    done: bool,

    engine: engine::Model,
    desk: Stage<Config, engine::Event, engine::Request>,

    tree: Tree,
    forge: Forge,
    disk: Checkout,
    /// What the engine's one commit hash stands for.
    commit: Commit,

    wire: Schedule<Delivery>,
    lanes: [Time; 4],
    channel: Channel,
    epochs: u64,
    drops: u32,
    /// Until when every dial fails.
    outage: Time,
    /// The worker holds the channel open, from its own point of view: told
    /// it connected, and not yet that it was lost; since when it has not.
    up: bool,
    down_since: Option<Time>,
    /// When the worker was last out of reach past its grace.
    graced: Option<Time>,
    /// The answers the worker has given up so far.
    abandoned: u64,

    /// Inbound events framed for each attempt.
    places: BTreeMap<Token, u64>,
    attempts: BTreeMap<Token, Attempt>,
    /// Attempts the worker was given whose answers it has not heard the
    /// engine acknowledge (a refusal's aside, which it forgets at once), and
    /// those whose answers it gave up.
    open: BTreeSet<Token>,
    given_up: BTreeSet<Token>,
    /// Attempts whose answers reached the engine.
    reached: BTreeSet<Token>,
    /// Attempts a hello listed as answered, whose answers follow it.
    following: BTreeSet<Token>,
    /// Attempts the engine's cancel reached the worker for, by the place among
    /// the worker's requests of the first one made after it; and those whose
    /// agents have heard a cancel since. The stop sends the cancel down behind
    /// what waits for the agent, a relayed answer the run had before among it:
    /// none may follow.
    cancelled: BTreeMap<Token, u64>,
    stopped: BTreeSet<Token>,
    /// The worker's requests routed so far.
    routed: u64,
    agents: BTreeMap<Token, Agent>,
    spaces: BTreeMap<Token, Space>,
    ops: Ledger<Token, Pending>,
    /// Holds whose runs have answered: they touch nothing again.
    closed: BTreeSet<Token>,
    /// The saved-work branches the engine named, and where the last save
    /// that landed put each, by remote and branch.
    save_branches: BTreeSet<Vec<u8>>,
    saves: BTreeMap<(Vec<u8>, Vec<u8>), u64>,
    edits: u64,

    stats: Stats,
    trace: Trace,
}

impl World {
    #[must_use]
    pub fn new(settings: Settings) -> World {
        let limits = settings.worker;
        assert!(worker::worst_case(&limits).is_some(), "the shell refuses limits it cannot provision");
        let mut rng = Rng::new(settings.seed);
        let engine = engine::Model::new(&settings.engine, rng.next_u64());
        let worker = Model::new(&limits, rng.next_u64());
        let sizes = Sizes {
            call: limits.agent.call_bytes,
            fact: limits.agent.fact_bytes,
            outcome: limits.agent.outcome_bytes,
            snapshot: limits.agent.snapshot_bytes,
            long: limits.agent.long_span,
        };
        let tree = Tree::new(settings.tree, settings.script, sizes, rng.next_u64());
        let mut draws = Rng::new(rng.next_u64());
        let (forge, commit) = seed_forge(&mut draws, &settings);
        let max_out = worker::max_out(&limits);
        let mut world = World {
            now: Time::ZERO,
            rng: draws,
            worker,
            stage: Stage::new(limits, max_out, max_out + SPARE),
            shut: false,
            done: false,
            engine,
            desk: Stage::new(settings.engine.clone(), engine::MAX_OUT, engine::MAX_OUT + SPARE),
            tree,
            forge,
            disk: Checkout::new(),
            commit: io::commit(commit),
            wire: Schedule::new(),
            lanes: [Time::ZERO; 4],
            channel: Channel::Idle,
            epochs: 0,
            drops: settings.network.drops,
            outage: Time::ZERO,
            up: false,
            down_since: None,
            graced: None,
            abandoned: 0,
            places: BTreeMap::new(),
            attempts: BTreeMap::new(),
            open: BTreeSet::new(),
            given_up: BTreeSet::new(),
            reached: BTreeSet::new(),
            following: BTreeSet::new(),
            cancelled: BTreeMap::new(),
            stopped: BTreeSet::new(),
            routed: 0,
            agents: BTreeMap::new(),
            spaces: BTreeMap::new(),
            ops: Ledger::new("git operation"),
            closed: BTreeSet::new(),
            save_branches: BTreeSet::new(),
            saves: BTreeMap::new(),
            edits: 0,
            stats: Stats::default(),
            trace: Trace::default(),
            settings,
        };
        if world.rng.chance(world.settings.shutdowns) {
            let at = Time::ZERO.saturating_add(world.settings.shutdown_at.draw(&mut world.rng));
            world.wire.send(at, Delivery::Shutdown);
        }
        world
    }

    #[must_use]
    pub fn now(&self) -> Time {
        self.now
    }

    #[must_use]
    pub fn stats(&self) -> Stats {
        Stats {
            abandoned: self.worker.abandoned(),
            facts_lost: self.worker.facts_lost(),
            told_lost: self.worker.told_lost(),
            ..self.stats.clone()
        }
    }

    /// What the fake engine counted.
    #[must_use]
    pub fn tally(&self) -> engine::Tally {
        self.engine.tally()
    }

    /// What the process trees counted.
    #[must_use]
    pub fn tree(&self) -> tree::Tally {
        self.tree.tally()
    }

    /// What crossed between the worker and the world, in order, with times.
    #[must_use]
    pub fn trace(&self) -> &[String] {
        self.trace.lines()
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

    /// One iteration of the loop: what is due is delivered, then each model's
    /// stage runs as its shell would run it.
    fn iterate(&mut self) {
        self.stage.tick(self.now);
        self.desk.tick(self.now);
        self.deliver();
        if let Some(since) = self.down_since
            && self.now >= since.saturating_add(self.settings.worker.grace)
        {
            self.graced = Some(self.now);
        }
        if !self.done {
            self.run_worker();
        }
        self.run_engine();
    }

    // The worker's stage.

    fn run_worker(&mut self) {
        // Its ready list first, then its events, then its alarms, while it
        // has room for what one more may produce.
        while self.stage.has_room() && self.worker.is_ready() {
            self.trace.log(self.now, "worker resume");
            worker::resume(&mut self.worker, &self.stage.env, &mut self.stage.out);
        }
        while let Some(event) = self.stage.next_event() {
            self.trace.log(self.now, format!("worker <- {event:?}"));
            self.take(&event);
            worker::step(&mut self.worker, &self.stage.env, event, &mut self.stage.out);
        }
        while self.stage.has_room() && self.worker.is_due(self.now) {
            self.trace.log(self.now, "worker alarm");
            worker::fire(&mut self.worker, &self.stage.env, &mut self.stage.out);
        }
        // The facts, drained as the shell would write them out; the run's,
        // sent to the engine best effort.
        while self.worker.pop_fact().is_some() {
            self.stats.facts += 1;
        }
        while let Some(told) = self.worker.pop_told() {
            self.stats.told += 1;
            self.send_up(translate::told(&told, WORKER));
        }
        // What the steps asked for, submitted at the end of the iteration.
        while let Some(request) = self.stage.out.pop() {
            self.trace.log(self.now, format!("worker -> {request:?}"));
            self.route(request);
            self.routed += 1;
        }
        let following: Vec<&Token> = self.following.iter().collect();
        assert!(following.is_empty(), "the answers a hello lists as held follow it: {following:?} did not");
        self.worker.reclaim();
        let hosted = self.worker.host().hosted();
        assert!(hosted <= self.settings.worker.host.slots, "runs stay within their slots");
        self.stats.peak = self.stats.peak.max(hosted);
        let abandoned = self.worker.abandoned();
        if abandoned > self.abandoned {
            assert!(self.shut && !self.up, "only a worker shutting down out of reach gives answers up");
            assert!(self.graced.is_some(), "past its grace");
            assert_eq!(self.worker.host().unanswered(), 0, "with no run left, whose answer could go with them");
            self.abandoned = abandoned;
        }
        if self.shut && self.worker.is_done() {
            self.stop();
        }
    }

    /// Notes what the worker takes as `event`, before it takes it.
    fn take(&mut self, event: &Event) {
        match event {
            Event::Connected => {
                if let Some(since) = self.down_since.take()
                    && !self.open.is_empty()
                {
                    let outage = self.now.saturating_since(since);
                    self.stats.longest_outage =
                        Some(self.stats.longest_outage.map_or(outage, |longest| longest.max(outage)));
                }
                self.up = true;
            }
            Event::Lost => {
                if self.up {
                    self.up = false;
                    self.down_since = Some(self.now);
                }
            }
            Event::Shutdown => self.shut = true,
            Event::Assign { assignment } => {
                let attempt = assignment.attempt;
                if let Some(branch) = &assignment.save {
                    self.save_branches.insert(branch.to_vec());
                }
                let repositories = assignment.workspace.repositories.iter().map(|repository| {
                    let saved = match &repository.start {
                        host::Start::Saved { branch } => Some(branch.to_vec()),
                        host::Start::Base { .. } | host::Start::Branch { .. } | host::Start::Commit { .. } => None,
                    };
                    let push = match &repository.access {
                        host::Access::Writable { push } => Some(push.to_vec()),
                        host::Access::ReadOnly => None,
                    };
                    Repository { name: repository.name.to_vec(), remote: repository.remote.to_vec(), saved, push }
                });
                let record = Attempt {
                    run: assignment.run,
                    at: self.now,
                    repositories: repositories.collect(),
                    snapshot: assignment.snapshot.clone(),
                    agent: None,
                    answer: None,
                };
                assert!(self.attempts.insert(attempt, record).is_none(), "the engine assigns an attempt once");
                self.open.insert(attempt);
            }
            Event::Acknowledged { attempt, .. } => {
                // The worker forgets the answer: no hello lists it again.
                if self.attempts.get(attempt).is_some_and(|record| record.answer.is_some()) {
                    self.open.remove(attempt);
                }
                self.stats.acknowledgements += 1;
            }
            Event::Cancel { attempt, .. } => {
                let live = self.attempts.get(attempt).is_some_and(|record| record.answer.is_none());
                if live && !self.cancelled.contains_key(attempt) {
                    let first = self.routed + u64::from(self.stage.out.len());
                    self.cancelled.insert(*attempt, first);
                }
            }
            Event::Inbound { .. }
            | Event::Relayed { .. }
            | Event::Spawned { .. }
            | Event::Unspawned { .. }
            | Event::Sent { .. }
            | Event::Unsent { .. }
            | Event::Received { .. }
            | Event::Malformed { .. }
            | Event::Hangup { .. }
            | Event::Signalled { .. }
            | Event::Exited { .. }
            | Event::Reaped { .. }
            | Event::Done { .. } => {}
        }
    }

    /// One of the worker's requests, to the world that carries it out.
    fn route(&mut self, request: Request) {
        match request {
            Request::Dial => self.dial(),
            Request::Hello { hello } => {
                self.hello(&hello);
                self.send_up(engine::Event::Hello { worker: WORKER, hello: translate::hello(hello) });
            }
            Request::Answer { run, attempt, answer } => {
                self.answered(run, attempt, &answer);
                let answer = translate::answer(answer);
                self.send_up(engine::Event::Answered { worker: WORKER, run, attempt, answer });
            }
            Request::Relay { run, attempt, call, body } => {
                assert!(self.up, "a relay goes on a channel open");
                self.stats.relays += 1;
                self.send_up(engine::Event::Relay { worker: WORKER, run, attempt, call, body });
            }
            Request::Bounced { run, attempt, bounce } => {
                assert!(self.up, "a bounce goes on a channel open");
                self.stats.bounces += 1;
                let bounce = translate::bounce(bounce);
                self.send_up(engine::Event::Bounced { worker: WORKER, run, attempt, bounce });
            }
            Request::Spawn { owner, workspace, deadline } => {
                self.spawn(owner, workspace);
                self.tree_take(agent::Request::Spawn { owner, workspace, deadline });
            }
            Request::Send { owner, process, message } => {
                self.down(owner, &message);
                self.tree_take(agent::Request::Send { owner, process, message });
            }
            Request::Read { owner, process } => self.tree_take(agent::Request::Read { owner, process }),
            Request::Signal { owner, process, signal } => {
                self.tree_take(agent::Request::Signal { owner, process, signal });
            }
            Request::Wait { owner, process } => self.tree_take(agent::Request::Wait { owner, process }),
            Request::Reap { owner, process } => self.tree_take(agent::Request::Reap { owner, process }),
            Request::Io { owner, op, deadline } => self.start_op(owner, op, deadline),
            Request::CancelIo { owner } => self.cancel_op(owner),
        }
    }

    /// The worker's hello: on a channel it holds open, listing exactly the
    /// runs it was given and has not answered, those whose answers it holds
    /// followed by them.
    fn hello(&mut self, hello: &Hello) {
        assert!(self.up, "a hello goes on a channel open");
        let slots = if self.shut { 0 } else { self.settings.worker.host.slots };
        assert_eq!(hello.slots, slots, "the hello says the worker's slots, none once it is shutting down");
        let listed: BTreeSet<Token> = hello.hosting.iter().map(|hosted| hosted.attempt).collect();
        assert_eq!(listed.len(), hello.hosting.len(), "a hello lists each run once");
        // Of the runs it was given and has not answered, it lists all but those
        // whose answers it gave up, shutting down out of reach past its grace.
        let unlisted: Vec<Token> = self.open.difference(&listed).copied().collect();
        assert!(listed.is_subset(&self.open), "a hello lists only runs the worker was given and has not answered");
        let given_up = u64::try_from(self.given_up.len() + unlisted.len()).expect("fits");
        assert_eq!(
            given_up,
            self.worker.abandoned(),
            "a hello lists every run the worker has not answered or given up"
        );
        for attempt in unlisted {
            self.open.remove(&attempt);
            self.given_up.insert(attempt);
        }
        for hosted in &hello.hosting {
            let record = self.attempts.get(&hosted.attempt).expect("a run listed was assigned");
            assert_eq!(record.run, hosted.run, "a run is listed by its names");
            match hosted.phase {
                Phase::Answered => {
                    self.following.insert(hosted.attempt);
                    self.stats.held += 1;
                    if self.shut && self.graced.is_some() {
                        self.stats.kept_past_grace += 1;
                    }
                }
                Phase::Preparing | Phase::Starting | Phase::Active | Phase::Waiting | Phase::Ending => {}
            }
        }
        self.stats.hellos += 1;
    }

    /// The worker's answer for the run `run`'s attempt `attempt`: once, on a
    /// channel it holds open, for an attempt it was given, once its agent has
    /// gone and nothing of git runs for it; and cancelled only by whoever
    /// may cancel it.
    fn answered(&mut self, run: Token, attempt: Token, answer: &host::Answer) {
        assert!(self.up, "an answer goes on a channel open");
        let record = self.attempts.get_mut(&attempt).expect("an answer is for an attempt the worker was given");
        assert_eq!(record.run, run, "an answer names its run");
        assert!(self.open.contains(&attempt), "an answer is for a run neither acknowledged nor given up");
        self.following.remove(&attempt);
        self.stats.answers_sent += 1;
        let said = format!("{answer:?}");
        if let Some(first) = &record.answer {
            // Sent again after a hello, the engine's acknowledgement not heard.
            assert_eq!(*first, said, "an answer sent again is the same answer");
            self.stats.resent += 1;
            return;
        }
        record.answer = Some(said);
        let assigned = record.at;
        let agent = record.agent;
        *self.stats.answers.entry(translate::answer_kind(answer)).or_default() += 1;
        match answer {
            host::Answer::Failed { failure: host::Failure::Cancelled(reason), .. } => match reason {
                host::Reason::Engine => {
                    assert!(
                        self.cancelled.contains_key(&attempt),
                        "a run is cancelled by the engine once it cancelled it"
                    );
                }
                host::Reason::Contact => assert!(
                    self.graced.is_some_and(|at| at >= assigned),
                    "a run is cancelled for contact once the worker was out of reach past its grace"
                ),
                host::Reason::Shutdown => assert!(self.shut, "a run is cancelled for a shutdown once told to"),
            },
            // A refusal goes once, and holds no slot: the worker forgets it.
            host::Answer::Refused(_) => {
                self.open.remove(&attempt);
            }
            host::Answer::Ended { .. } | host::Answer::Parked { .. } | host::Answer::Failed { .. } => {}
        }
        let Some(owner) = agent else {
            return;
        };
        assert!(self.gone(owner), "a run answers once its agent has gone");
        let record = self.agents.remove(&owner).expect("an agent started is known");
        let space = self.spaces.get_mut(&record.workspace).expect("an agent runs in a workspace");
        if space.agent == Some(owner) {
            space.agent = None;
        }
        if let Some(hold) = space.hold {
            assert!(!self.ops.contains(hold), "a run answers once nothing of git runs in its workspace");
            self.closed.insert(hold);
        }
    }

    fn spawn(&mut self, owner: Token, workspace: Token) {
        assert!(self.disk.exists(&io::dir(workspace)), "an agent is spawned in a workspace io has");
        let busy = self.busy(workspace);
        assert!(!busy, "an agent is spawned in a workspace nothing of git runs in");
        let space = self.spaces.get_mut(&workspace).expect("an agent is spawned in a workspace prepared");
        if let Some(before) = space.agent {
            let record = self.agents.get(&before).expect("an agent is known until its run answers");
            assert!(record.attempt.is_none() && gone(&self.tree, record), "one agent at a time in a workspace");
        }
        space.agent = Some(owner);
        let agent =
            Agent { owner, workspace, attempt: None, spawned: false, unspawned: false, pushes: BTreeSet::new() };
        assert!(self.agents.insert(owner, agent).is_none(), "an agent is spawned once");
        self.stats.spawns += 1;
    }

    /// What the worker sends down to its agent `owner`.
    fn down(&mut self, owner: Token, message: &Down) {
        for bytes in translate::down_bytes(message) {
            let leaks = bytes.windows(IDENTITY.len()).any(|window| window == IDENTITY);
            assert!(!leaks, "nothing an agent hears holds the forge identity");
        }
        match message {
            Down::Start { charter, snapshot } => {
                self.start(owner, translate::charter_attempt(charter), snapshot.as_deref());
            }
            Down::Event { event } => {
                let agent = self.agents.get(&owner).expect("an event goes to an agent spawned");
                assert_eq!(
                    agent.attempt,
                    Some(translate::event_attempt(event)),
                    "an inbound event reaches its attempt's agent"
                );
                self.stats.events += 1;
            }
            Down::Answer { call, reply } => {
                let agent = self.agents.get_mut(&owner).expect("an answer goes to an agent spawned");
                agent.pushes.remove(&call.raw());
                let attempt = agent.attempt.expect("an agent calls once started");
                match reply {
                    Reply::Relayed { .. } => {
                        let stopped = self.stopped.contains(&attempt);
                        assert!(!stopped, "a cancelled run's relayed calls are answered unavailable");
                        self.stats.relayed += 1;
                    }
                    Reply::Pushed(push) => *self.stats.pushed.entry(push_kind(*push)).or_default() += 1,
                    Reply::Unavailable => self.stats.unavailable += 1,
                    Reply::Busy => self.stats.busy += 1,
                    Reply::Withdrawn => self.stats.withdrawn += 1,
                    Reply::TooLarge => {}
                }
            }
            Down::Cancel => {
                let agent = self.agents.get(&owner).expect("a cancel goes to an agent spawned");
                if let Some(attempt) = agent.attempt
                    && self.cancelled.get(&attempt).is_some_and(|first| self.routed >= *first)
                {
                    self.stopped.insert(attempt);
                }
                self.stats.cancels += 1;
            }
        }
    }

    /// The agent `owner` starts for `attempt`: one not answered, with its
    /// snapshot, in the tree each repository was checked out at.
    fn start(&mut self, owner: Token, attempt: Token, snapshot: Option<&[u8]>) {
        let agent = self.agents.get_mut(&owner).expect("a start goes to an agent spawned");
        assert_eq!(agent.attempt, None, "an agent starts once");
        agent.attempt = Some(attempt);
        let workspace = agent.workspace;
        let record = self.attempts.get_mut(&attempt).expect("an agent starts for an attempt the worker was given");
        assert!(record.answer.is_none(), "no agent starts for a run answered");
        assert_eq!(record.agent, None, "an attempt starts one agent");
        record.agent = Some(owner);
        assert_eq!(snapshot, record.snapshot.as_deref(), "an agent starts from its attempt's snapshot");
        self.stats.starts += 1;
        if snapshot.is_some() {
            self.stats.resumed += 1;
        }
        let space = self.spaces.get(&workspace).expect("an agent runs in a workspace prepared");
        for repository in &record.repositories {
            let commit =
                *space.checked.get(&repository.name).expect("every repository is checked out before its run starts");
            let files = files(&self.disk, workspace, &repository.name);
            assert_eq!(
                files,
                self.forge.object(commit).tree,
                "an agent starts in the tree its repository was checked out at"
            );
            if let Some(branch) = &repository.saved {
                let key = (repository.remote.clone(), branch.clone());
                let saved = self.saves.get(&key).expect("a run starts from saved work only where a save landed");
                assert_eq!(commit, *saved, "a run started from saved work starts from the last save that landed");
                self.stats.from_saved += 1;
            }
        }
        // Another party may move a push branch while the run works.
        let pushes: Vec<(Vec<u8>, Vec<u8>)> = record
            .repositories
            .iter()
            .filter_map(|repository| Some((repository.remote.clone(), repository.push.clone()?)))
            .collect();
        if !pushes.is_empty() && self.rng.chance(self.settings.git.advance) {
            let index = usize::try_from(self.rng.below(u64::try_from(pushes.len()).expect("fits"))).expect("fits");
            let (remote, branch) = pushes[index].clone();
            let at = self.now.saturating_add(self.settings.git.advance_after.draw(&mut self.rng));
            self.wire.send(at, Delivery::Advance { remote, branch });
        }
    }

    fn tree_take(&mut self, request: agent::Request) {
        let outs = self.tree.take(self.now, request);
        self.tree_outs(outs);
    }

    fn tree_outs(&mut self, outs: Vec<tree::Out>) {
        for out in outs {
            match out {
                tree::Out::Model { after, event } => {
                    let lane = match &event {
                        agent::Event::Received { .. }
                        | agent::Event::Malformed { .. }
                        | agent::Event::Hangup { .. } => Some(Lane::Reads),
                        agent::Event::Exited { .. } | agent::Event::Reaped { .. } => Some(Lane::Exits),
                        agent::Event::Spawned { owner, .. } => {
                            self.agents.get_mut(owner).expect("a spawn is the world's").spawned = true;
                            None
                        }
                        agent::Event::Unspawned { owner, .. } => {
                            self.agents.get_mut(owner).expect("a spawn is the world's").unspawned = true;
                            None
                        }
                        agent::Event::Sent { .. } | agent::Event::Unsent { .. } | agent::Event::Signalled { .. } => {
                            None
                        }
                        agent::Event::Spawn { .. }
                        | agent::Event::Deliver { .. }
                        | agent::Event::Answer { .. }
                        | agent::Event::Stop { .. } => unreachable!("io ends requests"),
                    };
                    let at = match lane {
                        Some(lane) => self.lane(lane, after),
                        None => {
                            self.now.saturating_add(after).saturating_add(self.settings.network.hop.draw(&mut self.rng))
                        }
                    };
                    self.wire.send(at, Delivery::Worker(translate::from_agent_io(event)));
                }
                tree::Out::Due { after, due } => {
                    self.wire.send(self.now.saturating_add(after), Delivery::Tree(due));
                }
                tree::Out::Wrote { owner, said } => self.wrote(owner, &said),
            }
        }
    }

    /// The agent `owner` wrote `said`: before it asks to push, and now and
    /// then as it tells a fact, it edits its working trees, while no push of
    /// its own is under way.
    fn wrote(&mut self, owner: Token, said: &Said) {
        match said {
            Said::Call { name, push: true, .. } => {
                self.stats.push_calls += 1;
                if self.may_edit(owner) && self.rng.chance(self.settings.edits) {
                    self.edit(owner);
                }
                let agent = self.agents.get_mut(&owner).expect("an agent that writes was spawned");
                agent.pushes.insert(*name);
                let workspace = agent.workspace;
                let attempt = agent.attempt.expect("an agent writes once started");
                let record = self.attempts.get(&attempt).expect("an attempt started");
                let space = self.spaces.get_mut(&workspace).expect("an agent runs in a workspace prepared");
                for repository in &record.repositories {
                    space.asked.insert(repository.name.clone(), files(&self.disk, workspace, &repository.name));
                }
            }
            Said::Call { push: false, .. } => self.stats.relay_calls += 1,
            Said::Fact { .. } => {
                if self.may_edit(owner) && self.rng.chance(self.settings.scribbles) {
                    self.edit(owner);
                }
            }
            Said::Withdraw { .. } => self.stats.withdraws += 1,
            Said::Long { .. }
            | Said::LongDone
            | Said::Waiting { .. }
            | Said::Ended { .. }
            | Said::Parked { .. }
            | Said::Failed { .. }
            | Said::Garbage => {}
        }
    }

    fn may_edit(&self, owner: Token) -> bool {
        let agent = self.agents.get(&owner).expect("an agent that writes was spawned");
        agent.attempt.is_some() && agent.pushes.is_empty() && !self.busy(agent.workspace)
    }

    /// The agent `owner` writes a file in one of its repositories.
    fn edit(&mut self, owner: Token) {
        let agent = self.agents.get(&owner).expect("an agent that writes was spawned");
        let workspace = agent.workspace;
        let record = self.attempts.get(&agent.attempt.expect("started")).expect("an attempt started");
        let count = u64::try_from(record.repositories.len()).expect("fits");
        let index = usize::try_from(self.rng.below(count)).expect("fits");
        let name = record.repositories[index].name.clone();
        self.edits += 1;
        let at = Place { workspace, repository: name.clone().into_boxed_slice() };
        let file =
            if self.rng.chance(500) { b"README".to_vec() } else { format!("notes/{}", self.edits % 4).into_bytes() };
        let path = [io::path(&at).as_slice(), b"/", &file].concat();
        self.disk.write(&path, format!("edit {}", self.edits).as_bytes());
        let left = files(&self.disk, workspace, &name);
        self.spaces.get_mut(&workspace).expect("an agent runs in a workspace prepared").left.insert(name, left);
        self.stats.edits += 1;
    }

    /// Whether the agent `owner` has gone: it could not be spawned, or its
    /// process tree is empty.
    fn gone(&self, owner: Token) -> bool {
        gone(&self.tree, self.agents.get(&owner).expect("an agent is known until its run answers"))
    }

    // The engine's stage.

    fn run_engine(&mut self) {
        while self.desk.has_room() && self.engine.is_ready() {
            self.trace.log(self.now, "engine resume");
            engine::resume(&mut self.engine, &self.desk.env, &mut self.desk.out);
        }
        while let Some(event) = self.desk.next_event() {
            self.trace.log(self.now, format!("engine <- {event:?}"));
            engine::step(&mut self.engine, &self.desk.env, event, &mut self.desk.out);
        }
        while self.desk.has_room() && self.engine.is_due(self.now) {
            self.trace.log(self.now, "engine alarm");
            engine::fire(&mut self.engine, &self.desk.env, &mut self.desk.out);
        }
        while let Some(request) = self.desk.out.pop() {
            self.trace.log(self.now, format!("engine -> {request:?}"));
            self.send_down(request);
        }
        self.engine.reclaim();
    }

    // The wire.

    /// Hands over what is due now.
    fn deliver(&mut self) {
        while let Some(delivery) = self.wire.next(self.now) {
            match delivery {
                Delivery::Worker(event) => {
                    if !self.done {
                        self.stage.push(event);
                    }
                }
                Delivery::Dialled { epoch } => self.dialled(epoch),
                Delivery::Up { epoch, event } => self.arrived_up(epoch, event),
                Delivery::Down { epoch, event } => match self.channel {
                    Channel::Open { epoch: open, .. } if open == epoch => self.stage.push(event),
                    Channel::Open { .. } | Channel::Idle | Channel::Dialling { .. } | Channel::Shut => {
                        self.stats.lost_down += 1;
                    }
                },
                Delivery::Engine(event) => self.desk.push(event),
                Delivery::Drop { epoch } => self.drop_channel(epoch),
                Delivery::Tree(due) => {
                    let outs = self.tree.due(self.now, due);
                    self.tree_outs(outs);
                }
                Delivery::Ran { owner } => self.ran(owner),
                Delivery::Advance { remote, branch } => self.advance(&remote, &branch),
                Delivery::Shutdown => {
                    if !self.done {
                        self.stats.shutdown = true;
                        self.stage.push(Event::Shutdown);
                    }
                }
            }
        }
    }

    /// When something sent `after` from now on `lane` arrives: a hop later,
    /// and after what was sent on it before.
    fn lane(&mut self, lane: Lane, after: Duration) -> Time {
        let index = match lane {
            Lane::Reads => 0,
            Lane::Exits => 1,
            Lane::Up => 2,
            Lane::Down => 3,
        };
        let at = self.now.saturating_add(after).saturating_add(self.settings.network.hop.draw(&mut self.rng));
        let at = at.max(self.lanes[index]);
        self.lanes[index] = at;
        at
    }

    fn has_work_now(&self) -> bool {
        let worker = !self.done && (self.stage.has_events() || self.worker.is_due(self.now) || self.worker.is_ready());
        let engine = self.desk.has_events() || self.engine.is_due(self.now) || self.engine.is_ready();
        worker || engine || self.wire.is_due(self.now)
    }

    fn next_time(&self) -> Option<Time> {
        let worker = if self.done { None } else { self.worker.next_deadline() };
        [self.wire.next_time(), worker, self.engine.next_deadline()].into_iter().flatten().min()
    }

    /// Checks the invariants of a world with nothing left to happen.
    fn assert_settled(&self) {
        assert!(self.wire.is_empty(), "nothing is in flight");
        assert!(!self.desk.has_events(), "the engine has taken everything");
        assert!(self.done || !self.stage.has_events(), "the worker has taken everything");
        // The engine.
        assert_eq!(self.engine.outstanding(), 0, "the engine has no attempt out");
        assert_eq!(self.engine.next_deadline(), None, "the engine has nothing left due");
        if !self.done {
            assert_eq!(self.engine.items(), 0, "every item closed");
        }
        // The worker.
        let host = self.worker.host();
        assert_eq!(host.hosted(), 0, "every slot is free");
        assert_eq!(host.calls(), 0, "no host call is open");
        assert_eq!(self.worker.checkout().holds(), 0, "no workspace is held");
        assert_eq!(self.worker.workspaces(), 0, "every workspace asked for was released");
        assert_eq!(self.worker.agent().agents(), 0, "no agent is left");
        assert_eq!(self.worker.agent().next_deadline(), None, "no agent's alarm is armed");
        assert_eq!(self.worker.held(), 0, "no answer is held");
        if self.done {
            assert!(self.worker.is_done(), "a worker stopped was done");
        } else {
            assert_eq!(self.worker.next_deadline(), None, "no alarm is armed");
        }
        assert!(self.shut || self.worker.abandoned() == 0, "only a worker shutting down gives answers up");
        // Its neighbours.
        self.tree.assert_settled();
        self.ops.assert_settled();
        let open = u64::try_from(self.open.len() + self.given_up.len()).expect("fits");
        assert_eq!(
            open,
            self.worker.abandoned(),
            "every run the worker was given was answered, or its answer given up"
        );
        for (owner, agent) in &self.agents {
            assert!(gone(&self.tree, agent), "agent {owner:?} has gone");
        }
        // The engine takes each attempt's answer once, however often it came.
        let tally = self.engine.tally();
        let taken = tally.busy + tally.invalid + tally.ended + tally.parked + tally.failed + tally.late;
        assert_eq!(taken + tally.duplicates, self.stats.answers_taken, "the engine took every answer that reached it");
        let reached = u32::try_from(self.reached.len()).expect("fits");
        assert_eq!(taken, reached, "the engine took one answer for each attempt answered");
        for (attempt, record) in &self.attempts {
            // A refusal goes once: lost in flight, its attempt is presumed lost.
            let refused = record.answer.as_ref().is_some_and(|answer| answer.starts_with("Refused"));
            assert!(
                self.reached.contains(attempt)
                    || self.given_up.contains(attempt)
                    || self.open.contains(attempt)
                    || refused,
                "the answer for {attempt:?} reached the engine, unless the worker gave it up: {:?}",
                record.answer
            );
        }
        assert_eq!(
            self.stats.answers_sent,
            self.stats.answers_taken + self.stats.answers_lost,
            "every answer sent reached the engine, or was lost in flight with its channel"
        );
    }
}

/// Whether `agent` has gone: it could not be spawned, or its process tree is
/// empty.
fn gone(tree: &Tree, agent: &Agent) -> bool {
    agent.unspawned || (agent.spawned && tree.is_gone(agent.owner))
}

fn push_kind(push: agent::channel::Push) -> &'static str {
    match push {
        agent::channel::Push::Done => "done",
        agent::channel::Push::Moved => "moved",
        agent::channel::Push::Failed => "failed",
        agent::channel::Push::Nothing => "nothing",
    }
}
