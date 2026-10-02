use std::collections::BTreeMap;

use temper_engine_model_forge::{
    self as sub, Config as Deployment, Event, Fact, Failure, Item, Limits, Model, News, Record, Request, Written,
};
use temper_forge_model::api::{self as forge_api, Checks, File, Git, Permission, Protection, Setup};
use temper_forge_model::{self as forge, Config};
use temper_lib::{Duration, Env, Queue, ReplyTo, Rng, Time, Token};
use temper_world::{Key, Ledger, Referee, Schedule, Span, Stage, Trace};

use crate::parent::{self, Action, Parent};
use crate::people::{self, Act, PEOPLE, People};
use crate::referee::{Forge, Seen, Stimulus};
use crate::translate::{self, Asked};

/// The engine's forge user, CI's, and the workers'.
pub const ENGINE: u64 = 1;
pub const CI: u64 = 2;
pub const WORKER: u64 = 3;

/// The deployment's repositories, by their forge names, in its order.
pub const REPOSITORIES: [&[u8]; 2] = [b"acme/one", b"acme/two"];
pub const MAIN: &[u8] = b"main";

/// The tracking label, the hand-in label, and the phases the parent
/// projects; and every label the repositories define.
pub const TRACKING: &[u8] = b"temper";
pub const HAND_IN: &[u8] = b"temper:hand-in";
pub const WORKING: &[u8] = b"temper:working";
pub const WAITING: &[u8] = b"temper:waiting";
pub const LABELS: [&[u8]; 6] = [TRACKING, HAND_IN, WORKING, WAITING, b"bug", b"feature"];

/// Room in the sub-model's output queue beyond what one step may emit.
const SLACK: u32 = 2;

/// The world's own bounds: lines of its trace, and deliveries scheduled at
/// once. A world past either fails with its seed, rather than grow.
const TRACE: usize = 400_000;
const DELIVERIES: u32 = 20_000;

/// What the sub-model told, by kind: each must be reached by the sweep.
pub const ENDINGS: [&str; 23] = [
    "announced: found",
    "announced: missing",
    "announced: mangled",
    "announced: unlabelled",
    "offered",
    "full",
    "news: comment",
    "news: review",
    "news: pull",
    "changed",
    "left",
    "loaded",
    "read",
    "read: failed",
    "wrote",
    "wrote: created",
    "wrote: commented",
    "wrote: merged",
    "wrote: edited",
    "wrote: failed",
    "found",
    "timed out",
    "limited",
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Settings {
    pub seed: u64,
    pub limits: Limits,
    pub forge: Config,
    pub parent: parent::Script,
    pub people: people::Script,
    /// The protocol layer's deadline for a call: an answer later than this
    /// is a timeout, and the late answer is dropped.
    pub timeout: Duration,
    /// Restarts of the engine, each at a moment drawn from `restart_at`.
    pub restarts: u32,
    pub restart_at: Span,
    /// The bound within which a change reaches the working set.
    pub within: Duration,
}

impl Settings {
    /// A world where nothing goes wrong: room for everything, answers in
    /// time, webhooks delivered, no restarts.
    #[must_use]
    pub const fn calm(seed: u64) -> Settings {
        Settings {
            seed,
            limits: Limits {
                repositories: 2,
                items: 16,
                labels: 6,
                inbox: 8,
                reads: 4,
                writes: 8,
                calls: 6,
                page: 8,
                name_bytes: 48,
                title_bytes: 32,
                body_bytes: 96,
                rate: 120,
                window: Duration::from_secs(60),
                poll: Duration::from_secs(30),
                hinted: Duration::from_secs(2),
                resolution: Duration::from_secs(1),
                slow: Duration::from_secs(120),
                backoff: Duration::from_millis(500),
                backoff_max: Duration::from_secs(10),
                attempts: 5,
                facts: 64,
            },
            forge: Config {
                limits: forge::Limits {
                    repositories: 2,
                    users: 8,
                    labels: 8,
                    items: 96,
                    comments: 96,
                    reviews: 8,
                    dependencies: 4,
                    branches: 64,
                    commits: 1024,
                    files: 8,
                    statuses: 128,
                    contexts: 1,
                    pages: 8,
                    name_bytes: 48,
                    title_bytes: 64,
                    body_bytes: 256,
                    content_bytes: 128,
                    page_size: 64,
                    calls: 64,
                    hooks: 64,
                    observations: 4096,
                },
                latency_min: Duration::from_millis(20),
                latency_max: Duration::from_millis(400),
                late: 0,
                late_min: Duration::from_secs(5),
                late_max: Duration::from_secs(20),
                unavailable: 0,
                timeouts: 0,
                rate_limit: 0,
                rate_window: Duration::from_secs(60),
                ci: CI,
                hook_min: Duration::from_millis(50),
                hook_max: Duration::from_millis(500),
                hooks_late: 0,
                hooks_lost: 0,
                resolution: Duration::from_secs(1),
                status_updates: false,
                edit_updates: false,
            },
            parent: parent::Script {
                run: Span::millis(1_000, 20_000),
                replies: 300,
                projections: 300,
                tasks: 150,
                changes: 250,
                notes: 150,
                reads: 150,
                closes: 80,
                runs: 40,
                takes: 12,
                release: Span::millis(5_000, 60_000),
            },
            people: people::Script {
                actions: 120,
                gap: Span::millis(500, 8_000),
                weights: people::Weights {
                    opens: 6,
                    comments: 10,
                    labels: 4,
                    removals: 0,
                    hand_ins: 3,
                    reviews: 4,
                    pushes: 2,
                    closes: 1,
                    mangles: 0,
                },
            },
            timeout: Duration::from_secs(10),
            restarts: 0,
            restart_at: Span::millis(10_000, 600_000),
            within: Duration::from_secs(150),
        }
    }

    /// A world where everything that can go wrong does, now and then: a
    /// world for the random sweep.
    #[must_use]
    pub const fn rough(seed: u64) -> Settings {
        let calm = Settings::calm(seed);
        Settings {
            limits: Limits {
                items: 6,
                inbox: 4,
                reads: 2,
                writes: 4,
                calls: 3,
                page: 4,
                rate: 40,
                facts: 8,
                ..calm.limits
            },
            forge: Config {
                late: 60,
                unavailable: 40,
                timeouts: 40,
                rate_limit: 50,
                hooks_late: 200,
                hooks_lost: 300,
                ..calm.forge
            },
            people: people::Script {
                weights: people::Weights { removals: 2, closes: 2, mangles: 1, ..calm.people.weights },
                ..calm.people
            },
            restarts: 2,
            within: Duration::from_secs(240),
            ..calm
        }
    }

    /// A world drawn from `seed`, between calm and rough.
    #[must_use]
    pub fn random(seed: u64) -> Settings {
        let mut rng = Rng::new(seed ^ 0x5eed);
        let rough = Settings::rough(seed);
        let calm = Settings::calm(seed);
        let pick = |rng: &mut Rng, chance: u32| rng.chance(chance);
        let limits = if pick(&mut rng, 500) { rough.limits } else { calm.limits };
        let forge = Config {
            late: u32::try_from(rng.below(80)).expect("small"),
            unavailable: u32::try_from(rng.below(60)).expect("small"),
            timeouts: u32::try_from(rng.below(60)).expect("small"),
            rate_limit: if pick(&mut rng, 400) { 40 } else { 0 },
            hooks_late: u32::try_from(rng.below(400)).expect("small"),
            hooks_lost: u32::try_from(rng.below(1001)).expect("small"),
            ..calm.forge
        };
        let people = if pick(&mut rng, 500) { rough.people } else { calm.people };
        let restarts = u32::try_from(rng.below(3)).expect("small");
        Settings { limits, forge, people, restarts, within: rough.within, ..calm }
    }
}

/// What the world counted.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct Stats {
    pub endings: BTreeMap<&'static str, u32>,
    pub facts: u64,
    pub facts_lost: u64,
    pub restarts: u32,
    pub sent: u64,
    pub late: u32,
    pub parent: parent::Tally,
    pub people: people::Tally,
    pub forge: Option<forge::Tally>,
    /// The most items the working set held at once.
    pub peak: u32,
    /// The writes that failed, by why.
    pub failures: BTreeMap<String, u32>,
}

/// Something on its way, delivered at its time.
#[derive(Debug)]
enum Delivery {
    /// The parent acts.
    Parent(Action),
    /// A person acts.
    People,
    /// The protocol layer's deadline for the engine's call it names.
    Deadline(u64),
}

/// An engine call the protocol layer has out: the sub-model's name for it,
/// what it asked, the engine's life it belongs to, its deadline, and whether
/// it timed out.
#[derive(Debug)]
struct Out {
    call: Token,
    asked: Asked,
    life: u64,
    deadline: Key,
    expired: bool,
}

/// A call people or a worker have out.
#[derive(Debug)]
enum Theirs {
    Person,
    /// A worker pushing `item`'s branch.
    Push {
        item: Item,
        branch: Vec<u8>,
    },
}

pub struct World {
    now: Time,
    rng: Rng,
    settings: Settings,

    model: Model,
    stage: Stage<Limits, Event, Request>,
    /// The engine's lives: one more each restart.
    life: u64,

    forge: forge::Model,
    forge_env: Env<Config>,
    forge_out: Queue<forge::Request>,

    wire: Schedule<Delivery>,
    scheduled: u32,
    /// The parent's deliveries in flight, which a restart withdraws.
    pending: Vec<Key>,
    /// The engine's calls out, by the protocol layer's names; those of the
    /// sub-model, by its own (each ended once in a life); people's and
    /// workers' calls.
    calls: Ledger<u64, Out>,
    owned: Ledger<(u64, Token), ()>,
    theirs: Ledger<u64, Theirs>,
    /// The reads and writes the parent has in the sub-model, each answered
    /// once.
    reads: Ledger<u64, ()>,
    writes: Ledger<u64, ()>,

    parent: Parent,
    people: People,
    referee: Referee<Forge>,
    stats: Stats,
    trace: Trace,
}

impl World {
    #[must_use]
    pub fn new(settings: Settings) -> World {
        assert!(sub::worst_case(&settings.limits).is_some(), "the shell refuses limits it cannot provision");
        let mut rng = Rng::new(settings.seed);
        let max_out = sub::max_out(&settings.limits);
        let forge_seed = rng.next_u64();
        let mut forge = forge::Model::new(&settings.forge, forge_seed);
        for name in REPOSITORIES {
            let setup = Setup {
                name: name.into(),
                default: MAIN.into(),
                tree: Box::new([File { path: b"README".as_slice().into(), content: b"hello".as_slice().into() }]),
                labels: LABELS.iter().map(|label| (*label).into()).collect(),
                checks: Checks {
                    contexts: Box::new([b"ci".as_slice().into()]),
                    latency_min: Duration::from_secs(1),
                    latency_max: Duration::from_secs(40),
                    silent: 20,
                    passes: 850,
                    cue: None,
                },
                protection: Some(Protection {
                    branch: MAIN.into(),
                    contexts: Box::new([b"ci".as_slice().into()]),
                    approvals: 0,
                    dismiss_stale: false,
                }),
                hooked: true,
            };
            forge::repository(&mut forge, &settings.forge, setup);
            forge::grant(&mut forge, name, ENGINE, Permission::Write);
            forge::grant(&mut forge, name, WORKER, Permission::Write);
            forge::grant(&mut forge, name, CI, Permission::Write);
            forge::grant(&mut forge, name, PEOPLE[0], Permission::Admin);
            for user in &PEOPLE[1..] {
                forge::grant(&mut forge, name, *user, Permission::Write);
            }
        }
        let mut referee = Referee::new(Forge::new(settings.within));
        for _ in 0..settings.restarts {
            referee.inject(Time::ZERO.saturating_add(settings.restart_at.draw(&mut rng)), Stimulus::Restart);
        }
        let model_seed = rng.next_u64();
        let mut world = World {
            now: Time::ZERO,
            rng: Rng::new(rng.next_u64()),
            model: Model::new(&settings.limits, deployment(), model_seed),
            stage: Stage::new(settings.limits, max_out, max_out + SLACK),
            life: 0,
            forge,
            forge_env: Env { now: Time::ZERO, limits: settings.forge },
            forge_out: Queue::with_capacity(16),
            wire: Schedule::new(),
            scheduled: 0,
            pending: Vec::new(),
            calls: Ledger::new("engine call"),
            owned: Ledger::new("sub-model call"),
            theirs: Ledger::new("person's call"),
            reads: Ledger::new("fresh read"),
            writes: Ledger::new("write"),
            parent: Parent::new(settings.parent, rng.next_u64()),
            people: People::new(settings.people, rng.next_u64()),
            referee,
            stats: Stats::default(),
            trace: Trace::default(),
            settings,
        };
        world.send(Time::ZERO, Delivery::People);
        world
    }

    #[must_use]
    pub fn now(&self) -> Time {
        self.now
    }

    #[must_use]
    pub fn stats(&self) -> Stats {
        Stats {
            parent: self.parent.tally(),
            people: self.people.tally(),
            forge: Some(self.forge.tally()),
            facts_lost: self.model.facts_lost(),
            ..self.stats.clone()
        }
    }

    /// What crossed between the sub-model and the world, in order, with
    /// times.
    #[must_use]
    pub fn trace(&self) -> &[String] {
        self.trace.lines()
    }

    /// Changes the referee saw reach the working set, and engine writes it
    /// judged.
    #[must_use]
    pub fn judged(&self) -> (u64, u64) {
        let forge = self.referee.expectations();
        (forge.reached, forge.writes)
    }

    /// Runs until people and the parent are done and nothing is in flight,
    /// then checks the invariants of a settled world. Panics if it takes more
    /// than `iterations`.
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
            let next = self.next_time().expect("the sub-model polls, so there is always a next time");
            assert!(next > self.now, "time moves forward");
            self.now = next;
        }
        panic!("seed {}: the world did not settle in {iterations} iterations", self.settings.seed);
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
                match stimulus {
                    Stimulus::Restart => self.restart(),
                }
            }
        }
        // The ready list first, at the start of the stage, then the events,
        // then the alarms.
        while self.stage.has_room() && self.model.is_ready() {
            sub::resume(&mut self.model, &self.stage.env, &mut self.stage.out);
        }
        while let Some(event) = self.stage.next_event() {
            self.log(format!("forge <- {}", describe(&event)));
            sub::step(&mut self.model, &self.stage.env, event, &mut self.stage.out);
        }
        while self.stage.has_room() && self.model.is_due(now) {
            sub::fire(&mut self.model, &self.stage.env, &mut self.stage.out);
        }
        while let Some(request) = self.stage.out.pop() {
            self.request(request);
        }
        while let Some(observation) = self.forge.pop_observation() {
            self.observe(Seen::Forge(observation));
        }
        while let Some(fact) = self.model.pop_fact() {
            self.stats.facts += 1;
            match fact {
                Fact::Found { .. } => self.end("found"),
                Fact::Sent { .. }
                | Fact::Failed { .. }
                | Fact::Limited { .. }
                | Fact::Spent { .. }
                | Fact::Admitted { .. }
                | Fact::Refused { .. }
                | Fact::Left { .. }
                | Fact::Listing { .. }
                | Fact::Retried { .. }
                | Fact::Wrote { .. }
                | Fact::Unwritten { .. }
                | Fact::Edited { .. } => {}
            }
        }
        // The reclaim point.
        self.model.reclaim();
        self.forge.reclaim();
        self.stats.peak = self.stats.peak.max(self.model.items());
    }

    /// Hands `delivery` to its destination.
    fn deliver(&mut self, delivery: Delivery) {
        match delivery {
            Delivery::Parent(action) => self.act(action),
            Delivery::People => {
                if let Some(act) = self.people.act(&self.forge, &self.settings.forge) {
                    self.person(act);
                }
                if !self.people.is_done() {
                    let gap = self.people.gap();
                    self.send(self.now.saturating_add(gap), Delivery::People);
                }
            }
            Delivery::Deadline(name) => {
                let life = self.life;
                let Some(out) = self.calls.get_mut(name) else {
                    return;
                };
                assert!(out.life == life, "a restart withdraws its calls' deadlines");
                out.expired = true;
                let call = out.call;
                self.end("timed out");
                self.answer(call, Err(sub::api::Error::Timeout));
            }
        }
    }

    /// The parent's action reaches the sub-model, or the world.
    fn act(&mut self, action: Action) {
        match action {
            Action::Model(event) => {
                match &event {
                    Event::Read { owner, .. } => self.reads.open(owner.raw(), ()),
                    Event::Untrack { item } => self.observe(Seen::Untracked { item: *item }),
                    Event::Track { .. }
                    | Event::Link { .. }
                    | Event::Took { .. }
                    | Event::Hint { .. }
                    | Event::Write { .. }
                    | Event::Answered { .. } => {}
                }
                self.stage.push(event);
            }
            Action::Write { owner, write, resumed, plan } => {
                self.observe(Seen::Planned { plan: owner, write: plan });
                self.writes.open(owner, ());
                self.stage.push(Event::Write { owner: Token::new(owner), write, resumed });
            }
            Action::Run(item) => {
                let actions = self.parent.run(item);
                self.schedule(actions);
            }
            Action::Release(item) => {
                let actions = self.parent.release(item);
                self.schedule(actions);
            }
            Action::Push { item, branch } => self.push(item, branch),
        }
    }

    /// Schedules the parent's actions.
    fn schedule(&mut self, actions: Vec<(Duration, Action)>) {
        for (delay, action) in actions {
            let at = self.now.saturating_add(delay);
            let key = self.send(at, Delivery::Parent(action));
            self.pending.push(key);
        }
    }

    /// A person acts.
    fn person(&mut self, act: Act) {
        match act {
            Act::Call { user, repository, op } => self.call(user, repository, op, Theirs::Person),
            Act::Push { user, repository, branch } => {
                let path = format!("work-{}", self.rng.below(1_000)).into_bytes();
                let pushed = forge::advance(
                    &mut self.forge,
                    &self.forge_env,
                    REPOSITORIES[repository],
                    &branch,
                    &path,
                    b"more",
                    user,
                );
                if pushed.is_err() {
                    self.end("push refused");
                }
            }
        }
    }

    /// A worker pushes the branch of `item`'s change: a commit on the
    /// default branch, pushed to a branch of its own.
    fn push(&mut self, item: Item, branch: Vec<u8>) {
        let repository = usize::try_from(item.repository).expect("few repositories");
        let name = REPOSITORIES[repository];
        let tip = self.forge.branch(name, MAIN).expect("a default branch");
        let mut files: Vec<File> = self
            .forge
            .object(tip)
            .expect("the default branch's commit")
            .tree
            .iter()
            .map(|(path, content)| File { path: path.clone(), content: content.clone() })
            .collect();
        files.push(File { path: branch.clone().into_boxed_slice(), content: b"a change".as_slice().into() });
        let Ok(Some(commit)) = forge::commit(&mut self.forge, &self.settings.forge, tip, files.into_boxed_slice())
        else {
            self.end("push refused");
            return;
        };
        let op = forge_api::Op::Git(Git::Push { branch: branch.clone().into_boxed_slice(), commit });
        self.call(WORKER, repository, op, Theirs::Push { item, branch });
    }

    /// A call of someone else's to the forge.
    fn call(&mut self, user: u64, repository: usize, op: forge_api::Op, theirs: Theirs) {
        let name = self.wire.name();
        self.theirs.open(name, theirs);
        let reply_to = ReplyTo::new(Token::new(name));
        let event = forge::Event::Call { reply_to, user, repository: REPOSITORIES[repository].into(), op };
        forge::step(&mut self.forge, &self.forge_env, event, &mut self.forge_out);
        self.drain_forge();
    }

    /// What the forge emitted: replies to their calls, and webhooks.
    fn drain_forge(&mut self) {
        while let Some(request) = self.forge_out.pop() {
            match request {
                forge::Request::Reply { to, result } => self.reply(to.into_token().raw(), result),
                forge::Request::Hook { repository, change: _, number, branch: _, commit } => {
                    let repository = REPOSITORIES.iter().position(|name| **name == *repository).expect("ours");
                    let repository = u32::try_from(repository).expect("few repositories");
                    let commit = commit.map(translate::commit);
                    self.stage.push(Event::Hint { repository, item: number, commit });
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
            let result = translate::answer(out.asked, result, &self.settings.limits);
            if let Err(sub::api::Error::RateLimited { reset }) = result {
                self.end("limited");
                self.observe(Seen::Limited { reset });
            }
            self.answer(out.call, result);
            return;
        }
        match self.theirs.end(name) {
            Theirs::Person => {}
            Theirs::Push { item, branch } => {
                if let Ok(forge_api::Answer::Pushed(forge_api::Pushed::Pushed)) = result {
                    let actions = self.parent.pushed(item, branch);
                    self.schedule(actions);
                }
            }
        }
    }

    /// Terminal for the sub-model's call `call`.
    fn answer(&mut self, call: Token, result: Result<sub::api::Answer, sub::api::Error>) {
        self.owned.end((self.life, call));
        self.stage.push(Event::Answered { call, result });
    }

    /// What the sub-model asked for.
    fn request(&mut self, request: Request) {
        self.log(format!("forge -> {}", describe_request(&request)));
        match request {
            Request::Call { call, repository, op } => {
                self.stats.sent += 1;
                self.observe(Seen::Sent);
                self.owned.open((self.life, call), ());
                let (asked, op) = translate::op(op, self.settings.limits.page, self.parent.fill());
                let name = self.wire.name();
                let deadline = self.send(self.now.saturating_add(self.settings.timeout), Delivery::Deadline(name));
                self.calls.open(name, Out { call, asked, life: self.life, deadline, expired: false });
                let reply_to = ReplyTo::new(Token::new(name));
                let repository = REPOSITORIES[usize::try_from(repository).expect("few")].into();
                let event = forge::Event::Call { reply_to, user: ENGINE, repository, op };
                forge::step(&mut self.forge, &self.forge_env, event, &mut self.forge_out);
                self.drain_forge();
            }
            told @ (Request::Read { .. }
            | Request::Wrote { .. }
            | Request::Full { .. }
            | Request::Announced { .. }
            | Request::Offered { .. }
            | Request::Inbox { .. }
            | Request::Changed { .. }
            | Request::Left { .. }
            | Request::Loaded) => {
                self.tell(&told);
                let actions = self.parent.told(&told);
                self.schedule(actions);
            }
        }
    }

    /// What the sub-model told its parent: counted, and seen by the referee.
    fn tell(&mut self, request: &Request) {
        match request {
            Request::Call { .. } => unreachable!("calls go to the forge"),
            Request::Read { owner, result } => {
                self.reads.end(owner.raw());
                self.end(if result.is_ok() { "read" } else { "read: failed" });
            }
            Request::Wrote { owner, result } => {
                self.writes.end(owner.raw());
                let edited = matches!(result, Err(Failure::Edited { .. }));
                self.observe(Seen::Wrote { plan: owner.raw(), written: result.is_ok(), edited });
                let ending = match result {
                    Ok(Written::Created(_)) => "wrote: created",
                    Ok(Written::Commented(_)) => "wrote: commented",
                    Ok(Written::Merged(_)) => "wrote: merged",
                    Ok(Written::Revision(_) | Written::Done) => "wrote",
                    Err(Failure::Edited { .. }) => "wrote: edited",
                    Err(Failure::Busy | Failure::Invalid | Failure::Unknown | Failure::Forge(_)) => "wrote: failed",
                };
                if let Err(failure) = result {
                    *self.stats.failures.entry(format!("{failure:?}")).or_default() += 1;
                }
                self.end(ending);
            }
            Request::Full { .. } => self.end("full"),
            Request::Announced { item, view } => {
                let ending = match view.record {
                    Record::Found { .. } => "announced: found",
                    Record::Missing => "announced: missing",
                    Record::Mangled { .. } => "announced: mangled",
                };
                self.end(ending);
                let labelled = view.labels.iter().any(|label| **label == *TRACKING || **label == *HAND_IN);
                if !labelled {
                    // Carrying neither label: found by the slow pass.
                    self.end("announced: unlabelled");
                }
                let labels = view.labels.iter().map(|label| label.to_vec()).collect();
                self.observe(Seen::Announced { item: *item, labels });
            }
            Request::Offered { .. } => self.end("offered"),
            Request::Inbox { item, seq: _, news } => {
                let (ending, comment) = match news {
                    News::Comment { id, .. } => ("news: comment", Some(*id)),
                    News::Review { .. } => ("news: review", None),
                    News::Pull { .. } => ("news: pull", None),
                };
                self.end(ending);
                self.observe(Seen::News { item: *item, comment });
            }
            Request::Changed { item, labels } => {
                self.end("changed");
                let labels = labels.iter().map(|label| label.to_vec()).collect();
                self.observe(Seen::Changed { item: *item, labels });
            }
            Request::Left { item } => {
                self.end("left");
                self.observe(Seen::Left { item: *item });
            }
            Request::Loaded => {
                self.end("loaded");
                self.observe(Seen::Loaded);
            }
        }
    }

    /// The engine restarts: a new sub-model, starting cold, and a parent
    /// that remembers only what the forge holds. The calls the old one had
    /// out still reach the forge; their answers are dropped.
    fn restart(&mut self) {
        self.stats.restarts += 1;
        self.log("the engine restarts".to_owned());
        self.life += 1;
        let seed = self.rng.next_u64();
        self.model = Model::new(&self.settings.limits, deployment(), seed);
        let max_out = sub::max_out(&self.settings.limits);
        self.stage = Stage::new(self.settings.limits, max_out, max_out + SLACK);
        self.stage.tick(self.now);
        let keys: Vec<Key> = self.calls.values().map(|out| out.deadline).collect();
        for key in keys {
            self.withdraw(key);
        }
        for key in std::mem::take(&mut self.pending) {
            self.withdraw(key);
        }
        self.owned = Ledger::new("sub-model call");
        self.reads = Ledger::new("fresh read");
        self.writes = Ledger::new("write");
        self.parent.restart();
        self.observe(Seen::Restarted);
    }

    /// Schedules `delivery` at `at`, within the world's bound.
    fn send(&mut self, at: Time, delivery: Delivery) -> Key {
        self.scheduled += 1;
        assert!(
            self.scheduled <= DELIVERIES,
            "seed {}: more deliveries scheduled than the world holds",
            self.settings.seed
        );
        self.wire.send(at, delivery)
    }

    /// Withdraws the delivery of `key`, if it is still in flight.
    fn withdraw(&mut self, key: Key) {
        if self.wire.withdraw(key).is_some() {
            self.scheduled -= 1;
        }
    }

    /// Logs `line` in the trace, within the world's bound.
    fn log(&mut self, line: String) {
        assert!(self.trace.lines().len() < TRACE, "seed {}: the trace grew past its bound", self.settings.seed);
        self.trace.log(self.now, line);
    }

    fn end(&mut self, ending: &'static str) {
        *self.stats.endings.entry(ending).or_default() += 1;
    }

    /// The referee observes `seen`, which ends the test if it breaks an
    /// expectation; what it injects at once is injected before the world
    /// goes on.
    fn observe(&mut self, seen: Seen) {
        let mut stimuli = Vec::new();
        self.referee.observe(self.now, seen, &mut stimuli);
        self.referee.assert_holding(self.settings.seed);
        for stimulus in stimuli {
            match stimulus {
                Stimulus::Restart => self.restart(),
            }
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

    /// Whether the world has settled between the sub-model's passes: people
    /// done, the parent waiting on nothing, nothing in flight anywhere, and
    /// the referee expecting nothing more.
    fn is_quiet(&self) -> bool {
        self.people.is_done()
            && self.wire.is_empty()
            && !self.parent.is_waiting()
            && self.model.calls() == 0
            && self.model.reads() == 0
            && self.model.writes() == 0
            && self.forge.calls() == 0
            && self.forge.deliveries() == 0
            && !matches!(self.referee.verdict(), temper_world::Verdict::Open { .. })
    }

    /// The invariants of a settled world.
    fn assert_settled(&mut self) {
        let seed = self.settings.seed;
        self.calls.assert_settled();
        self.owned.assert_settled();
        self.theirs.assert_settled();
        self.reads.assert_settled();
        self.writes.assert_settled();
        assert_eq!(self.model.calls_out(), 0, "seed {seed}: no call out");
        let tally = self.forge.tally();
        assert_eq!(tally.forgotten, 0, "seed {seed}: the forge kept every call it took: {tally:?}");
        self.observe(Seen::Settled);
        self.referee.assert_passed(seed);
    }
}

/// The deployment's forge configuration.
fn deployment() -> Deployment {
    Deployment { engine: ENGINE, tracking: TRACKING.into(), hand_in: HAND_IN.into() }
}

fn describe(event: &Event) -> String {
    match event {
        Event::Answered { call, result } => match result {
            Ok(answer) => format!("answered {} {}", call.raw(), describe_answer(answer)),
            Err(error) => format!("answered {} {error:?}", call.raw()),
        },
        Event::Track { .. }
        | Event::Untrack { .. }
        | Event::Link { .. }
        | Event::Took { .. }
        | Event::Hint { .. }
        | Event::Read { .. }
        | Event::Write { .. } => format!("{event:?}"),
    }
}

fn describe_answer(answer: &sub::api::Answer) -> String {
    match answer {
        sub::api::Answer::Items { items, more } => {
            let numbers: Vec<u64> = items.iter().map(|item| item.number).collect();
            format!("items {numbers:?} more {more}")
        }
        sub::api::Answer::Item { item, comments, more } => {
            let ids: Vec<u64> = comments.iter().map(|comment| comment.id).collect();
            format!("item {} comments {ids:?} more {more}", item.number)
        }
        sub::api::Answer::Comment(_)
        | sub::api::Answer::Pull(_)
        | sub::api::Answer::Statuses(_)
        | sub::api::Answer::Permission(_)
        | sub::api::Answer::Commit(_)
        | sub::api::Answer::Pages { .. }
        | sub::api::Answer::Page(_)
        | sub::api::Answer::Created(_)
        | sub::api::Answer::Commented { .. }
        | sub::api::Answer::Edited { .. }
        | sub::api::Answer::Merged(_)
        | sub::api::Answer::Revision(_)
        | sub::api::Answer::Done => format!("{answer:?}"),
    }
}

fn describe_request(request: &Request) -> String {
    format!("{request:?}")
}
