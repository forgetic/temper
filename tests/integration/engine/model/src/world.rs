use std::collections::BTreeMap;

use temper_engine_model::forge::api as engine_api;
use temper_engine_model::{
    self as engine, Answer, Assignment, Event, Fact, Hello, Item, Landed, Limits, Model, Request, Start, Work, plan,
};
use temper_forge_model::api::{self as forge_api, Checks, Cue, File, Git, Permission, Protection, Setup};
use temper_forge_model::{self as forge, Skew};
use temper_lib::{Duration, Env, Queue, ReplyTo, Rng, Time, Token};
use temper_world::{Key, Ledger, Referee, Schedule, Span, Stage, Trace};

use crate::codec;
use crate::deployment::{
    self, CI, CUE, ELSEWHERE, ENGINE, GREEN, LABELS, LIMITS, MAIN, PEOPLE, REPOSITORIES, REVIEWER, STRANGER, WORKER,
};
use crate::mirror::Mirror;
use crate::people::{self, Asker, People, Story};
use crate::referee::{Bounds, Engine, Moment, Needs, Seen, Stimulus};
use crate::store::{self, Store};
use crate::translate::{self, Asked};
use crate::workers::{self, Assigned, Down, Effect, Said, Up, Worker};

/// Room in the engine's output queue beyond what one step may emit.
const SLACK: u32 = 4;

/// The world's own bounds: lines of its trace, deliveries scheduled at once,
/// and traces the store keeps. A world past one fails with its seed, rather
/// than grow.
const TRACE: usize = 400_000;
const DELIVERIES: u32 = 20_000;
const TRACES: usize = 100_000;

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Settings {
    pub seed: u64,
    pub limits: Limits,
    pub forge: forge::Config,
    pub stories: Vec<Story>,
    /// How many workers dial in, and how they behave.
    pub workers: usize,
    pub worker: workers::Script,
    /// A channel's latency, each way; and how long a worker whose channel
    /// dropped waits before it dials again.
    pub channel: Span,
    pub redial: Span,
    /// Between people's looks at the forge.
    pub people: Span,
    pub store: store::Script,
    /// The forge protocol layer's deadline for a call.
    pub timeout: Duration,
    /// Restarts of the engine, and channels dropping, each at a moment
    /// drawn from its span.
    pub restarts: u32,
    pub restart_at: Span,
    pub drops: u32,
    pub drop_at: Span,
    /// Workers whose channel drops and who never come back: their runs and
    /// the answers they kept go with them.
    pub vanishes: u32,
    /// Moments at which the engine restarts besides, each once, as the
    /// forge shows them.
    pub restart_on: Vec<Moment>,
    /// How a session's wake rule batches what wakes it.
    pub batch: plan::Batch,
    /// Whether the protected branches' rules dismiss an approval of an
    /// earlier head: if not, only the engine keeps a stale one from
    /// counting.
    pub dismiss_stale: bool,
    pub bounds: Bounds,
}

impl Settings {
    /// A world where nothing goes wrong: room for everything, answers in
    /// time, webhooks delivered, no restarts, every story.
    #[must_use]
    pub fn calm(seed: u64) -> Settings {
        Settings {
            seed,
            limits: LIMITS,
            forge: forge::Config {
                limits: forge::Limits {
                    repositories: 3,
                    users: 16,
                    labels: 8,
                    items: 64,
                    comments: 64,
                    reviews: 32,
                    dependencies: 8,
                    branches: 32,
                    commits: 256,
                    files: 32,
                    statuses: 64,
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
            },
            stories: people::STORIES.to_vec(),
            workers: 2,
            worker: workers::Script {
                slots: 2,
                pace: Span::millis(200, 3_000),
                grace: Duration::from_secs(30),
                call: Duration::from_secs(60),
            },
            channel: Span::millis(5, 50),
            redial: Span::millis(1_000, 10_000),
            people: Span::millis(1_000, 10_000),
            store: store::Script { latency: Span::millis(1, 50), failures: 0, done_anyway: 0 },
            timeout: Duration::from_secs(10),
            restarts: 0,
            restart_at: Span::millis(5_000, 200_000),
            drops: 0,
            drop_at: Span::millis(5_000, 200_000),
            vanishes: 0,
            restart_on: Vec::new(),
            batch: deployment::session().wake.batch,
            dismiss_stale: true,
            bounds: Bounds { story: Duration::from_secs(4 * 3_600), message: Duration::from_secs(3_600) },
        }
    }

    /// A world where everything that can go wrong does, now and then: the
    /// forge late, failing, limiting and losing webhooks, its clock behind;
    /// the store failing; channels dropping; the engine restarting.
    #[must_use]
    pub fn rough(seed: u64) -> Settings {
        let calm = Settings::calm(seed);
        Settings {
            forge: forge::Config {
                late: 60,
                unavailable: 40,
                timeouts: 40,
                landing: 30,
                rate_limit: 50,
                hooks_late: 200,
                hooks_lost: 300,
                skew: Skew::Behind(Duration::from_secs(20)),
                ..calm.forge
            },
            store: store::Script { latency: Span::millis(1, 2_000), failures: 50, done_anyway: 300 },
            restarts: 2,
            drops: 3,
            ..calm
        }
    }

    /// A world drawn from `seed`, between calm and rough.
    #[must_use]
    pub fn random(seed: u64) -> Settings {
        Settings::drawn(seed, &people::SWEPT)
    }

    /// A random world whose stories are drawn from all of them, plans'
    /// included.
    #[must_use]
    pub fn planning(seed: u64) -> Settings {
        Settings::drawn(seed, &people::STORIES)
    }

    /// A world drawn from `seed`, between calm and rough, its stories drawn
    /// from `told`.
    fn drawn(seed: u64, told: &[Story]) -> Settings {
        let mut rng = Rng::new(seed ^ 0x5eed);
        let calm = Settings::calm(seed);
        let skew = match rng.below(3) {
            0 => Skew::None,
            1 => Skew::Ahead(Duration::from_millis(rng.below(60_000))),
            _ => Skew::Behind(Duration::from_millis(rng.below(60_000))),
        };
        let small = |rng: &mut Rng, most: u64| u32::try_from(rng.below(most)).expect("small");
        let forge = forge::Config {
            late: small(&mut rng, 80),
            unavailable: small(&mut rng, 60),
            timeouts: small(&mut rng, 60),
            landing: small(&mut rng, 40),
            rate_limit: if rng.chance(400) { 40 } else { 0 },
            hooks_late: small(&mut rng, 400),
            hooks_lost: small(&mut rng, 1_001),
            skew,
            ..calm.forge
        };
        let mut stories = Vec::new();
        for story in told {
            if rng.chance(600) {
                stories.push(*story);
            }
        }
        let store = store::Script { latency: Span::millis(1, 2_000), failures: small(&mut rng, 80), done_anyway: 300 };
        let workers = 1 + usize::try_from(rng.below(3)).expect("few");
        // Some come back past their grace, and one of several may never.
        let redial = if rng.chance(300) { Span::millis(1_000, 60_000) } else { calm.redial };
        let vanishes = u32::from(workers > 1 && rng.chance(300));
        Settings { forge, stories, workers, redial, store, drops: small(&mut rng, 4), vanishes, ..calm }
    }

    /// A random world whose engine restarts once or twice, at drawn moments.
    #[must_use]
    pub fn restarting(seed: u64) -> Settings {
        let restarts = 1 + u32::try_from(seed % 2).expect("small");
        Settings { restarts, ..Settings::random(seed) }
    }

    /// The calm world with only `stories`.
    #[must_use]
    pub fn only(seed: u64, stories: &[Story]) -> Settings {
        Settings { stories: stories.to_vec(), ..Settings::calm(seed) }
    }
}

/// What the world counted, by name, that the sweep must reach.
pub const ENDINGS: [&str; 16] = [
    "acknowledged",
    "answer: ended",
    "answer: parked",
    "assigned",
    "corrected",
    "dropped",
    "inbound",
    "limited",
    "loaded",
    "merged",
    "relayed: served",
    "released",
    "reviewed",
    "store: failed",
    "timed out",
    "vanished",
];

/// What the world counted.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct Stats {
    pub endings: BTreeMap<&'static str, u32>,
    pub facts: u64,
    pub facts_lost: u64,
    pub restarts: u32,
    pub drops: u32,
    pub sent: u64,
    pub late: u32,
    pub workers: Vec<workers::Tally>,
    pub people: people::Tally,
    pub store: store::Tally,
    pub forge: Option<forge::Tally>,
}

/// Something on its way, delivered at its time.
#[derive(Debug)]
enum Delivery {
    /// The protocol layer's deadline for the engine's call it names.
    Deadline(u64),
    /// An event for the engine of life `life`.
    Engine {
        life: u64,
        event: Event,
    },
    /// Down the channel `channel` of a worker, and up it.
    Down {
        worker: usize,
        channel: Token,
        down: Down,
    },
    Up {
        worker: usize,
        channel: Token,
        up: Up,
    },
    /// A run's next act.
    Wake {
        worker: usize,
        item: Item,
        attempt: u64,
        serial: u64,
    },
    /// A worker dials in; checks its grace.
    Dial(usize),
    Grace(usize),
    /// People look at the forge.
    People,
}

/// An engine call the protocol layer has out.
#[derive(Debug)]
struct Out {
    call: Token,
    asked: Asked,
    life: u64,
    deadline: Key,
    expired: bool,
}

/// A call people or a worker have out on the forge.
#[derive(Debug)]
enum Theirs {
    Person { tale: Option<usize> },
    Review { repository: usize, number: u64, head: u64 },
    Push { worker: usize, item: Item, attempt: u64, repository: u32, branch: Box<[u8]>, commit: u64 },
}

/// A person's message, as they asked for it to be written.
#[derive(Debug)]
struct Message {
    item: Item,
    key: Vec<u8>,
    text: Vec<u8>,
}

pub struct World {
    now: Time,
    rng: Rng,
    settings: Settings,

    model: Model,
    stage: Stage<Limits, Event, Request>,
    life: u64,

    forge: forge::Model,
    forge_env: Env<forge::Config>,
    forge_out: Queue<forge::Request>,

    wire: Schedule<Delivery>,
    scheduled: u32,
    /// The latest a delivery down or up each channel is due, so that a
    /// channel keeps its order.
    order: BTreeMap<(u64, bool), Time>,
    /// The channels the engine of this life has heard a hello on, by worker.
    open: BTreeMap<u64, usize>,

    calls: Ledger<u64, Out>,
    owned: Ledger<(u64, Token), ()>,
    theirs: Ledger<u64, Theirs>,
    /// People's asks, by their names: who asked, the life it went to, and
    /// the item a message was for.
    asks: Ledger<u64, (Asker, u64, Option<Message>, Option<Item>)>,
    stores: Ledger<(u64, Token), ()>,

    workers: Vec<Worker>,
    /// The workers that vanished.
    gone: Vec<bool>,
    people: People,
    store: Store,
    mirror: Mirror,
    referee: Referee<Engine>,
    stats: Stats,
    trace: Trace,
    /// When each attempt was assigned, and each item closed, in order.
    assignments: Vec<(Time, Item, u64)>,
    closings: Vec<(Time, Item)>,
}

impl World {
    #[must_use]
    pub fn new(settings: Settings) -> World {
        assert!(engine::worst_case(&settings.limits).is_some(), "the shell refuses limits it cannot provision");
        let mut rng = Rng::new(settings.seed);
        let mut forge = forge::Model::new(&settings.forge, rng.next_u64());
        for name in REPOSITORIES.iter().chain([&ELSEWHERE]) {
            setup(&mut forge, &settings.forge, name, settings.dismiss_stale);
        }
        let expectations = Engine::new(settings.bounds, settings.stories.len()).restarting_at(&settings.restart_on);
        let mut referee = Referee::new(expectations);
        for _ in 0..settings.restarts {
            referee.inject(Time::ZERO.saturating_add(settings.restart_at.draw(&mut rng)), Stimulus::Restart);
        }
        for _ in 0..settings.drops {
            let worker = usize::try_from(rng.below(settings.workers as u64)).expect("few");
            referee.inject(Time::ZERO.saturating_add(settings.drop_at.draw(&mut rng)), Stimulus::Drop { worker });
        }
        for worker in 0..usize::try_from(settings.vanishes).expect("few") {
            referee.inject(Time::ZERO.saturating_add(settings.drop_at.draw(&mut rng)), Stimulus::Vanish { worker });
        }
        let max_out = engine::max_out(&settings.limits);
        let model = Model::new(config(&settings), &settings.limits, rng.next_u64(), Time::ZERO);
        let workers =
            (0..settings.workers).map(|_| Worker::new(settings.worker, rng.next_u64())).collect::<Vec<Worker>>();
        let mut world = World {
            now: Time::ZERO,
            rng: Rng::new(rng.next_u64()),
            model,
            stage: Stage::new(settings.limits, max_out, max_out + SLACK),
            life: 0,
            forge,
            forge_env: Env { now: Time::ZERO, limits: settings.forge },
            forge_out: Queue::with_capacity(64),
            wire: Schedule::new(),
            scheduled: 0,
            order: BTreeMap::new(),
            open: BTreeMap::new(),
            calls: Ledger::new("engine call"),
            owned: Ledger::new("engine's forge call"),
            theirs: Ledger::new("person's or worker's call"),
            asks: Ledger::new("person's ask"),
            stores: Ledger::new("store operation"),
            gone: vec![false; workers.len()],
            workers,
            people: People::new(&settings.stories),
            store: Store::new(settings.store, rng.next_u64(), TRACES),
            mirror: Mirror::default(),
            referee,
            stats: Stats::default(),
            trace: Trace::default(),
            assignments: Vec::new(),
            closings: Vec::new(),
            settings,
        };
        world.observe(Seen::Start);
        world.send(Time::ZERO, Delivery::People);
        for worker in 0..world.workers.len() {
            let at = world.settings.redial.draw(&mut world.rng);
            world.send(Time::ZERO.saturating_add(at), Delivery::Dial(worker));
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
            workers: self.workers.iter().map(Worker::tally).collect(),
            people: self.people.tally(),
            store: self.store.tally(),
            forge: Some(self.forge.tally()),
            facts_lost: self.model.facts_lost(),
            ..self.stats.clone()
        }
    }

    #[must_use]
    pub fn trace(&self) -> &[String] {
        self.trace.lines()
    }

    /// How many stories the world tells.
    #[must_use]
    pub fn stories(&self) -> usize {
        self.settings.stories.len()
    }

    /// How many safety checks the referee made, and liveness expectations
    /// it saw met.
    #[must_use]
    pub fn judged(&self) -> (u64, u64) {
        self.referee.judged()
    }

    /// The forge as observed.
    #[must_use]
    pub fn mirror(&self) -> &Mirror {
        &self.mirror
    }

    /// The store, for what it kept.
    #[must_use]
    pub fn store(&self) -> &Store {
        &self.store
    }

    /// The item of the story `tale`, once its person knows it.
    #[must_use]
    pub fn item(&self, tale: usize) -> Option<Item> {
        self.people.item(tale)
    }

    /// Keyed creations an engine made again after a restart.
    #[must_use]
    pub fn again(&self) -> u32 {
        self.referee.expectations().again
    }

    /// When each attempt was assigned to a worker, in order.
    #[must_use]
    pub fn assignments(&self) -> &[(Time, Item, u64)] {
        &self.assignments
    }

    /// When each item of the deployment's was closed, in order.
    #[must_use]
    pub fn closings(&self) -> &[(Time, Item)] {
        &self.closings
    }

    /// Runs until the stories are done and nothing is in flight, then checks
    /// the invariants of a settled world. Panics if it takes more than
    /// `iterations`.
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
            "seed {}: the world did not settle in {iterations} iterations, at {:?}: {:?}",
            self.settings.seed,
            self.now,
            self.referee.verdict()
        );
    }

    /// One iteration of the loop, as the shell would run it.
    fn iterate(&mut self) {
        let now = self.now;
        self.stage.tick(now);
        self.forge_env.now = now;
        while let Some(delivery) = self.wire.next(now) {
            self.scheduled -= 1;
            self.deliver(delivery);
        }
        while self.forge.is_due(now) {
            forge::fire(&mut self.forge, &self.forge_env, &mut self.forge_out);
            self.drain_forge();
        }
        if self.referee.is_due(now) {
            let mut stimuli = Vec::new();
            self.referee.fire(now, &mut stimuli);
            self.referee.assert_holding(self.settings.seed);
            for stimulus in stimuli {
                self.inject(stimulus);
            }
        }
        while self.stage.has_room() && self.model.is_ready() {
            engine::resume(&mut self.model, &self.stage.env, &mut self.stage.out);
        }
        while let Some(event) = self.stage.next_event() {
            self.log(format!("engine <- {}", describe(&event)));
            engine::step(&mut self.model, &self.stage.env, event, &mut self.stage.out);
        }
        while self.stage.has_room() && self.model.is_due(now) {
            engine::fire(&mut self.model, &self.stage.env, &mut self.stage.out);
        }
        while let Some(request) = self.stage.out.pop() {
            self.request(request);
        }
        self.observe_forge();
        while let Some(fact) = self.model.pop_fact() {
            self.stats.facts += 1;
            self.fact(&fact);
        }
        self.model.reclaim();
        self.forge.reclaim();
    }

    fn fact(&mut self, fact: &Fact) {
        let ending = match fact {
            Fact::Loaded => "loaded",
            Fact::Mangled { .. } => "mangled",
            Fact::Untracked { .. } => "untracked",
            Fact::Ruled { refused: true, .. } => "ruled: refused",
            Fact::Ruled { refused: false, .. } => "ruled: held",
            Fact::Work { .. }
            | Fact::Forge { .. }
            | Fact::Fleet { .. }
            | Fact::Brief { .. }
            | Fact::Notes { .. }
            | Fact::Views { .. } => return,
        };
        self.end(ending);
    }

    /// What the fake forge did: the mirror and the referee see it.
    fn observe_forge(&mut self) {
        while let Some(observation) = self.forge.pop_observation() {
            self.mirror.observe(&observation);
            match &observation {
                forge::Observation::Merged { .. } => self.end("merged"),
                forge::Observation::Closed { repository, number, .. } => {
                    if let Some(index) = deployment::index(repository) {
                        self.closings.push((self.now, Item { repository: index, number: *number }));
                    }
                }
                forge::Observation::Wiki { by, .. } if *by != ENGINE => self.end("corrected"),
                forge::Observation::Reviewed { .. } => self.end("reviewed"),
                forge::Observation::Wiki { .. }
                | forge::Observation::Moved { .. }
                | forge::Observation::Deleted { .. }
                | forge::Observation::Opened { .. }
                | forge::Observation::Reopened { .. }
                | forge::Observation::Labelled { .. }
                | forge::Observation::Revised { .. }
                | forge::Observation::Depends { .. }
                | forge::Observation::Requested { .. }
                | forge::Observation::Defined { .. }
                | forge::Observation::Commented { .. }
                | forge::Observation::Edited { .. }
                | forge::Observation::Removed { .. }
                | forge::Observation::Reported { .. }
                | forge::Observation::Refused { .. }
                | forge::Observation::Rejected { .. } => {}
            }
            self.observe(Seen::Forge(observation));
        }
    }

    fn deliver(&mut self, delivery: Delivery) {
        match delivery {
            Delivery::Deadline(name) => {
                let life = self.life;
                let Some(out) = self.calls.get_mut(name) else { return };
                assert!(out.life == life, "a restart withdraws its calls' deadlines");
                out.expired = true;
                let call = out.call;
                self.end("timed out");
                self.answer(call, Err(engine_api::Error::Timeout), Box::new([]));
            }
            Delivery::Engine { life, event } => {
                if life != self.life {
                    return;
                }
                match &event {
                    // The store's answer reaches the engine that asked: its
                    // operation ends here, once.
                    Event::Stored { owner, .. } => {
                        self.stores.end((life, *owner));
                    }
                    Event::Answered { .. }
                    | Event::Hint { .. }
                    | Event::Hello { .. }
                    | Event::Lost { .. }
                    | Event::Answer { .. }
                    | Event::Relay { .. }
                    | Event::Bounced { .. }
                    | Event::Told { .. }
                    | Event::Ask { .. }
                    | Event::Unwatch { .. }
                    | Event::Delivered { .. } => {}
                }
                self.stage.push(event);
            }
            Delivery::Down { worker, channel, down } => {
                if self.workers[worker].channel() != Some(channel) {
                    return;
                }
                self.end(match &down {
                    Down::Assign(_) => "assigned",
                    Down::Inbound { .. } => "inbound",
                    Down::Cancel { .. } => "cancelled",
                    Down::Relayed { served, .. } => {
                        if workers::is_served(served) {
                            "relayed: served"
                        } else {
                            "relayed: unserved"
                        }
                    }
                    Down::Acknowledge { .. } => "acknowledged",
                });
                self.observe_down(worker, &down);
                let mut effects = Vec::new();
                self.workers[worker].down(down, self.now, &self.mirror, &mut effects);
                self.effects(worker, effects);
            }
            Delivery::Up { worker, channel, up } => {
                if self.workers[worker].channel() != Some(channel) || !self.open.contains_key(&channel.raw()) {
                    return;
                }
                let event = World::up(channel, up);
                self.stage.push(event);
            }
            Delivery::Wake { worker, item, attempt, serial } => {
                let mut effects = Vec::new();
                self.workers[worker].act(item, attempt, serial, self.now, &mut effects);
                self.effects(worker, effects);
            }
            Delivery::Dial(worker) => self.dial(worker),
            Delivery::Grace(worker) => self.workers[worker].expire(self.now),
            Delivery::People => {
                let mut acts = Vec::new();
                self.people.act(&self.mirror, &mut acts);
                for act in acts {
                    self.person(act);
                }
                for tale in 0..self.settings.stories.len() {
                    if let Some(item) = self.people.item(tale) {
                        self.observe(Seen::Story { tale, item });
                    }
                }
                if !self.people.is_done(&self.mirror) {
                    let gap = self.settings.people.draw(&mut self.rng);
                    self.send(self.now.saturating_add(gap), Delivery::People);
                }
            }
        }
    }

    /// What the referee sees of what goes down a worker's channel: news
    /// for a run, an assignment, a call's answer.
    fn observe_down(&mut self, worker: usize, down: &Down) {
        match down {
            Down::Inbound { item, event, .. } => {
                // News, and the comment it is, if it is one.
                let news = match event {
                    engine::Inbound::News(engine::forge::News::Comment { id, .. }) => Some(Some(*id)),
                    engine::Inbound::News(engine::forge::News::Reviews { .. } | engine::forge::News::Pull { .. }) => {
                        Some(None)
                    }
                    engine::Inbound::Finished { .. }
                    | engine::Inbound::Held { .. }
                    | engine::Inbound::Decided { .. } => None,
                };
                if let Some(comment) = news {
                    self.observe(Seen::Inbound { item: *item, comment });
                }
            }
            Down::Assign(assigned) => {
                let item = assigned.item;
                let attempt = assigned.attempt;
                let live = self.live_elsewhere(worker, item, attempt);
                let charter = codec::charter_of(&assigned.charter).expect("a charter decodes as it was encoded");
                let brief = codec::brief_of(&charter);
                self.assignments.push((self.now, item, attempt));
                self.observe(Seen::Assigned { item, attempt, live, brief, grants: Some(charter.grants) });
            }
            Down::Relayed { item, attempt, call, served } => {
                let ungranted = match served {
                    engine::Served::Unserved(engine::Unserved::Ungranted) => true,
                    engine::Served::Unserved(
                        engine::Unserved::Busy
                        | engine::Unserved::Invalid
                        | engine::Unserved::Refused
                        | engine::Unserved::Failed,
                    )
                    | engine::Served::Read(_)
                    | engine::Served::Recalled { .. }
                    | engine::Served::Noted(_)
                    | engine::Served::Posted { .. } => false,
                };
                self.observe(Seen::Served { item: *item, attempt: *attempt, call: call.raw(), ungranted });
            }
            Down::Cancel { .. } | Down::Acknowledge { .. } => {}
        }
    }

    /// Whether a worker in contact hosts another attempt of `item`, not yet
    /// answered.
    fn live_elsewhere(&self, _worker: usize, item: Item, attempt: u64) -> bool {
        self.workers.iter().any(|worker| {
            worker.channel().is_some() && worker.running().iter().any(|&(of, at)| of == item && at != attempt)
        })
    }

    /// A worker dials in: a channel of its own, and its hello.
    fn dial(&mut self, worker: usize) {
        if self.workers[worker].channel().is_some() || self.gone[worker] {
            return;
        }
        let channel = Token::new(self.wire.name());
        let mut effects = Vec::new();
        self.workers[worker].hello(channel, self.now, &mut effects);
        self.open.insert(channel.raw(), worker);
        self.log(format!("worker {worker} dials in on {}", channel.raw()));
        self.effects(worker, effects);
    }

    /// The channel of `worker` drops: the engine of this life hears it lost.
    fn hang_up(&mut self, worker: usize) {
        let Some(channel) = self.workers[worker].channel() else { return };
        self.stats.drops += 1;
        self.end("dropped");
        self.log(format!("worker {worker}'s channel {} drops", channel.raw()));
        let mut effects = Vec::new();
        self.workers[worker].lose(self.now, &mut effects);
        self.effects(worker, effects);
        if self.open.remove(&channel.raw()).is_some() {
            self.stage.push(Event::Lost { channel });
        }
        let grace = self.settings.worker.grace;
        self.send(self.now.saturating_add(grace), Delivery::Grace(worker));
        let at = self.settings.redial.draw(&mut self.rng);
        self.send(self.now.saturating_add(at), Delivery::Dial(worker));
    }

    /// The worker `worker` goes, its channel with it, and never comes back:
    /// what it hosted and kept is lost.
    fn vanish(&mut self, worker: usize) {
        self.end("vanished");
        self.log(format!("worker {worker} vanishes"));
        if let Some(channel) = self.workers[worker].channel()
            && self.open.remove(&channel.raw()).is_some()
        {
            self.stage.push(Event::Lost { channel });
        }
        self.gone[worker] = true;
        self.workers[worker] = Worker::new(self.settings.worker, 0);
    }

    /// What a worker asked of the world.
    fn effects(&mut self, worker: usize, effects: Vec<Effect>) {
        for effect in effects {
            match effect {
                Effect::Up(up) => {
                    if let Up::Answer { item, attempt, said } = &up {
                        self.end(match said {
                            Said::Busy => "answer: busy",
                            Said::Ended { .. } => "answer: ended",
                            Said::Parked { .. } => "answer: parked",
                            Said::Failed { .. } => "answer: failed",
                        });
                        self.observe(Seen::Answered { item: *item, attempt: *attempt });
                    }
                    if let Up::Bounced { .. } = &up {
                        self.end("bounced");
                    }
                    if let Up::Relay { item, attempt, call, body } = &up {
                        let needs = match body {
                            engine::Call::Read(_) => Needs::Forge,
                            engine::Call::Note { scope, .. } => Needs::Note(*scope),
                            engine::Call::Recall(_) | engine::Call::Comment { .. } | engine::Call::Escalate { .. } => {
                                Needs::Nothing
                            }
                        };
                        self.observe(Seen::Called { item: *item, attempt: *attempt, call: call.raw(), needs });
                    }
                    let Some(channel) = self.workers[worker].channel() else { continue };
                    let at = self.channel_time(channel, true);
                    self.send(at, Delivery::Up { worker, channel, up });
                }
                Effect::Wake { at, item, attempt, serial } => {
                    self.send(at, Delivery::Wake { worker, item, attempt, serial });
                }
                Effect::Push { item, attempt, repository, start, branch, content } => {
                    self.push(worker, item, attempt, repository, &start, branch, &content);
                }
            }
        }
    }

    /// When the next delivery on `channel` is due, in order, up or down.
    fn channel_time(&mut self, channel: Token, up: bool) -> Time {
        let at = self.now.saturating_add(self.settings.channel.draw(&mut self.rng));
        let last = self.order.entry((channel.raw(), up)).or_insert(Time::ZERO);
        let at = if at < *last { *last } else { at };
        *last = at;
        at
    }

    /// What a worker says, as the engine's protocol layer hands it on.
    fn up(channel: Token, up: Up) -> Event {
        match up {
            Up::Hello { slots, workstreams, hosting } => Event::Hello {
                channel,
                hello: Hello { slots, workstreams: workstreams.into(), hosting: hosting.into() },
            },
            Up::Answer { item, attempt, said } => Event::Answer { channel, item, attempt, answer: answer_of(said) },
            Up::Relay { item, attempt, call, body } => Event::Relay { channel, item, attempt, call, body },
            Up::Bounced { item, attempt, bounce } => Event::Bounced { item, attempt, bounce },
            Up::Told { item, attempt, kind, content } => Event::Told { item, attempt, kind, content: content.into() },
        }
    }

    /// A worker pushes the change of `item`'s run: `content` in the file CI
    /// reads, on its start, to its branch.
    #[expect(clippy::too_many_arguments, reason = "a push says all of this")]
    fn push(
        &mut self,
        worker: usize,
        item: Item,
        attempt: u64,
        repository: u32,
        start: &Start,
        branch: Box<[u8]>,
        content: &[u8],
    ) {
        let name = deployment::name(repository);
        let main = self.forge.branch(name, MAIN).expect("a default branch");
        let tip = match start {
            Start::Base { branch } | Start::Branch { branch } | Start::Saved { branch } => {
                self.forge.branch(name, branch).unwrap_or(main)
            }
            Start::Commit { commit } => translate::count(*commit),
        };
        // It merges its base before it pushes: its tree is the base's as it
        // is now, with its own files, on top of where it started, so a
        // change rebased onto a base that moved merges cleanly.
        let mut files: Vec<File> = self
            .forge
            .object(main)
            .expect("the base's commit")
            .tree
            .iter()
            .filter(|(path, _)| ***path != *CUE)
            .map(|(path, content)| File { path: path.clone(), content: content.clone() })
            .collect();
        files.push(File { path: CUE.into(), content: content.into() });
        // What the change itself writes: its own file, so that no two
        // attempts make the same tree, nor one its base's.
        let path = format!("change-{}", item.number).into_bytes().into_boxed_slice();
        files.retain(|file| file.path != path);
        files.push(File { path, content: format!("attempt {attempt}").into_bytes().into_boxed_slice() });
        let commit = match forge::commit(&mut self.forge, &self.settings.forge, tip, files.into_boxed_slice()) {
            Ok(Some(commit)) => commit,
            Ok(None) => tip,
            Err(_) => {
                self.end("push refused");
                let mut effects = Vec::new();
                self.workers[worker].pushed(item, attempt, None, self.now, &mut effects);
                self.effects(worker, effects);
                return;
            }
        };
        let op = forge_api::Op::Git(Git::Push { branch: branch.clone(), commit });
        let at = usize::try_from(repository).expect("few");
        let theirs = Theirs::Push { worker, item, attempt, repository, branch, commit };
        self.call(WORKER, REPOSITORIES[at], op, theirs);
    }

    /// A person acts.
    fn person(&mut self, act: people::Act) {
        match act {
            people::Act::Ask { asker, person, ask } => {
                let name = self.wire.name();
                let message = match &ask {
                    engine::Ask::Message { item, key, message } => {
                        Some(Message { item: *item, key: key.to_vec(), text: message.to_vec() })
                    }
                    engine::Ask::Open { .. }
                    | engine::Ask::Accept { .. }
                    | engine::Ask::Reject { .. }
                    | engine::Ask::Stop { .. }
                    | engine::Ask::Release { .. }
                    | engine::Ask::Watch { .. } => None,
                };
                let accept = match &ask {
                    engine::Ask::Accept { item } => Some(*item),
                    engine::Ask::Open { .. }
                    | engine::Ask::Message { .. }
                    | engine::Ask::Reject { .. }
                    | engine::Ask::Stop { .. }
                    | engine::Ask::Release { .. }
                    | engine::Ask::Watch { .. } => None,
                };
                self.asks.open(name, (asker, self.life, message, accept));
                self.log(format!("person {person} asks {ask:?}"));
                self.stage.push(Event::Ask { reply_to: ReplyTo::new(Token::new(name)), person, ask });
            }
            people::Act::Forge { tale, user, repository, op } => {
                self.log(format!("person {user} calls {op:?}"));
                let theirs = match &op {
                    forge_api::Op::Write(forge_api::Write::Review { number, .. }) if tale.is_none() => {
                        let head = self.mirror_head(repository, *number);
                        Theirs::Review { repository, number: *number, head }
                    }
                    forge_api::Op::Read(_) | forge_api::Op::Write(_) | forge_api::Op::Git(_) => Theirs::Person { tale },
                };
                self.call(user, REPOSITORIES[repository], op, theirs);
            }
        }
    }

    fn mirror_head(&self, repository: usize, number: u64) -> u64 {
        let issue = self.mirror.issue(REPOSITORIES[repository], number).expect("a pull request reviewed");
        issue.pull.as_ref().expect("a pull request").commit
    }

    /// A call of someone else's to the forge.
    fn call(&mut self, user: u64, repository: &[u8], op: forge_api::Op, theirs: Theirs) {
        let name = self.wire.name();
        self.theirs.open(name, theirs);
        let reply_to = ReplyTo::new(Token::new(name));
        let event = forge::Event::Call { reply_to, user, repository: repository.into(), op };
        forge::step(&mut self.forge, &self.forge_env, event, &mut self.forge_out);
        self.drain_forge();
    }

    /// What the forge emitted: replies to calls, and webhooks.
    fn drain_forge(&mut self) {
        while let Some(request) = self.forge_out.pop() {
            match request {
                forge::Request::Reply { to, result } => self.reply(to.into_token().raw(), result),
                forge::Request::Hook { repository, change: _, number, branch, commit } => {
                    let Some(repository) = deployment::index(&repository) else { continue };
                    let commit = commit.map(translate::commit);
                    self.stage.push(Event::Hint { repository, item: number, commit, branch });
                }
            }
        }
    }

    /// The forge answered the call `name`.
    fn reply(&mut self, name: u64, result: Result<forge_api::Answer, forge_api::Error>) {
        if self.calls.contains(name) {
            let out = self.calls.end(name);
            if out.expired || out.life != self.life {
                self.stats.late += 1;
                return;
            }
            self.withdraw(out.deadline);
            let now = forge::time(&self.settings.forge, self.now);
            let (bodies, page) = translate::bodies(&result);
            let limits = &self.settings.limits.forge;
            let mut answer = temper_engine_model_forge_tests::translate::answer(out.asked, result, limits, now);
            let decoded = translate::decode(&mut answer, &bodies, page);
            if let Err(engine_api::Error::RateLimited { .. }) = &answer {
                self.end("limited");
            }
            self.answer(out.call, answer, decoded);
            return;
        }
        let made = result.is_ok();
        self.log(format!("their call {name} answered {}", if made { "ok" } else { "failed" }));
        match self.theirs.end(name) {
            Theirs::Person { tale } => {
                if let Some(tale) = tale {
                    self.people.called(tale, made);
                }
            }
            Theirs::Review { repository, number, head } => self.people.reviewed(repository, number, head),
            Theirs::Push { worker, item, attempt, repository, branch, commit } => {
                // A push that ended ambiguously is verified against the
                // forge, by its branch (worker-model.md, section 5).
                let landed = match result {
                    Ok(forge_api::Answer::Pushed(pushed)) => pushed == forge_api::Pushed::Pushed,
                    Ok(_) => false,
                    Err(_) => self.forge.branch(deployment::name(repository), &branch) == Some(commit),
                };
                let pushed = landed.then(|| Landed { repository, commit: translate::commit(commit) });
                let mut effects = Vec::new();
                self.workers[worker].pushed(item, attempt, pushed, self.now, &mut effects);
                self.effects(worker, effects);
            }
        }
    }

    /// Terminal for the engine's call `call`.
    fn answer(
        &mut self,
        call: Token,
        result: Result<engine_api::Answer, engine_api::Error>,
        decoded: Box<[engine::Decoded]>,
    ) {
        self.owned.end((self.life, call));
        self.stage.push(Event::Answered { call, result, decoded });
    }

    /// What the engine asked for.
    fn request(&mut self, request: Request) {
        self.log(format!("engine -> {}", describe_request(&request)));
        match request {
            Request::Forge { call, repository, op, payload } => {
                self.stats.sent += 1;
                self.owned.open((self.life, call), ());
                let (asked, op) = translate::op(op, payload.as_ref(), self.settings.limits.forge.page);
                let name = self.wire.name();
                let deadline = self.send(self.now.saturating_add(self.settings.timeout), Delivery::Deadline(name));
                self.calls.open(name, Out { call, asked, life: self.life, deadline, expired: false });
                let reply_to = ReplyTo::new(Token::new(name));
                let repository = deployment::name(repository).into();
                let event = forge::Event::Call { reply_to, user: ENGINE, repository, op };
                forge::step(&mut self.forge, &self.forge_env, event, &mut self.forge_out);
                self.drain_forge();
            }
            Request::Assign { channel, assignment } => {
                let Assignment { item, attempt, workspace, save: _, charter, snapshot } = assignment;
                let charter = crate::codec::charter(&charter);
                let assigned = Assigned { item, attempt, workspace, charter, snapshot };
                self.down(channel, Down::Assign(assigned));
            }
            Request::Inbound { channel, item, attempt, event } => {
                self.down(channel, Down::Inbound { item, attempt, event });
            }
            Request::Cancel { channel, item, attempt } => self.down(channel, Down::Cancel { item, attempt }),
            Request::Relayed { channel, item, attempt, call, served } => {
                self.down(channel, Down::Relayed { item, attempt, call, served });
            }
            Request::Acknowledge { channel, item, attempt } => {
                self.down(channel, Down::Acknowledge { item, attempt });
            }
            Request::Refuse { channel } => {
                if let Some(&worker) = self.open.get(&channel.raw()) {
                    self.open.remove(&channel.raw());
                    if self.workers[worker].channel() == Some(channel) {
                        self.end("refused");
                        let mut effects = Vec::new();
                        self.workers[worker].lose(self.now, &mut effects);
                        self.effects(worker, effects);
                        let at = self.settings.redial.draw(&mut self.rng);
                        self.send(self.now.saturating_add(at), Delivery::Dial(worker));
                    }
                }
            }
            Request::Reply { to, reply } => {
                let name = to.into_token().raw();
                let (asker, life, message, accept) = self.asks.end(name);
                let messaged = message.is_some();
                if let Some(item) = accept {
                    self.observe(Seen::Accepted { item, reply });
                }
                assert_eq!(life, self.life, "a person's ask is answered by the engine it went to");
                if reply == engine::Reply::Done
                    && let Some(Message { item, key, text }) = message
                {
                    self.observe(Seen::Messaged { item, key, text });
                }
                if let Asker::Caretaker(_) = asker
                    && reply == engine::Reply::Done
                {
                    self.end("released");
                }
                self.people.replied(asker, reply, messaged);
            }
            Request::Deliver { watcher, .. } => {
                let at = self.now.saturating_add(self.settings.channel.draw(&mut self.rng));
                self.send(at, Delivery::Engine { life: self.life, event: Event::Delivered { watcher, done: true } });
            }
            Request::Ended { .. } => {}
            Request::Store { owner, op } => {
                self.stores.open((self.life, owner), ());
                let (stored, after) = self.store.apply(op);
                if stored == engine::Stored::Failed {
                    self.end("store: failed");
                }
                let event = Event::Stored { owner, stored };
                self.send(self.now.saturating_add(after), Delivery::Engine { life: self.life, event });
            }
        }
    }

    /// Sends `down` on `channel`, if a worker holds it.
    fn down(&mut self, channel: Token, down: Down) {
        let Some(&worker) = self.open.get(&channel.raw()) else { return };
        let at = self.channel_time(channel, false);
        self.send(at, Delivery::Down { worker, channel, down });
    }

    fn inject(&mut self, stimulus: Stimulus) {
        match stimulus {
            Stimulus::Restart => self.restart(),
            Stimulus::Drop { worker } => self.hang_up(worker),
            Stimulus::Vanish { worker } => self.vanish(worker),
        }
    }

    /// The engine restarts: a new model, starting cold. Its channels close,
    /// people's asks in flight are lost, and the calls it made still reach
    /// the forge, their answers dropped.
    fn restart(&mut self) {
        self.stats.restarts += 1;
        self.log("the engine restarts".to_owned());
        self.life += 1;
        let seed = self.rng.next_u64();
        self.model = Model::new(config(&self.settings), &self.settings.limits, seed, self.now);
        let max_out = engine::max_out(&self.settings.limits);
        self.stage = Stage::new(self.settings.limits, max_out, max_out + SLACK);
        self.stage.tick(self.now);
        let keys: Vec<Key> = self.calls.values().map(|out| out.deadline).collect();
        for key in keys {
            self.withdraw(key);
        }
        self.owned = Ledger::new("engine's forge call");
        // What the store was asked by the engine that stopped is answered to
        // no one.
        let stores: Vec<(u64, Token)> = self.stores.keys().copied().collect();
        for key in stores {
            self.stores.end(key);
        }
        let asks: Vec<u64> = self.asks.keys().copied().collect();
        for name in asks {
            let (asker, _, _, _) = self.asks.end(name);
            self.people.lost(asker);
        }
        self.open.clear();
        for worker in 0..self.workers.len() {
            if self.workers[worker].channel().is_some() {
                let mut effects = Vec::new();
                self.workers[worker].lose(self.now, &mut effects);
                self.effects(worker, effects);
                let grace = self.settings.worker.grace;
                self.send(self.now.saturating_add(grace), Delivery::Grace(worker));
                let at = self.settings.redial.draw(&mut self.rng);
                self.send(self.now.saturating_add(at), Delivery::Dial(worker));
            }
        }
        self.observe(Seen::Restarted);
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

    fn withdraw(&mut self, key: Key) {
        if self.wire.withdraw(key).is_some() {
            self.scheduled -= 1;
        }
    }

    fn log(&mut self, line: String) {
        assert!(self.trace.lines().len() < TRACE, "seed {}: the trace grew past its bound", self.settings.seed);
        self.trace.log(self.now, line);
    }

    fn end(&mut self, ending: &'static str) {
        *self.stats.endings.entry(ending).or_default() += 1;
    }

    fn observe(&mut self, seen: Seen) {
        let mut stimuli = Vec::new();
        self.referee.observe(self.now, seen, &mut stimuli);
        self.referee.assert_holding(self.settings.seed);
        for stimulus in stimuli {
            self.inject(stimulus);
        }
    }

    fn has_work_now(&self) -> bool {
        self.stage.has_events()
            || self.model.is_ready()
            || self.model.is_due(self.now)
            || self.wire.is_due(self.now)
            || self.forge.is_due(self.now)
            || self.referee.is_due(self.now)
    }

    fn next_time(&self) -> Option<Time> {
        [self.wire.next_time(), self.model.next_deadline(), self.forge.next_deadline(), self.referee.next_deadline()]
            .into_iter()
            .flatten()
            .min()
    }

    /// Whether the world has settled: the stories done, the workers idle,
    /// nothing in flight anywhere, and the referee expecting nothing more.
    fn is_quiet(&self) -> bool {
        self.people.is_done(&self.mirror)
            && self.wire.is_empty()
            && self.workers.iter().all(Worker::is_idle)
            && self.calls.is_empty()
            && self.theirs.is_empty()
            && self.asks.is_empty()
            && self.forge.calls() == 0
            && self.forge.deliveries() == 0
            && self.unsettled().is_empty()
            && match self.referee.verdict() {
                temper_world::Verdict::Open { .. } => false,
                temper_world::Verdict::Passed
                | temper_world::Verdict::Stopped { .. }
                | temper_world::Verdict::Failed(_) => true,
            }
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
                temper_engine_model::work::Phase::Held { .. } => {}
                temper_engine_model::work::Phase::Waiting
                | temper_engine_model::work::Phase::Parked
                | temper_engine_model::work::Phase::Retrying(_)
                | temper_engine_model::work::Phase::Claimed
                | temper_engine_model::work::Phase::Applying { .. }
                | temper_engine_model::work::Phase::Done => unsettled.push(number),
            }
        }
        unsettled
    }

    /// The invariants of a settled world.
    fn assert_settled(&mut self) {
        let seed = self.settings.seed;
        self.calls.assert_settled();
        self.owned.assert_settled();
        self.theirs.assert_settled();
        self.asks.assert_settled();
        self.stores.assert_settled();
        let unsettled = self.unsettled();
        assert!(unsettled.is_empty(), "seed {seed}: every open item the engine tracks is held: {unsettled:?} are not");
        let tally = self.forge.tally();
        assert_eq!(tally.forgotten, 0, "seed {seed}: the forge kept every call it took: {tally:?}");
        self.observe(Seen::Settled);
        self.referee.assert_passed(seed);
    }
}

/// A worker's answer, as the engine's protocol layer decodes it.
fn answer_of(said: Said) -> Answer {
    let work = |landed: Vec<Landed>| Work { landed: landed.into() };
    match said {
        Said::Busy => Answer::Busy,
        Said::Ended { outcome, landed } => Answer::Ended {
            outcome: crate::codec::outcome_of(&outcome).expect("an outcome decodes as it was encoded"),
            work: work(landed),
        },
        Said::Parked { snapshot, landed } => {
            Answer::Parked { snapshot: snapshot.map(Vec::into_boxed_slice), work: work(landed) }
        }
        Said::Failed { failure, landed } => Answer::Failed { failure, work: work(landed) },
    }
}

/// The deployment's configuration, its sessions' wake rule batching as the
/// settings say.
fn config(settings: &Settings) -> engine::Config {
    let mut config = deployment::config();
    config.session.wake.batch = settings.batch;
    config
}

/// Sets up a repository of the fake forge: its default branch holding a
/// file CI reads as green, CI cued by it, its default branch protected
/// (dismissing approvals of earlier heads if `dismiss_stale`), and its
/// users.
fn setup(forge: &mut forge::Model, config: &forge::Config, name: &[u8], dismiss_stale: bool) {
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
            dismiss_stale,
        }),
        hooked: true,
    };
    forge::repository(forge, config, setup);
    forge::grant(forge, name, ENGINE, Permission::Write);
    forge::grant(forge, name, WORKER, Permission::Write);
    forge::grant(forge, name, CI, Permission::Write);
    forge::grant(forge, name, PEOPLE[0], Permission::Admin);
    for user in PEOPLE[1..].iter().chain([&REVIEWER]) {
        forge::grant(forge, name, *user, Permission::Write);
    }
    forge::grant(forge, name, STRANGER, Permission::Read);
}

fn describe(event: &Event) -> String {
    match event {
        Event::Answered { call, result, decoded } => {
            format!("answered {} {} ({} decoded)", call.raw(), short(result), decoded.len())
        }
        Event::Hint { .. }
        | Event::Hello { .. }
        | Event::Lost { .. }
        | Event::Answer { .. }
        | Event::Relay { .. }
        | Event::Bounced { .. }
        | Event::Told { .. }
        | Event::Ask { .. }
        | Event::Unwatch { .. }
        | Event::Delivered { .. }
        | Event::Stored { .. } => format!("{event:?}"),
    }
}

fn short(result: &Result<engine_api::Answer, engine_api::Error>) -> String {
    match result {
        Ok(engine_api::Answer::Items { items, more, .. }) => {
            let numbers: Vec<u64> = items.iter().map(|item| item.number).collect();
            format!("items {numbers:?} more {more}")
        }
        Ok(engine_api::Answer::Item { item, comments, .. }) => {
            let ids: Vec<u64> = comments.iter().map(|comment| comment.id).collect();
            format!("item {} comments {ids:?}", item.number)
        }
        Ok(answer) => format!("{answer:?}"),
        Err(error) => format!("{error:?}"),
    }
}

fn describe_request(request: &Request) -> String {
    match request {
        Request::Assign { channel, assignment } => {
            format!("assign {:?}#{} on {}", assignment.item, assignment.attempt, channel.raw())
        }
        Request::Forge { .. }
        | Request::Inbound { .. }
        | Request::Cancel { .. }
        | Request::Relayed { .. }
        | Request::Acknowledge { .. }
        | Request::Refuse { .. }
        | Request::Reply { .. }
        | Request::Deliver { .. }
        | Request::Ended { .. }
        | Request::Store { .. } => format!("{request:?}"),
    }
}
