use std::collections::{BTreeMap, BTreeSet};

use skein_lib::{Duration, Env, Queue, Rng, Time, Token, Wall};
use temper_fake_checkout::Checkout;
use temper_fake_checkout::git::Tree as Files;
use temper_fake_forge_domain::api::{Checks, Cue, File, Permission, Protection, Setup};
use temper_fake_forge_domain::{self as forge, Skew};
use temper_legacy_engine_domain::{self as engine, Item};
use temper_legacy_engine_domain_world::deployment::{self, CI, CUE, ELSEWHERE, ENGINE, GREEN, LABELS, MAIN, PEOPLE};
use temper_legacy_engine_domain_world::mirror::Mirror;
use temper_legacy_engine_domain_world::people::{self, Asker, People, Story};
use temper_legacy_engine_domain_world::referee::{self as stories, Bounds};
use temper_legacy_engine_domain_world::store::{self, Store};
use temper_legacy_engine_domain_world::translate::Asked;
use temper_worker_agent_world::script::{self, Fates, Sizes};
use temper_worker_agent_world::tree::{self, Tree};
use temper_worker_domain::{self as worker, Domain, Event, Limits, Request, agent, host};
use temper_world::{Key, Ledger, Referee, Schedule, Span, Stage, Trace};

use crate::protocol::{Names, Places};
use crate::referee::{self, Hosting};

mod engine_side;
mod git;
mod network;
mod story;
mod worker_side;

pub use git::Git;
pub use network::Network;

use git::{OTHER, Pending};
use network::Channel;
use story::Story as Content;

/// Room in each domain's output queue beyond what one step may emit. Small, so
/// the loop's flow control is exercised.
const SPARE: u32 = 2;

/// The world's own bounds: lines of its trace, and deliveries scheduled at
/// once. A world past one fails with its seed, rather than grow.
const TRACE: usize = 100_000;
const DELIVERIES: u32 = 20_000;

/// The store's bound on the traces it keeps.
const TRACES: usize = 100_000;

/// Within which the engine acknowledges an answer that reached it, the
/// channel it came on open.
const ACKNOWLEDGED: Duration = Duration::from_secs(10 * 60);

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Settings {
    /// Seeds the world, which seeds the engine, the worker, the process trees
    /// and the forge.
    pub seed: u64,
    pub worker: Limits,
    pub engine: engine::Limits,
    pub forge: forge::Config,
    /// What people do, and between their looks at the forge.
    pub stories: Vec<Story>,
    pub people: Span,
    /// The chance, per mille, that a person stops a run they see assigned,
    /// a drawn `stop_after` later, if they have not stopped its item's runs
    /// before; a session they stopped they wake with a message once it is
    /// released.
    pub stops: u32,
    pub stop_after: Span,
    pub store: store::Script,
    /// The forge protocol layer's deadline for the engine's call.
    pub timeout: Duration,
    pub bounds: Bounds,
    pub network: Network,
    pub git: Git,
    pub tree: tree::Script,
    pub script: script::Script,
    /// The chance, per mille, that an agent edits its working trees before it
    /// asks to push, and as it tells a fact: only while no push of its own is
    /// under way, so that what it asked to push is what lands.
    pub edits: u32,
    pub scribbles: u32,
    /// The chance, per mille, that an agent ends with the outcome its script
    /// wrote, which no engine reads, so that its run fails. (An agent fated
    /// to write garbage, whose run fails so, relays the calls its script
    /// wrote, which no engine reads either.)
    pub garbled: u32,
    /// Whether the changes sessions ask for land into [`RELEASE`], which the
    /// forge does not have until a change's first checkout creates it from
    /// the default branch, rather than into the default branch.
    pub release: bool,
    /// The chance, per mille, that the shell tells the worker to shut down, at
    /// a moment drawn from `shutdown_at`; once it is done, a new worker starts
    /// a drawn `comeback` later, with the limits `upgrade` if it says some.
    pub shutdowns: u32,
    pub shutdown_at: Span,
    pub comeback: Span,
    pub upgrade: Option<Limits>,
    /// How often the engine restarts, cold, each at a moment drawn from
    /// `restart_at`; and how often at most it restarts as soon as it has
    /// posted a run's outcome, while it applies it, each outcome posted with
    /// the chance `restart_applying`, per mille.
    pub restarts: u32,
    pub restart_at: Span,
    pub applying_restarts: u32,
    pub restart_applying: u32,
}

impl Settings {
    /// A world where nothing goes wrong: people with every story but the
    /// plans' (which run on their own); a forge that answers in time and
    /// whose webhooks come; a channel that never drops; git that never
    /// fails; agents that work, call, push what they edited, wait for
    /// events, park and end, and exit in good time.
    #[must_use]
    pub fn calm(seed: u64) -> Settings {
        Settings {
            seed,
            worker: LIMITS,
            engine: ENGINE_LIMITS,
            forge: FORGE,
            stories: people::SWEPT.to_vec(),
            people: Span::millis(1_000, 10_000),
            stops: 0,
            stop_after: Span::millis(1_000, 30_000),
            store: store::Script { latency: Span::millis(1, 50), failures: 0, done_anyway: 0 },
            timeout: Duration::from_secs(10),
            bounds: Bounds { story: Duration::from_secs(24 * 3_600), message: Duration::from_secs(4 * 3_600) },
            network: Network {
                hop: Span::millis(1, 20),
                dial: Span::millis(10, 100),
                drops: 0,
                drop: 0,
                life: Span::millis(10_000, 60_000),
                outage: Span::millis(1_000, 10_000),
                duplicates: 0,
                stalls: 0,
                stall: Span::millis(1_000, 20_000),
            },
            git: GIT,
            tree: TREE,
            script: SCRIPT,
            edits: 800,
            scribbles: 200,
            garbled: 0,
            release: false,
            shutdowns: 0,
            shutdown_at: Span::millis(10_000, 120_000),
            comeback: Span::millis(1_000, 30_000),
            upgrade: None,
            restarts: 0,
            restart_at: Span::millis(10_000, 600_000),
            applying_restarts: 0,
            restart_applying: 0,
        }
    }

    /// A world where everything that can go wrong does, now and then: a
    /// forge late and losing webhooks; a channel that drops, for less and
    /// more than the worker's grace, duplicates frames and stalls; git that
    /// fails, refuses and finds branches moved or deleted; process trees
    /// that fail to spawn and leave children; agents that misbehave every
    /// way the script knows; people who stop runs; an engine whose grace for
    /// a lost worker is shorter than the worker's in some worlds, longer in
    /// others, and which retries a failed run as often as the engine's world
    /// in some; and, in some worlds, a shutdown, and the engine restarting
    /// once or twice. A world for the random sweep.
    #[must_use]
    pub fn rough(seed: u64) -> Settings {
        let calm = Settings::calm(seed);
        let mut rng = Rng::new(seed ^ 0x5eed);
        // The engine's grace for a lost worker on either side of the
        // worker's own, the deployment's default among them; and its retries
        // the engine world's in some worlds, more in the others.
        let graces = [
            deployment::LIMITS.fleet.grace,
            Duration::from_secs(45),
            Duration::from_secs(70),
            Duration::from_secs(120),
        ];
        let grace = graces[usize::try_from(rng.below(4)).expect("few")];
        let retries = if rng.chance(300) { deployment::LIMITS.work.retries } else { calm.engine.work.retries };
        let engine = engine::Limits {
            work: engine::work::Limits { retries, ..calm.engine.work },
            fleet: engine::fleet::Limits { grace, ..calm.engine.fleet },
            ..calm.engine
        };
        Settings {
            engine,
            worker: Limits {
                agent: agent::Limits { wall_time: Duration::from_secs(300), ..calm.worker.agent },
                ..calm.worker
            },
            forge: forge::Config { late: 30, hooks_late: 200, hooks_lost: 300, ..calm.forge },
            stops: 100,
            network: Network {
                hop: Span::millis(1, 300),
                drops: 3,
                drop: 700,
                outage: Span::millis(1_000, 150_000),
                duplicates: 100,
                stalls: 20,
                ..calm.network
            },
            git: Git {
                stalls: 20,
                broken: 20,
                ambiguous: 20,
                unreachable: 30,
                refusing: 30,
                refusing_creates: 300,
                cancels_lost: 200,
                advance: 100,
                deletes: 50,
                ..calm.git
            },
            tree: tree::Script { unspawned: 30, children: 2, lingering: 200, holding: 200, stubborn: 200, ..calm.tree },
            script: script::Script {
                fates: Fates {
                    ended: 16,
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
            garbled: 20,
            release: rng.chance(500),
            restarts: if rng.chance(300) { 1 + u32::try_from(rng.below(2)).expect("few") } else { 0 },
            applying_restarts: 1,
            restart_applying: 30,
            shutdowns: 300,
            ..calm
        }
    }

    /// The calm world with only `stories`.
    #[must_use]
    pub fn only(seed: u64, stories: &[Story]) -> Settings {
        Settings { stories: stories.to_vec(), ..Settings::calm(seed) }
    }
}

/// The calm worker's limits: room for three runs of one repository each,
/// with the engine's charters, and events and outcomes of a few hundred
/// bytes.
pub const LIMITS: Limits = Limits {
    host: host::Limits {
        accounts: 4,
        slots: 3,
        repositories: 2,
        name_bytes: 32,
        charter_bytes: 8_192,
        snapshot_bytes: 64,
        outcome_bytes: 4_096,
        detail_bytes: 32,
        held: 2,
        event_bytes: 96,
        run_calls: 2,
        facts: 256,
    },
    checkout: worker::checkout::Limits {
        workspaces: 4,
        repositories: 2,
        name_bytes: 32,
        message_bytes: 512,
        conflicts: 0,
        path_bytes: 0,
        remote_timeout: Duration::from_secs(60),
        local_timeout: Duration::from_secs(10),
        facts: 256,
    },
    agent: agent::Limits {
        accounts: 4,
        repositories: 8,
        name_bytes: 256,
        agents: 3,
        charter_bytes: 8_192,
        snapshot_bytes: 64,
        event_bytes: 96,
        events: 2,
        calls: 2,
        call_bytes: 512,
        answer_bytes: 128,
        fact_bytes: 32,
        outcome_bytes: 4_096,
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

/// A failed run's retries: more than the engine's world allows, as its
/// agents fail far more often here.
const RETRY: engine::work::Retry =
    engine::work::Retry { retries: 6, base: Duration::from_secs(1), max: Duration::from_secs(8) };

/// The engine's limits: the engine world's, with more retries, and a grace
/// for a lost worker past the worker's own, so that the engine does not
/// place a run's next attempt while the worker may still host the last.
pub const ENGINE_LIMITS: engine::Limits = engine::Limits {
    work: engine::work::Limits {
        retries: engine::work::Retries {
            transient: RETRY,
            permanent: RETRY,
            run: RETRY,
            agent: RETRY,
            lost: RETRY,
            invalid: RETRY,
        },
        ..deployment::LIMITS.work
    },
    fleet: engine::fleet::Limits { grace: Duration::from_secs(70), ..deployment::LIMITS.fleet },
    ..deployment::LIMITS
};

/// The calm forge: room for the stories, answers in time, webhooks
/// delivered.
const FORGE: forge::Config = forge::Config {
    limits: forge::Limits {
        repositories: 3,
        users: 16,
        labels: 8,
        items: 64,
        comments: 128,
        reviews: 16,
        dependencies: 8,
        branches: 32,
        commits: 4_096,
        files: 16,
        statuses: 128,
        contexts: 2,
        pages: 16,
        name_bytes: 48,
        title_bytes: 64,
        body_bytes: 16_384,
        content_bytes: 256,
        page_size: 64,
        calls: 64,
        hooks: 64,
        observations: 4_096,
    },
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
    ci: CI,
    hook_min: Duration::from_millis(50),
    hook_max: Duration::from_millis(500),
    hooks_late: 0,
    hooks_lost: 0,
    resolution: Duration::from_secs(1),
    skew: Skew::None,
    status_updates: false,
    edit_updates: false,
};

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
    refusing_creates: 0,
    cancels_lost: 0,
    advance: 0,
    advance_after: Span::millis(0, 10_000),
    deletes: 0,
    moves: u32::MAX,
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
    calls: 400,
    longs: 100,
    waits: 150,
    pushes: 500,
    blocking: 300,
    call_deadline: Span::millis(5_000, 20_000),
    long: Span::millis(5_000, 60_000),
    idle: Span::millis(5_000, 30_000),
    fates: Fates {
        ended: 8,
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

/// What the world counted, by name, that the sweep must reach on the
/// engine's side.
/// Known v1 recovery gap: seed12 cold restart reuses a named inbound for the
/// same attempt with a different body. Neither a namespace nor Waiting crosses
/// worker-to-engine v1; the agent ledger finding preserves its consequence.
pub const FINDINGS: [u64; 1] = [12];

pub const ENDINGS: [&str; 14] = [
    "acknowledged",
    "assigned",
    "cancelled",
    "inbound",
    "merged",
    "relayed",
    "released",
    "restarted",
    "reviewed",
    "stopped",
    "story closed",
    "undecodable",
    "undecoded",
    "woken",
];

/// What the world counted.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct Stats {
    /// Answers the worker made, by kind; sent, taken by the engine, and lost
    /// in flight with their channel; copies of them the network made that
    /// reached the engine; listed held in a hello; and given up.
    pub answers: BTreeMap<&'static str, u32>,
    pub answers_sent: u32,
    pub answers_taken: u32,
    pub answers_lost: u32,
    pub answers_copied: u32,
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
    /// dropped; hellos; frames sent again, and channels that stalled; and
    /// what the engine sent that was lost in flight, and the relays,
    /// bounces and facts the worker sent that were, and the copies that
    /// were.
    pub dials: u32,
    pub connects: u32,
    pub failed_dials: u32,
    pub drops: u32,
    pub hellos: u32,
    pub duplicated: u32,
    pub stalled: u32,
    pub lost_down: u32,
    pub lost_up: u32,
    pub copies_lost: u32,
    /// Copies that reached the worker once the attempt they were for had
    /// answered, by what they were; and cancels sent again on a new channel,
    /// behind the hello that listed their attempt.
    pub late: BTreeMap<&'static str, u32>,
    pub recancels: u32,
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
    pub deleted: u32,
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
    /// The engine restarted.
    pub restarts: u32,
    /// Deferred namespace finding: one attempt received a reused name with different bytes.
    pub reused_inbound_names: u32,
    /// The worker was told to shut down, and was done; and how often a new
    /// one started after.
    pub shutdown: bool,
    pub done: bool,
    pub comebacks: u32,
    /// The most runs hosted at once.
    pub peak: u32,
    /// What the engine's side came to, by name; the engine's facts, and
    /// those it dropped.
    pub endings: BTreeMap<&'static str, u32>,
    pub engine_facts: u64,
    pub engine_facts_lost: u64,
    pub people: people::Tally,
    pub store: store::Tally,
}

/// Something on its way, delivered at its time.
#[derive(Debug)]
enum Delivery {
    /// An event for the worker of life `.0`: an io terminal, or the channel
    /// lost.
    Worker(u64, Event),
    /// A dial ends.
    Dialled {
        epoch: u64,
    },
    /// What the worker sent up the channel `epoch`, as the engine's protocol
    /// layer hands it on, or a copy the network made of it; and what the
    /// engine sent down it, as the worker's does.
    Up {
        epoch: u64,
        event: engine::Event,
        copy: bool,
    },
    Down {
        epoch: u64,
        event: Event,
        copy: bool,
    },
    /// An event for the engine of life `life` off the channel: the channel
    /// lost, a store's terminal, a watcher's delivery.
    Engine {
        life: u64,
        event: engine::Event,
    },
    /// Something of the process trees of the worker of life `.0` falls due.
    Tree(u64, tree::Due),
    /// A git operation ends.
    Ran {
        owner: Token,
    },
    /// Another party moves a branch, or deletes one.
    Advance {
        remote: Vec<u8>,
        branch: Vec<u8>,
    },
    Delete {
        remote: Vec<u8>,
        branch: Vec<u8>,
    },
    /// A new worker starts.
    Comeback,
    /// The protocol layer's deadline for the engine's forge call it names.
    Deadline(u64),
    /// People look at the forge.
    People,
    /// A person stops the item's run.
    Stop(Item),
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
    at: Time,
    /// Referee input, separate from the unchanged production charter bytes.
    charter: Box<[u8]>,
    repositories: Vec<Repository>,
    snapshot: Option<Box<[u8]>>,
    agent: Option<Token>,
    /// Its answer, as the worker first sent it.
    answer: Option<String>,
    refused: bool,
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

/// What of an agent's words are left as its script wrote them.
#[derive(Clone, Copy, Debug, Default)]
struct Garbled {
    calls: bool,
    outcome: bool,
}

/// An agent the worker spawned.
#[derive(Debug)]
struct Agent {
    /// The worker's name for it.
    owner: Token,
    workspace: Token,
    attempt: Option<Names>,
    spawned: bool,
    unspawned: bool,
    /// Its push calls not answered yet.
    pushes: BTreeSet<u64>,
    /// What its words say, once it started; whether its relayed calls are
    /// left as its script wrote them, and its outcome.
    content: Option<Content>,
    garbled: Garbled,
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

/// An engine call the forge protocol layer has out.
#[derive(Debug)]
struct Out {
    /// The life of the engine that made it.
    life: u64,
    call: Token,
    asked: Asked,
    deadline: Key,
    expired: bool,
}

/// A call people have out on the forge.
#[derive(Debug)]
enum Theirs {
    Person { tale: Option<usize> },
    Review { repository: usize, number: u64, head: u64 },
}

/// Who asked the engine through its web.
#[derive(Clone, PartialEq, Eq, Debug)]
enum Asking {
    /// A story's person, or the caretaker; the item a message was for, its
    /// key and its text; and the item an acceptance was for.
    People(Asker, Option<(Item, Vec<u8>, Vec<u8>)>, Option<Item>),
    /// A person stopping the item's run; and waking the item, a session
    /// they stopped that was released since, with a message: its key and
    /// its text.
    Stopper(Item),
    Waker(Item, Vec<u8>, Vec<u8>),
}

pub struct World {
    now: Time,
    /// Draws the latencies, the faults and the agents' edits; and, apart,
    /// the copies of frames the network sends again.
    rng: Rng,
    copies: Rng,
    settings: Settings,

    // The worker, its neighbours, and what the world checks of it.
    worker: Domain,
    stage: Stage<Limits, Event, Request>,
    /// The worker was told to shut down; it was done, and its shell stopped.
    shut: bool,
    done: bool,
    /// The workers started so far: a new one after each that shut down.
    lives: u64,
    tree: Tree,
    disk: Checkout,
    /// git's calls to the forge so far.
    git_calls: u64,
    attempts: BTreeMap<Names, Attempt>,
    /// Attempts the worker was given whose answers it has not heard the
    /// engine acknowledge (a refusal's aside, which it forgets at once), and
    /// those whose answers it gave up.
    open: BTreeSet<Names>,
    given_up: BTreeSet<Names>,
    /// Attempts whose answers reached the engine.
    reached: BTreeSet<Names>,
    /// Attempts a hello listed as answered, whose answers follow it.
    following: BTreeSet<Names>,
    /// Attempts the engine's cancel reached the worker for, by the place among
    /// the worker's requests of the first one made after it; and those whose
    /// agents have heard a cancel since. The stop sends the cancel down behind
    /// what waits for the agent, a relayed answer the run had before among it:
    /// none may follow.
    cancelled: BTreeMap<Names, u64>,
    stopped: BTreeSet<Names>,
    /// The worker's requests routed so far.
    routed: u64,
    /// Relay delivery waits, retained until exactly one lower terminal wins.
    relay_waits: BTreeMap<Token, (Names, bool)>,
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
    /// How often another party moved each branch, by remote and branch.
    moved: BTreeMap<(Vec<u8>, Vec<u8>), u32>,

    // The channel between them, the copies of frames held back, and the
    // acknowledgements sent on each channel.
    wire: Schedule<Delivery>,
    held: Vec<network::Held>,
    acknowledged: BTreeSet<(u64, Names)>,
    /// The attempts the engine cancelled.
    engine_cancels: BTreeSet<Names>,
    scheduled: u32,
    lanes: [Time; 4],
    channel: Channel,
    epochs: u64,
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
    /// The protocol layers' state: inbound events framed for each attempt,
    /// and the repositories each attempt was assigned, by the deployment's
    /// index.
    places: Places,
    assigned: BTreeMap<Names, Vec<u32>>,
    inbound_names: BTreeMap<(Names, Token), Vec<Box<[u8]>>>,

    // The engine and its neighbours: the engines started so far, a new one
    // after each restart.
    engine_life: u64,
    engine: engine::Domain,
    desk: Stage<engine::Limits, engine::Event, engine::Request>,
    forge: forge::Domain,
    forge_out: Queue<forge::Request>,
    calls: Ledger<u64, Out>,
    owned: Ledger<Token, ()>,
    theirs: Ledger<u64, Theirs>,
    asks: Ledger<u64, Asking>,
    stores: Ledger<Token, ()>,
    people: People,
    store: Store,
    mirror: Mirror,
    /// The items whose runs a person stopped, or will, once each; the
    /// sessions they stopped, which they wake with a message once released;
    /// and those released, to wake until the engine takes the message, and
    /// those with a message on its way.
    stopping: BTreeSet<Item>,
    stopped_sessions: BTreeSet<Item>,
    waking: BTreeSet<Item>,
    wakers: BTreeSet<Item>,
    wakes: u64,
    /// The first assignment of each attempt the engine made.
    first: BTreeSet<(Item, u64)>,

    stories: Referee<stories::Engine>,
    hosting: Referee<Hosting>,
    stats: Stats,
    trace: Trace,
}

impl World {
    #[must_use]
    pub fn new(settings: Settings) -> World {
        let limits = settings.worker;
        for limits in [Some(limits), settings.upgrade].into_iter().flatten() {
            assert!(worker::worst_case(&limits).is_some(), "the shell refuses limits it cannot provision");
        }
        assert!(engine::worst_case(&settings.engine).is_some(), "the shell refuses limits it cannot provision");
        let mut rng = Rng::new(settings.seed);
        let mut forge = forge::Domain::new(&settings.forge, rng.next_u64());
        for name in deployment::REPOSITORIES.iter().chain([&ELSEWHERE]) {
            setup(&mut forge, &settings.forge, name);
        }
        let engine = engine::Domain::new(config(), &settings.engine, rng.next_u64(), Time::ZERO);
        let worker = Domain::new(&limits, rng.next_u64());
        let tree = Tree::new(settings.tree, settings.script, sizes(&limits), rng.next_u64());
        let max_out = worker::max_out(&limits);
        let desk_out = engine::max_out(&settings.engine);
        // How long each channel that drops lives.
        let lives = (0..settings.network.drops).map(|_| settings.network.life.draw(&mut rng)).collect();
        let mut world = World {
            now: Time::ZERO,
            rng: Rng::new(rng.next_u64()),
            copies: Rng::new(rng.next_u64()),
            worker,
            stage: Stage::new(limits, max_out, max_out + SPARE),
            shut: false,
            done: false,
            lives: 0,
            tree,
            disk: Checkout::new(),
            git_calls: 0,
            attempts: BTreeMap::new(),
            open: BTreeSet::new(),
            given_up: BTreeSet::new(),
            reached: BTreeSet::new(),
            following: BTreeSet::new(),
            cancelled: BTreeMap::new(),
            stopped: BTreeSet::new(),
            routed: 0,
            relay_waits: BTreeMap::new(),
            agents: BTreeMap::new(),
            spaces: BTreeMap::new(),
            ops: Ledger::new("git operation"),
            closed: BTreeSet::new(),
            save_branches: BTreeSet::new(),
            saves: BTreeMap::new(),
            edits: 0,
            moved: BTreeMap::new(),
            wire: Schedule::new(),
            held: Vec::new(),
            acknowledged: BTreeSet::new(),
            engine_cancels: BTreeSet::new(),
            scheduled: 0,
            lanes: [Time::ZERO; 4],
            channel: Channel::Idle,
            epochs: 0,
            outage: Time::ZERO,
            up: false,
            down_since: None,
            graced: None,
            abandoned: 0,
            places: Places::new(),
            assigned: BTreeMap::new(),
            inbound_names: BTreeMap::new(),
            engine_life: 0,
            engine,
            desk: Stage::new(settings.engine, desk_out, desk_out + SPARE),
            forge,
            forge_out: Queue::with_capacity(256),
            calls: Ledger::new("engine call"),
            owned: Ledger::new("engine's forge call"),
            theirs: Ledger::new("person's call"),
            asks: Ledger::new("person's ask"),
            stores: Ledger::new("store operation"),
            people: People::new(&settings.stories),
            store: Store::new(settings.store, rng.next_u64(), TRACES),
            mirror: Mirror::default(),
            stopping: BTreeSet::new(),
            stopped_sessions: BTreeSet::new(),
            waking: BTreeSet::new(),
            wakers: BTreeSet::new(),
            wakes: 0,
            first: BTreeSet::new(),
            stories: Referee::new(stories::Engine::new(settings.bounds, settings.stories.len())),
            hosting: Referee::new(Hosting::new(ACKNOWLEDGED, lives, settings.applying_restarts)),
            stats: Stats::default(),
            trace: Trace::default(),
            settings,
        };
        // Every story's end is expected within its bound, from the start:
        // a story that does not end fails at its bound rather than at the
        // world's caps.
        world.stories.observe(Time::ZERO, stories::Seen::Start, &mut Vec::new());
        world.send(Time::ZERO, Delivery::People);
        if world.rng.chance(world.settings.shutdowns) {
            let at = Time::ZERO.saturating_add(world.settings.shutdown_at.draw(&mut world.rng));
            world.hosting.inject(at, referee::Stimulus::Shutdown);
        }
        for _ in 0..world.settings.restarts {
            let at = Time::ZERO.saturating_add(world.settings.restart_at.draw(&mut world.rng));
            world.hosting.inject(at, referee::Stimulus::Restart);
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
            abandoned: self.stats.abandoned + self.worker.abandoned(),
            facts_lost: self.stats.facts_lost + self.worker.facts_lost(),
            told_lost: self.stats.told_lost + self.worker.told_lost(),
            engine_facts_lost: self.engine.facts_lost(),
            people: self.people.tally(),
            store: self.store.tally(),
            ..self.stats.clone()
        }
    }

    /// What the process trees counted.
    #[must_use]
    pub fn tree(&self) -> tree::Tally {
        self.tree.tally()
    }

    /// The forge as observed.
    #[must_use]
    pub fn mirror(&self) -> &Mirror {
        &self.mirror
    }

    /// How many stories the world tells.
    #[must_use]
    pub fn stories(&self) -> usize {
        self.settings.stories.len()
    }

    /// The item of the story `tale`, once its person knows it.
    #[must_use]
    pub fn item(&self, tale: usize) -> Option<Item> {
        self.people.item(tale)
    }

    /// How many safety checks the referees made, and liveness expectations
    /// they saw met.
    #[must_use]
    pub fn judged(&self) -> (u64, u64) {
        let (checks, met) = self.stories.judged();
        let (more, also) = self.hosting.judged();
        (checks + more, met + also)
    }

    /// What crossed between the worker, the engine and the world, in order,
    /// with times.
    #[must_use]
    pub fn trace(&self) -> &[String] {
        self.trace.lines()
    }

    /// Runs until the stories are done and nothing is in flight, then checks
    /// the invariants of a settled world. Panics, with the seed, if it takes
    /// more than `iterations`.
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
            "seed {}: the world did not settle in {iterations} iterations, at {:?}, waiting for {:?}: {:?} {:?}",
            self.settings.seed,
            self.now,
            self.unquiet(),
            self.stories.verdict(),
            self.hosting.verdict()
        );
    }

    /// Runs until `until`, checking the contracts as it goes, but not the
    /// invariants of a settled world: for a world that is not meant to
    /// settle. Panics, with the seed, if it takes more than `iterations`.
    pub fn run_for(&mut self, until: Duration, iterations: u32) {
        let until = Time::ZERO.saturating_add(until);
        for _ in 0..iterations {
            self.iterate();
            if self.has_work_now() {
                continue;
            }
            let next = self.next_time().expect("the engine polls, so there is always a next time");
            if next > until {
                self.assert_holding();
                return;
            }
            self.now = next;
        }
        panic!("seed {}: the world did not reach {until:?} in {iterations} iterations", self.settings.seed);
    }

    /// Checks what holds of a world at any moment: the referees hold.
    fn assert_holding(&self) {
        self.stories.assert_holding(self.settings.seed);
        self.hosting.assert_holding(self.settings.seed);
    }

    /// One iteration of the loop: what is due is delivered, then each domain's
    /// stage runs as its shell would run it.
    fn iterate(&mut self) {
        let now = self.now;
        self.stage.tick(now);
        self.desk.tick(now);
        while let Some(delivery) = self.wire.next(now) {
            self.scheduled -= 1;
            self.deliver(delivery);
        }
        self.drain_forge();
        let env = Env { now, wall: Wall::EPOCH, limits: self.settings.forge };
        while self.forge.is_due(now) {
            forge::fire(&mut self.forge, &env, &mut self.forge_out);
            self.drain_forge();
        }
        self.referee_fire();
        if let Some(since) = self.down_since
            && self.now >= since.saturating_add(self.stage.env.limits.grace)
        {
            self.graced = Some(self.now);
        }
        if !self.done {
            self.run_worker();
        }
        self.run_engine();
        self.observe_forge();
        self.forge.reclaim();
    }

    fn referee_fire(&mut self) {
        if self.stories.is_due(self.now) {
            let mut stimuli = Vec::new();
            self.stories.fire(self.now, &mut stimuli);
            self.stories.assert_holding(self.settings.seed);
            assert!(stimuli.is_empty(), "nothing is injected into the stories");
        }
        if self.hosting.is_due(self.now) {
            let mut stimuli = Vec::new();
            self.hosting.fire(self.now, &mut stimuli);
            self.hosting.assert_holding(self.settings.seed);
            for stimulus in stimuli {
                self.inject(stimulus);
            }
        }
    }

    /// What the hosting referee injects.
    pub(super) fn inject(&mut self, stimulus: referee::Stimulus) {
        match stimulus {
            referee::Stimulus::Drop { epoch } => self.drop_channel(epoch),
            referee::Stimulus::Shutdown => {
                if !self.done {
                    self.stats.shutdown = true;
                    self.stage.push(Event::Shutdown);
                }
            }
            referee::Stimulus::Restart => self.restart(),
        }
    }

    /// Hands over what is due now.
    fn deliver(&mut self, delivery: Delivery) {
        match delivery {
            Delivery::Worker(life, event) => {
                if !self.done && life == self.lives {
                    self.push_worker_event(event);
                }
            }
            Delivery::Dialled { epoch } => self.dialled(epoch),
            Delivery::Up { epoch, event, copy } => self.arrived_up(epoch, event, copy),
            Delivery::Down { epoch, event, copy } => self.arrived_down(epoch, event, copy),
            Delivery::Engine { life, event } => {
                if life == self.engine_life {
                    self.desk.push(event);
                }
            }
            Delivery::Tree(life, due) => {
                if life != self.lives {
                    return;
                }
                let outs = self.tree.due(self.now, due);
                self.tree_outs(outs);
            }
            Delivery::Ran { owner } => self.ran(owner),
            Delivery::Advance { remote, branch } => self.advance(&remote, &branch),
            Delivery::Delete { remote, branch } => self.delete(&remote, &branch),
            Delivery::Comeback => self.come_back(),
            Delivery::Deadline(name) => self.deadline(name),
            Delivery::People => self.look(),
            Delivery::Stop(item) => self.stop_run(item),
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

    fn send(&mut self, at: Time, delivery: Delivery) -> Key {
        self.scheduled += 1;
        assert!(
            self.scheduled <= DELIVERIES,
            "seed {}: more deliveries scheduled than the world holds",
            self.settings.seed
        );
        self.wire.send(at, delivery)
    }

    fn withdraw(&mut self, key: Key) -> Option<Delivery> {
        let withdrawn = self.wire.withdraw(key);
        if withdrawn.is_some() {
            self.scheduled -= 1;
        }
        withdrawn
    }

    fn log(&mut self, line: impl std::fmt::Display) {
        assert!(
            self.trace.lines().len() < TRACE,
            "seed {}: the trace grew past its bound, at {:?}, waiting for {:?}",
            self.settings.seed,
            self.now,
            self.unquiet()
        );
        self.trace.log(self.now, line);
    }

    fn end(&mut self, ending: &'static str) {
        *self.stats.endings.entry(ending).or_default() += 1;
    }

    fn has_work_now(&self) -> bool {
        let worker = !self.done && (self.stage.has_events() || self.worker.is_due(self.now) || self.worker.is_ready());
        let engine = self.desk.has_events() || self.engine.is_due(self.now) || self.engine.is_ready();
        worker
            || engine
            || self.wire.is_due(self.now)
            || self.forge.is_due(self.now)
            || self.stories.is_due(self.now)
            || self.hosting.is_due(self.now)
    }

    fn next_time(&self) -> Option<Time> {
        let worker = if self.done { None } else { self.worker.next_deadline() };
        [
            self.wire.next_time(),
            worker,
            self.engine.next_deadline(),
            self.forge.next_deadline(),
            self.stories.next_deadline(),
            self.hosting.next_deadline(),
        ]
        .into_iter()
        .flatten()
        .min()
    }

    /// Whether the world has settled: the stories done, nothing in flight
    /// anywhere, the worker with nothing to do, and the referees expecting
    /// nothing more.
    fn is_quiet(&self) -> bool {
        self.unquiet().is_empty()
    }

    /// What keeps the world from settling.
    fn unquiet(&self) -> Vec<&'static str> {
        let open = |verdict: temper_world::Verdict| match verdict {
            temper_world::Verdict::Open { .. } => true,
            temper_world::Verdict::Passed
            | temper_world::Verdict::Stopped { .. }
            | temper_world::Verdict::Failed(_) => false,
        };
        [
            (!self.people.is_done(&self.mirror), "the stories"),
            (self.done, "the worker stopped"),
            (!self.wire.is_empty(), "deliveries"),
            (self.worker.next_deadline().is_some(), "the worker's alarms"),
            (self.worker.host().hosted() > 0, "runs hosted"),
            (self.worker.held() > 0, "answers held"),
            (!self.calls.is_empty(), "the engine's calls"),
            (!self.relay_waits.is_empty(), "relay terminals"),
            (!self.theirs.is_empty(), "people's calls"),
            (!self.asks.is_empty(), "people's asks"),
            (self.forge.calls() > 0 || self.forge.deliveries() > 0, "the forge"),
            (!self.unsettled().is_empty(), "items unsettled"),
            (open(self.stories.verdict()), "the stories' referee"),
            (open(self.hosting.verdict()), "the hosting referee"),
        ]
        .into_iter()
        .filter_map(|(unquiet, what)| unquiet.then_some(what))
        .collect()
    }

    /// The open items the engine tracks, with a record, that are not held.
    fn unsettled(&self) -> Vec<u64> {
        let mut unsettled = Vec::new();
        for (repository, number, issue) in self.mirror.items() {
            let tracked = issue.labels.iter().any(|label| **label == *deployment::TRACKING);
            if !tracked || !issue.open || deployment::index(repository).is_none() {
                continue;
            }
            let Some(record) = self.mirror.record(repository, number) else { continue };
            match record.lifecycle.phase {
                engine::work::Phase::Held { .. } => {}
                engine::work::Phase::Waiting
                | engine::work::Phase::Parked
                | engine::work::Phase::Retrying(_)
                | engine::work::Phase::Claimed
                | engine::work::Phase::Applying { .. }
                | engine::work::Phase::Done => unsettled.push(number),
            }
        }
        unsettled
    }

    /// Checks the invariants of a world with nothing left to happen.
    fn assert_settled(&mut self) {
        let seed = self.settings.seed;
        self.forget_held();
        assert!(!self.desk.has_events(), "seed {seed}: the engine has taken everything");
        assert!(!self.stage.has_events(), "seed {seed}: the worker has taken everything");
        // The engine's side.
        self.calls.assert_settled();
        self.owned.assert_settled();
        self.theirs.assert_settled();
        self.asks.assert_settled();
        self.stores.assert_settled();
        let tally = self.forge.tally();
        assert_eq!(tally.forgotten, 0, "seed {seed}: the forge kept every call it took: {tally:?}");
        // The worker.
        self.assert_worker_settled();
        assert_eq!(self.worker.next_deadline(), None, "seed {seed}: no alarm is armed");
        // Between them: every answer sent reached the engine or was lost in
        // flight, its copies apart.
        assert_eq!(
            self.stats.answers_sent,
            self.stats.answers_taken + self.stats.answers_lost,
            "seed {seed}: every answer sent reached the engine, or was lost in flight with its channel"
        );
        // The referees' verdicts.
        let mut stimuli = Vec::new();
        self.stories.observe(self.now, stories::Seen::Settled, &mut stimuli);
        self.stories.assert_passed(seed);
        self.hosting.observe(self.now, referee::Seen::Settled, &mut Vec::new());
        self.hosting.assert_passed(seed);
    }

    /// Checks what holds of a worker with nothing left to do: settled, or
    /// done once it shut down.
    fn assert_worker_settled(&self) {
        let seed = self.settings.seed;
        let host = self.worker.host();
        assert_eq!(host.hosted(), 0, "seed {seed}: every slot is free");
        assert_eq!(host.calls(), 0, "seed {seed}: no host call is open");
        assert!(self.relay_waits.is_empty(), "seed {seed}: every relay received one lower terminal");
        assert_eq!(self.worker.checkout().holds(), 0, "seed {seed}: no workspace is held");
        assert_eq!(self.worker.workspaces(), 0, "seed {seed}: every workspace asked for was released");
        assert_eq!(self.worker.agent().agents(), 0, "seed {seed}: no agent is left");
        assert_eq!(self.worker.agent().next_deadline(), None, "seed {seed}: no agent's alarm is armed");
        assert_eq!(self.worker.held(), 0, "seed {seed}: no answer is held");
        assert!(self.shut || self.worker.abandoned() == 0, "seed {seed}: only a worker shutting down gives answers up");
        self.tree.assert_settled();
        self.ops.assert_settled();
        let open = u64::try_from(self.open.len() + self.given_up.len()).expect("fits");
        assert_eq!(
            open,
            self.worker.abandoned(),
            "seed {seed}: every run the worker was given was answered, or its answer given up"
        );
        for (owner, agent) in &self.agents {
            assert!(gone(&self.tree, agent), "seed {seed}: agent {owner:?} has gone");
        }
        for (names, record) in &self.attempts {
            // A refusal goes once: lost in flight, its attempt is placed again
            // or presumed lost.
            assert!(
                self.reached.contains(names)
                    || self.given_up.contains(names)
                    || self.open.contains(names)
                    || record.refused,
                "seed {seed}: the answer for {names:?} reached the engine, unless the worker gave it up: {:?}",
                record.answer
            );
        }
    }
}

/// The sizes the scripted agents write within, but for the outcome, whose
/// content is the run's.
fn sizes(limits: &Limits) -> Sizes {
    Sizes {
        call: limits.agent.call_bytes,
        fact: limits.agent.fact_bytes.saturating_sub(5),
        outcome: 8,
        snapshot: limits.agent.snapshot_bytes,
        long: limits.agent.long_span,
    }
}

/// Whether `agent` has gone: it could not be spawned, or its process tree is
/// empty.
fn gone(tree: &Tree, agent: &Agent) -> bool {
    agent.unspawned || (agent.spawned && tree.is_gone(agent.owner))
}

/// A base changes may land into besides the default branch, which the forge
/// does not have until a run's checkout creates it.
pub const RELEASE: &[u8] = b"release";

/// The deployment's configuration, as the engine's world has it, with
/// [`RELEASE`] a base of every repository after its default branch.
fn config() -> engine::Config {
    let mut config = deployment::config();
    let repo = engine::plan::Repo { bases: Box::new([MAIN.into(), RELEASE.into()]) };
    config.plan.repositories = config.plan.repositories.iter().map(|_| repo.clone()).collect();
    config
}

/// Sets up a repository of the fake forge, as the engine's world does: its
/// default branch holding a file CI reads as green, CI cued by it, its
/// default branch protected, and its users, the worker's and another
/// party's among them.
fn setup(forge: &mut forge::Domain, config: &forge::Config, name: &[u8]) {
    let setup = Setup {
        name: name.into(),
        default: MAIN.into(),
        tree: Box::new([
            File { path: b"README".as_slice().into(), content: b"hello".as_slice().into() },
            File { path: CUE.into(), content: GREEN.into() },
        ]),
        labels: LABELS.iter().map(|label| (*label).into()).collect(),
        checks: Checks {
            contexts: Box::new([b"ci".as_slice().into(), b"lint".as_slice().into()]),
            latency_min: Duration::from_secs(1),
            latency_max: Duration::from_secs(40),
            // The second repository's CI never reports: where changes stall.
            silent: if name == deployment::STALLED { 1_000 } else { 0 },
            passes: 1_000,
            reruns: 0,
            cue: Some(Cue { path: CUE.into(), green: GREEN.into() }),
        },
        protection: Some(Protection {
            branch: MAIN.into(),
            contexts: Box::new([b"ci".as_slice().into()]),
            approvals: 1,
            dismiss_stale: true,
        }),
        hooked: true,
    };
    forge::repository(forge, config, setup);
    forge::grant(forge, name, ENGINE, Permission::Write);
    forge::grant(forge, name, deployment::WORKER, Permission::Write);
    forge::grant(forge, name, CI, Permission::Write);
    forge::grant(forge, name, OTHER, Permission::Write);
    forge::grant(forge, name, PEOPLE[0], Permission::Admin);
    for user in PEOPLE[1..].iter().chain([&deployment::REVIEWER]) {
        forge::grant(forge, name, *user, Permission::Write);
    }
    forge::grant(forge, name, deployment::STRANGER, Permission::Read);
}

#[cfg(test)]
mod relay_terminal_tests {
    use super::{Event, Settings, Token, World};

    #[test]
    fn the_first_relay_terminal_wins_and_late_wire_replies_are_filtered() {
        for cancellation_first in [false, true] {
            let mut world = World::new(Settings::calm(7));
            world.stage.tick(world.now);
            let call = Token::new(123);
            world.relay_waits.insert(call, ((Token::new(1), Token::new(2)), true));
            let reply = Event::Relayed { run: Token::new(1), attempt: Token::new(2), call, answer: Box::new([]) };
            let cancelled = Event::RelayCancelled { call };
            let (first, second) = if cancellation_first { (cancelled, reply) } else { (reply, cancelled) };
            world.push_worker_event(first);
            world.push_worker_event(second);
            world.push_worker_event(Event::Relayed {
                run: Token::new(1),
                attempt: Token::new(2),
                call,
                answer: Box::new([]),
            });
            let terminal = world.stage.next_event().expect("one terminal wins");
            assert_eq!(matches!(terminal, Event::RelayCancelled { .. }), cancellation_first);
            assert!(
                world.stage.next_event().is_none(),
                "late replies and the losing cancellation do not reach the domain"
            );
            assert!(world.relay_waits.is_empty());
        }
    }

    #[test]
    fn a_reply_for_another_attempt_does_not_finish_the_relay_wait() {
        let mut world = World::new(Settings::calm(7));
        world.stage.tick(world.now);
        let call = Token::new(123);
        world.relay_waits.insert(call, ((Token::new(1), Token::new(2)), false));
        world.push_worker_event(Event::Relayed {
            run: Token::new(1),
            attempt: Token::new(3),
            call,
            answer: Box::new([]),
        });
        assert!(world.stage.next_event().is_none());
        assert!(world.relay_waits.contains_key(&call));
        world.push_worker_event(Event::Relayed {
            run: Token::new(1),
            attempt: Token::new(2),
            call,
            answer: Box::new([]),
        });
        assert!(world.stage.next_event().is_some());
        assert!(world.relay_waits.is_empty());
    }
}
