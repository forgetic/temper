use std::collections::{BTreeMap, VecDeque};

use skein_lib::{Duration, Rng, Time, Token};
use temper_engine_domain_views::{
    self as views, Capture, Domain, Dropped, End, Event, Fact, Kept, Kind, Limits, Lost, Policy, Record, Refusal,
    Request, Subject,
};
use temper_world::{Ledger, Referee, Schedule, Span, Stage, Trace};

use crate::referee::{Bounds, Seen, Stimulus, Views, copy, copy_record};

/// Room in the views' output queue beyond what one step may emit.
const SLACK: u32 = 2;

/// The most lines a trace keeps, and deliveries in flight: a world past
/// either fails with its seed rather than grow.
const MAX_TRACE: usize = 400_000;
const MAX_WIRE: usize = 4096;

/// Every kind of report, and every capture.
pub const KINDS: [Kind; 5] = [Kind::Text, Kind::Progress, Kind::Call, Kind::Tool, Kind::Usage];
const CAPTURES: [Capture; 3] = [Capture::Nothing, Capture::Shape, Capture::Content];

/// The phases the parent's codes stand for.
const PHASES: u32 = 8;

/// The views' limits in the calm world: room for every run and watch, a
/// backlog a few chunks long, batches of a few reports.
pub const LIMITS: Limits = Limits {
    runs: 6,
    watchers: 16,
    backlog: 4,
    report_bytes: 48,
    snapshot_bytes: 64,
    records: 8,
    batch_bytes: 256,
    appends: 2,
    flush: Duration::from_millis(500),
    retention: Duration::from_secs(30),
    sweep: Duration::from_secs(5),
    facts: 64,
};

/// How the world's watches, deliveries, reports and store operations ended,
/// for the sweep.
pub const ENDINGS: [&str; 24] = [
    "delivered",
    "closed",
    "burst",
    "missed",
    "undelivered",
    "hung",
    "busy",
    "unknown",
    "unfollowed",
    "oversized snapshot",
    "unwatched",
    "finished",
    "appended",
    "append failed",
    "partly kept",
    "expired",
    "expire failed",
    "at once",
    "oversized",
    "late",
    "lost",
    "rerun",
    "reused",
    "restarted",
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Settings {
    /// Seeds the world.
    pub seed: u64,
    pub limits: Limits,
    /// The runs the parent starts and the time between starts, a run of an
    /// item with one live starting it again for a new attempt; the most
    /// reports a run makes, and the time between them; per mille reports
    /// past the limits, and runs that report once more after they finished.
    pub runs: u32,
    pub run_gap: Span,
    pub reports: u32,
    pub report_gap: Span,
    pub oversized: u32,
    pub late: u32,
    /// Per mille reports that come several at once.
    pub report_burst: u32,
    /// The items and the repositories they are in; the phase changes in all,
    /// and the time between them.
    pub items: u32,
    pub repositories: u32,
    pub changes: u32,
    pub change_gap: Span,
    /// The times people start watching, the time between them, per mille
    /// those when several start at once, and how long each watches; the
    /// tokens they reuse; their streams' latency, per mille slow ones and
    /// theirs, per mille deliveries a stream does not take, and those it
    /// hangs on until the parent's bound; and per mille watches of a run
    /// that is not followed.
    pub watches: u32,
    pub watch_gap: Span,
    pub burst: u32,
    pub watch_for: Span,
    pub people: u32,
    pub latency: Span,
    pub slow: u32,
    pub slow_latency: Span,
    pub undelivered: u32,
    pub hang: u32,
    pub delivery_bound: Duration,
    /// Per mille people whose stream is closed already: the parent answers
    /// their first delivery as not taken, and stops their watch, at once.
    pub closed: u32,
    pub stale: u32,
    /// How long the store takes over an operation, per mille those that
    /// fail, and those it answers within the iteration.
    pub store_latency: Span,
    pub store_failures: u32,
    pub instant: u32,
    /// Restarts of the engine, each at a moment drawn.
    pub restarts: u32,
    pub restart_at: Span,
}

impl Settings {
    /// A world where nothing goes wrong: runs and watches within the limits,
    /// people who keep up, a store that is quick and never fails, an engine
    /// that never restarts.
    #[must_use]
    pub fn calm(seed: u64) -> Settings {
        Settings {
            seed,
            limits: LIMITS,
            runs: 16,
            run_gap: Span::millis(1000, 5000),
            reports: 60,
            report_gap: Span::millis(20, 300),
            oversized: 0,
            late: 0,
            report_burst: 0,
            items: 12,
            repositories: 2,
            changes: 40,
            change_gap: Span::millis(100, 2000),
            watches: 20,
            watch_gap: Span::millis(200, 3000),
            burst: 0,
            watch_for: Span::millis(5000, 30_000),
            people: 0,
            latency: Span::millis(1, 10),
            slow: 0,
            slow_latency: Span::millis(0, 0),
            undelivered: 0,
            hang: 0,
            delivery_bound: Duration::from_secs(5),
            closed: 0,
            stale: 0,
            store_latency: Span::millis(1, 50),
            store_failures: 0,
            instant: 0,
            restarts: 0,
            restart_at: Span::millis(0, 0),
        }
    }

    /// A world of its own for `seed`: runs that report fast, past the
    /// limits, late and again; more runs and watches than the limits hold,
    /// some in bursts and under tokens reused; people who are slow, whose
    /// streams fail and hang; a store that is slow, fails and answers at
    /// once; an engine that restarts, at chances drawn from the seed.
    #[must_use]
    pub fn random(seed: u64) -> Settings {
        let mut rng = Rng::new(seed ^ 0x5EE5_0000_0000_5EED);
        let mut chance = |most: u64| u32::try_from(rng.below(most + 1)).expect("a chance per mille");
        let calm = Settings::calm(seed);
        Settings {
            runs: 4 + chance(16),
            run_gap: Span::millis(1, 100 + u64::from(chance(5000))),
            reports: 1 + chance(60),
            report_gap: Span::millis(1, 5 + u64::from(chance(500))),
            oversized: chance(100),
            late: chance(300),
            report_burst: chance(300),
            items: 1 + chance(8),
            repositories: 1 + chance(3),
            changes: chance(80),
            change_gap: Span::millis(1, 10 + u64::from(chance(3000))),
            watches: 4 + chance(30),
            watch_gap: Span::millis(1, 10 + u64::from(chance(3000))),
            burst: chance(300),
            watch_for: Span::millis(1, 100 + u64::from(chance(30_000))),
            people: chance(6),
            latency: Span::millis(1, 1 + u64::from(chance(50))),
            slow: chance(500),
            slow_latency: Span::millis(50, 100 + u64::from(chance(3000))),
            undelivered: chance(100),
            hang: chance(50),
            closed: chance(200),
            stale: chance(150),
            store_latency: Span::millis(1, 5 + u64::from(chance(2000))),
            store_failures: chance(300),
            instant: chance(500),
            restarts: chance(3),
            restart_at: Span::millis(0, 60_000),
            limits: Limits {
                runs: 1 + chance(5),
                watchers: 1 + chance(7),
                backlog: 1 + chance(5),
                records: 1 + chance(8),
                appends: 1 + chance(2),
                flush: Duration::from_millis(u64::from(chance(2000))),
                retention: Duration::from_millis(100 + u64::from(chance(40_000))),
                sweep: Duration::from_millis(50 + u64::from(chance(10_000))),
                ..LIMITS
            },
            ..calm
        }
    }

    /// How long the views have to do what the referee expects.
    #[must_use]
    pub fn bounds(&self) -> Bounds {
        let tick = Duration::from_millis(1);
        let reach = self.latency.max.max(self.slow_latency.max).max(self.delivery_bound).saturating_add(tick);
        let end = reach.saturating_mul(2);
        // The store comes to an expire behind every append in flight.
        let store = self.store_latency.max.saturating_mul(u64::from(self.limits.appends) + 2);
        let forget = self.limits.sweep.saturating_add(store.saturating_mul(2)).saturating_add(tick);
        Bounds { reach, end, forget }
    }
}

/// What the world counted, as it crossed the boundary.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct Stats {
    /// The runs started, reports made, watches started and phase changes.
    pub runs: u32,
    pub reports: u32,
    pub watches: u32,
    pub changes: u32,
    /// How watches, deliveries, reports and the store's operations ended.
    pub endings: BTreeMap<&'static str, u32>,
    /// The views' facts.
    pub facts: u32,
}

/// Something on its way, delivered at its time.
#[derive(Clone, Copy, Debug)]
enum Delivery {
    /// The parent starts its next run.
    Start,
    /// The run of the item at `item` reports its next report, or finishes.
    Report { item: usize },
    /// That run, finished, reports once more: a fact that came late.
    Late { item: usize },
    /// An item's phase changes.
    Change,
    /// People start watching.
    Watch,
    /// The person stops the watch the world numbered `watch`, while the
    /// engine lives its life `life`.
    Unwatch { watcher: u64, watch: u64, life: u64 },
    /// The person's stream has taken its delivery, or has not.
    Delivered { watcher: u64, done: bool, life: u64 },
    /// The store is done with the operation in hand.
    Stored,
}

/// A run the parent plays, of an item: its attempt and policy, and the
/// reports it has made and has left to make.
#[derive(Debug)]
struct Script {
    attempt: u64,
    policy: Policy,
    reported: u32,
    left: u32,
}

/// A person watching: their stream's latency, and whether it is closed
/// already, and stopped.
#[derive(Debug)]
struct Person {
    latency: Span,
    closed: bool,
    stopped: bool,
}

/// An item, by the parent's token, which names its runs too; its repository,
/// and the attempts its runs have had.
#[derive(Debug)]
struct Item {
    token: u64,
    repository: u32,
    attempts: u64,
}

/// An operation on the store, by the views' owner token, asked by the
/// engine in its life `life`.
#[derive(Debug)]
enum Op {
    /// An append, which the world names `name`.
    Append {
        owner: Token,
        name: u64,
        records: Box<[Record]>,
        life: u64,
    },
    Expire {
        owner: Token,
        before: Time,
        life: u64,
    },
}

pub struct World {
    now: Time,
    rng: Rng,
    settings: Settings,

    domain: Domain,
    stage: Stage<Limits, Event, Request>,
    /// The engine's lives: each restart starts another.
    life: u64,

    /// Deliveries in flight, and their count; the count names watches and
    /// appends too.
    wire: Schedule<Delivery>,
    in_flight: usize,

    /// The parent's script: runs left to start, and those reporting, by
    /// their item; the items, and the changes left; the watches left to
    /// start, and each person's stream latency.
    runs_left: u32,
    runs: BTreeMap<usize, Script>,
    items: Vec<Item>,
    changes_left: u32,
    watches_left: u32,
    people: BTreeMap<u64, Person>,

    /// The watches from their watch to their end, with whether each was
    /// answered and the world's number for it; the deliveries in flight, by
    /// watcher; and the store's operations, by owner.
    watches: Ledger<u64, (bool, u64)>,
    deliveries: Ledger<u64, ()>,
    ops: Ledger<u64, ()>,

    /// The store: what it has yet to do, in order, whether it is doing it,
    /// and the records it keeps.
    queue: VecDeque<Op>,
    busy: bool,
    store: Vec<Record>,

    /// What the views lost in their earlier lives, the records in their
    /// batches when they restarted, and the records sent to the store and
    /// in appends that failed, as the views learnt it.
    lost: Lost,
    batched: u64,
    sent: u64,
    failed: u64,

    referee: Referee<Views>,
    stats: Stats,
    trace: Trace,
}

impl World {
    #[must_use]
    pub fn new(settings: Settings) -> World {
        assert!(views::worst_case(&settings.limits).is_some(), "the shell refuses limits it cannot provision");
        let max_out = views::max_out(&settings.limits);
        let items = (0..settings.items)
            .map(|index| Item {
                token: 1_000_000 + u64::from(index),
                repository: index % settings.repositories,
                attempts: 0,
            })
            .collect();
        let mut rng = Rng::new(settings.seed);
        let mut referee = Referee::new(Views::new(settings.limits, settings.bounds()));
        for _ in 0..settings.restarts {
            referee.inject(Time::ZERO.saturating_add(settings.restart_at.draw(&mut rng)), Stimulus::Restart);
        }
        let mut world = World {
            now: Time::ZERO,
            rng,
            domain: Domain::new(&settings.limits, Time::ZERO),
            stage: Stage::new(settings.limits, max_out, max_out + SLACK),
            life: 0,
            wire: Schedule::new(),
            in_flight: 0,
            runs_left: settings.runs,
            runs: BTreeMap::new(),
            items,
            changes_left: settings.changes,
            watches_left: settings.watches,
            people: BTreeMap::new(),
            watches: Ledger::new("watch"),
            deliveries: Ledger::new("delivery"),
            ops: Ledger::new("store operation"),
            queue: VecDeque::new(),
            busy: false,
            store: Vec::new(),
            lost: Lost { runs: 0, reports: 0, chunks: 0, records: 0, facts: 0 },
            batched: 0,
            sent: 0,
            failed: 0,
            referee,
            stats: Stats::default(),
            trace: Trace::default(),
            settings,
        };
        if settings.runs > 0 && settings.items > 0 {
            world.send(Time::ZERO, Delivery::Start);
        }
        if settings.changes > 0 && settings.items > 0 {
            world.send(Time::ZERO, Delivery::Change);
        }
        if settings.watches > 0 {
            world.send(Time::ZERO, Delivery::Watch);
        }
        world
    }

    #[must_use]
    pub fn now(&self) -> Time {
        self.now
    }

    #[must_use]
    pub fn stats(&self) -> Stats {
        self.stats.clone()
    }

    /// What crossed between the views and the world, in order, with times.
    #[must_use]
    pub fn trace(&self) -> &[String] {
        self.trace.lines()
    }

    /// Chunks, deliveries, missed chunks and records the referee judged.
    #[must_use]
    pub fn judged(&self) -> (u64, u64, u64, u64) {
        let views = self.referee.expectations();
        (views.chunks, views.deliveries, views.missed, views.records)
    }

    /// Runs until nothing is left to happen, then checks the invariants of a
    /// settled world. Panics, with the seed, if it takes more than
    /// `iterations`.
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
        panic!("seed {}: the world did not settle in {iterations} iterations", self.settings.seed);
    }

    /// One iteration of the loop, as the shell would run it. The parent
    /// routes what the views ask for as each step emits it, so the store may
    /// answer within the iteration.
    fn iterate(&mut self) {
        let now = self.now;
        self.stage.tick(now);
        while let Some(delivery) = self.wire.next(now) {
            self.in_flight -= 1;
            self.deliver(delivery);
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
        while let Some(event) = self.stage.next_event() {
            self.log(format!("views <- {}", describe(&event)));
            if let Some(seen) = taken(&event) {
                self.observe(seen);
            }
            views::step(&mut self.domain, &self.stage.env, event, &mut self.stage.out);
            self.route();
        }
        while self.stage.has_room() && self.domain.is_due(now) {
            views::fire(&mut self.domain, &self.stage.env, &mut self.stage.out);
            self.route();
        }
        while let Some(fact) = self.domain.pop_fact() {
            self.stats.facts += 1;
            match fact {
                Fact::Reported { kept: Kept::Lost, .. } => self.end("lost"),
                Fact::Followed
                | Fact::Unfollowed
                | Fact::Reported { kept: Kept::Nothing | Kept::Shape | Kept::Content, .. }
                | Fact::Turn { .. }
                | Fact::Dropped { dropped: Dropped::Unfollowed | Dropped::Oversized }
                | Fact::Changed { .. }
                | Fact::Watching
                | Fact::Refused { .. }
                | Fact::Ended { .. }
                | Fact::Overflowed { .. }
                | Fact::Delivered { .. }
                | Fact::Undelivered { .. }
                | Fact::Appending { .. }
                | Fact::Appended { .. }
                | Fact::Expiring
                | Fact::Expired { .. } => {}
            }
        }
        // The reclaim point.
        self.domain.reclaim();
    }

    /// The referee sees what a step or an alarm emitted, as it leaves the
    /// views, and the parent routes it on.
    fn route(&mut self) {
        while let Some(request) = self.stage.out.pop() {
            if let Some(seen) = emitted(&request) {
                self.observe(seen);
            }
            self.request(request);
        }
    }

    /// Hands `delivery` to its destination.
    fn deliver(&mut self, delivery: Delivery) {
        match delivery {
            Delivery::Start => self.start(),
            Delivery::Report { item } => self.report(item),
            Delivery::Late { item } => {
                // The parent passes on only what the attempt it claims
                // reports: not this, once the item runs again.
                if !self.runs.contains_key(&item) {
                    self.end("late");
                    let token = self.items[item].token;
                    let content = self.content(token, 0, 0);
                    self.stage.push(Event::Reported { run: Token::new(token), kind: Kind::Progress, content });
                }
            }
            Delivery::Change => self.change(),
            Delivery::Watch => self.watch(),
            Delivery::Unwatch { watcher, watch, life } => {
                // The parent forgets a watch once it has ended, and every
                // watch once the engine restarts.
                if life == self.life && self.watches.get(watcher) == Some(&(true, watch)) {
                    self.stage.push(Event::Unwatch { watcher: Token::new(watcher) });
                }
            }
            Delivery::Delivered { watcher, done, life } => {
                if life == self.life {
                    self.deliveries.end(watcher);
                    self.stage.push(Event::Delivered { watcher: Token::new(watcher), done });
                }
            }
            Delivery::Stored => self.stored(false),
        }
    }

    /// The parent starts a run of an item drawn, under a policy drawn: a new
    /// one, or, if the item has one live, the same run again for a new
    /// attempt.
    fn start(&mut self) {
        self.runs_left -= 1;
        if self.runs_left > 0 {
            let at = self.now.saturating_add(self.settings.run_gap.draw(&mut self.rng));
            self.send(at, Delivery::Start);
        }
        self.stats.runs += 1;
        let index = self.pick(self.items.len());
        let mut capture = || CAPTURES[usize::try_from(self.rng.below(3)).expect("small")];
        let policy =
            Policy { text: capture(), progress: capture(), calls: capture(), tools: capture(), usage: capture() };
        let item = &mut self.items[index];
        item.attempts += 1;
        let (token, attempt) = (item.token, item.attempts);
        if let Some(script) = self.runs.get_mut(&index) {
            // Its reports go on, under the new attempt.
            script.attempt = attempt;
            script.policy = policy;
            self.end("rerun");
        } else {
            let left = 1 + self.below(self.settings.reports);
            self.runs.insert(index, Script { attempt, policy, reported: 0, left });
            let at = self.now.saturating_add(self.settings.report_gap.draw(&mut self.rng));
            self.send(at, Delivery::Report { item: index });
        }
        let run = Token::new(token);
        self.stage.push(Event::Started { run, attempt: Token::new(attempt), item: run, policy });
    }

    /// The run reports, once or several times at once, of kinds drawn, past
    /// the limits now and then; or, with nothing left to report, finishes.
    fn report(&mut self, index: usize) {
        if self.rng.chance(self.settings.report_burst) {
            self.end("burst");
            for _ in 0..=self.pick(4) {
                if self.runs.get(&index).is_some_and(|script| script.left > 0) {
                    self.report_one(index, false);
                }
            }
        }
        self.report_one(index, true);
    }

    /// The run reports once, or finishes; then, if `next`, its next report is
    /// on its way.
    fn report_one(&mut self, index: usize, next: bool) {
        let token = self.items[index].token;
        let script = self.runs.get_mut(&index).expect("a run reporting is in the script");
        if script.left == 0 {
            self.runs.remove(&index);
            self.stage.push(Event::Finished { run: Token::new(token) });
            if self.rng.chance(self.settings.late) {
                let at = self.now.saturating_add(self.settings.report_gap.draw(&mut self.rng));
                self.send(at, Delivery::Late { item: index });
            }
            return;
        }
        script.left -= 1;
        script.reported += 1;
        let (attempt, reported) = (script.attempt, script.reported);
        self.stats.reports += 1;
        let kind = KINDS[self.pick(KINDS.len())];
        let content = self.content(token, attempt, reported);
        if content.len() > self.settings.limits.report_bytes as usize {
            self.end("oversized");
        }
        self.stage.push(Event::Reported { run: Token::new(token), kind, content });
        if next {
            let at = self.now.saturating_add(self.settings.report_gap.draw(&mut self.rng));
            self.send(at, Delivery::Report { item: index });
        }
    }

    /// A report's content: its run, attempt and number, which make it unique,
    /// then letters, within the limits or, now and then, past them.
    fn content(&mut self, run: u64, attempt: u64, number: u32) -> Box<[u8]> {
        let content = format!("r{run}a{attempt}#{number}:").into_bytes();
        let most = self.settings.limits.report_bytes as usize;
        self.fill(content, most)
    }

    /// `content` filled with letters to a length drawn up to `most`, or now
    /// and then past it.
    fn fill(&mut self, mut content: Vec<u8>, most: usize) -> Box<[u8]> {
        let len = if self.rng.chance(self.settings.oversized) {
            most + 1 + self.pick(16)
        } else {
            content.len() + self.pick(most.saturating_sub(content.len()) + 1)
        };
        while content.len() < len {
            content.push(b"abcdefghij"[self.pick(10)]);
        }
        content.into()
    }

    /// An item drawn goes to a phase drawn.
    fn change(&mut self) {
        self.changes_left -= 1;
        if self.changes_left > 0 {
            let at = self.now.saturating_add(self.settings.change_gap.draw(&mut self.rng));
            self.send(at, Delivery::Change);
        }
        self.stats.changes += 1;
        let index = self.pick(self.items.len());
        let phase = self.below(PHASES);
        let item = &self.items[index];
        let event = Event::Phase { item: Token::new(item.token), repository: item.repository, phase };
        self.stage.push(event);
    }

    /// People start watching, one or several at once, and stop together
    /// after a time drawn.
    fn watch(&mut self) {
        self.watches_left -= 1;
        if self.watches_left > 0 {
            let at = self.now.saturating_add(self.settings.watch_gap.draw(&mut self.rng));
            self.send(at, Delivery::Watch);
        }
        let count = if self.rng.chance(self.settings.burst) { 2 + self.pick(3) } else { 1 };
        let watch_for = self.settings.watch_for.draw(&mut self.rng);
        for _ in 0..count {
            self.watch_one(watch_for);
        }
    }

    /// A person starts watching a run, an item or a board, drawn; now and
    /// then a run that is not followed; under a token people reuse once its
    /// watch has ended, if one is free.
    fn watch_one(&mut self, watch_for: Duration) {
        self.stats.watches += 1;
        let free = (1..=u64::from(self.settings.people)).find(|token| !self.watches.contains(*token));
        let watcher = if let Some(token) = free {
            self.end("reused");
            token
        } else {
            self.wire.name() + 1000
        };
        let live: Vec<usize> = self.runs.keys().copied().collect();
        let subject = if self.rng.chance(self.settings.stale) {
            let idle: Vec<u64> = (0..self.items.len())
                .filter(|index| !self.runs.contains_key(index))
                .map(|index| self.items[index].token)
                .collect();
            match idle.len() {
                0 => Subject::Run(Token::new(u64::MAX)),
                len => Subject::Run(Token::new(idle[self.pick(len)])),
            }
        } else {
            match self.pick(3) {
                0 if !live.is_empty() => {
                    let index = live[self.pick(live.len())];
                    Subject::Run(Token::new(self.items[index].token))
                }
                0 | 1 if !self.items.is_empty() => {
                    let index = self.pick(self.items.len());
                    Subject::Item(Token::new(self.items[index].token))
                }
                _ => Subject::Board(self.below(self.settings.repositories + 1)),
            }
        };
        let snapshot = self.fill(format!("s{watcher}:").into_bytes(), self.settings.limits.snapshot_bytes as usize);
        let latency =
            if self.rng.chance(self.settings.slow) { self.settings.slow_latency } else { self.settings.latency };
        let watch = self.wire.name();
        let closed = self.rng.chance(self.settings.closed);
        self.people.insert(watcher, Person { latency, closed, stopped: false });
        self.watches.open(watcher, (false, watch));
        self.stage.push(Event::Watch { watcher: Token::new(watcher), subject, snapshot });
        let unwatch = Delivery::Unwatch { watcher, watch, life: self.life };
        self.send(self.now.saturating_add(watch_for), unwatch);
    }

    /// The engine restarts: new views, starting at `now`; every watch is
    /// gone with its stream, and what was on its way to the old views is
    /// lost; the store keeps what it holds, and the parent tells the new
    /// views of the runs that are live.
    fn restart(&mut self) {
        self.end("restarted");
        self.log("the engine restarts".to_owned());
        let lost = self.domain.lost();
        self.lost.runs += lost.runs;
        self.lost.reports += lost.reports;
        self.lost.chunks += lost.chunks;
        self.lost.records += lost.records;
        self.batched += u64::from(self.domain.batched());
        self.life += 1;
        self.domain = Domain::new(&self.settings.limits, self.now);
        let max_out = views::max_out(&self.settings.limits);
        self.stage = Stage::new(self.settings.limits, max_out, max_out + SLACK);
        self.stage.tick(self.now);
        self.watches = Ledger::new("watch");
        self.deliveries = Ledger::new("delivery");
        self.ops = Ledger::new("store operation");
        self.people.clear();
        self.observe(Seen::Restarted);
        for (&index, script) in &self.runs {
            let (run, attempt) = (Token::new(self.items[index].token), Token::new(script.attempt));
            self.stage.push(Event::Started { run, attempt, item: run, policy: script.policy });
        }
    }

    /// Takes `request` from the views' queue: an answer or an end to a
    /// watch, a delivery to a person's stream, or an operation on the store.
    fn request(&mut self, request: Request) {
        self.log(format!("views -> {}", describe_request(&request)));
        match request {
            Request::Watching { watcher } => {
                let (answered, _) = self.watches.get_mut(watcher.raw()).expect("a watch is answered while open");
                assert!(!*answered, "a watch is answered once");
                *answered = true;
            }
            Request::Refused { watcher, refusal } => {
                let (answered, _) = self.watches.end(watcher.raw());
                assert!(!answered, "a watch is answered once");
                self.people.remove(&watcher.raw());
                self.end(match refusal {
                    Refusal::Busy => "busy",
                    Refusal::Unknown => "unknown",
                    Refusal::Unfollowed => "unfollowed",
                    Refusal::Oversized => "oversized snapshot",
                });
            }
            Request::Deliver { watcher, missed, chunks } => {
                assert!(!chunks.is_empty(), "a delivery delivers something");
                assert!(
                    self.watches.get(watcher.raw()).is_some_and(|(answered, _)| *answered),
                    "a delivery goes to a watch that is open"
                );
                self.deliveries.open(watcher.raw(), ());
                self.end("delivered");
                if missed > 0 {
                    self.end("missed");
                }
                let person = self.people.get_mut(&watcher.raw()).expect("a delivery goes to a person watching");
                if person.closed {
                    // The parent answers at once, within its step, before it
                    // takes anything else, and stops the watch.
                    let stop = !std::mem::replace(&mut person.stopped, true);
                    self.end("closed");
                    self.deliveries.end(watcher.raw());
                    if stop {
                        self.stage.inbox.push_front(Event::Unwatch { watcher });
                    }
                    self.stage.inbox.push_front(Event::Delivered { watcher, done: false });
                    return;
                }
                let life = self.life;
                let latency = person.latency.draw(&mut self.rng);
                let (at, done) = if self.rng.chance(self.settings.hang) {
                    self.end("hung");
                    (self.settings.delivery_bound, false)
                } else if self.rng.chance(self.settings.undelivered) {
                    self.end("undelivered");
                    (latency, false)
                } else {
                    (latency, true)
                };
                self.send(self.now.saturating_add(at), Delivery::Delivered { watcher: watcher.raw(), done, life });
            }
            Request::Ended { watcher, end } => {
                let (answered, _) = self.watches.end(watcher.raw());
                assert!(answered, "a watch ends once it was taken");
                assert!(!self.deliveries.contains(watcher.raw()), "a watch ends once its delivery has");
                self.people.remove(&watcher.raw());
                self.end(match end {
                    End::Unwatched => "unwatched",
                    End::Finished => "finished",
                });
            }
            Request::Append { owner, records } => {
                self.ops.open(owner.raw(), ());
                self.sent += u64::try_from(records.len()).expect("small");
                let name = self.wire.name();
                self.ask(Op::Append { owner, name, records, life: self.life });
            }
            Request::Expire { owner, before } => {
                self.ops.open(owner.raw(), ());
                self.ask(Op::Expire { owner, before, life: self.life });
            }
        }
    }

    /// The store takes an operation: at once, within the iteration, now and
    /// then if it has nothing else to do; otherwise after the others, and a
    /// latency.
    fn ask(&mut self, op: Op) {
        self.queue.push_back(op);
        if !self.busy && self.queue.len() == 1 && self.rng.chance(self.settings.instant) {
            self.end("at once");
            self.busy = true;
            self.stored(true);
        } else {
            self.serve();
        }
    }

    /// The store's answer goes to the views: `at_once`, within the parent's
    /// step, before anything else; otherwise after what is on its way.
    fn answer(&mut self, event: Event, at_once: bool) {
        if at_once {
            self.stage.inbox.push_front(event);
        } else {
            self.stage.push(event);
        }
    }

    /// The store takes its next operation, if it is idle.
    fn serve(&mut self) {
        if self.busy || self.queue.is_empty() {
            return;
        }
        self.busy = true;
        let at = self.now.saturating_add(self.settings.store_latency.draw(&mut self.rng));
        self.send(at, Delivery::Stored);
    }

    /// The store is done with the operation in hand: it kept or forgot what
    /// it was asked to, or failed, an append keeping only its first records.
    /// Its answer reaches the views it was asked by, not those after a
    /// restart; one `at_once` before anything else the parent has for them.
    fn stored(&mut self, at_once: bool) {
        self.busy = false;
        let op = self.queue.pop_front().expect("the store does what it was asked");
        let failed = self.rng.chance(self.settings.store_failures);
        match op {
            Op::Append { owner, name, records, life } => {
                let mut records = records.into_vec();
                let count = u64::try_from(records.len()).expect("small");
                if failed {
                    let kept = self.pick(records.len() + 1);
                    records.truncate(kept);
                    self.end("append failed");
                    if kept > 0 {
                        self.end("partly kept");
                    }
                } else {
                    self.end("appended");
                }
                let kept = records.iter().map(copy_record).collect();
                self.store.extend(records);
                self.observe(Seen::Kept { append: name, records: kept });
                if life == self.life {
                    if failed {
                        self.failed += count;
                    }
                    self.ops.end(owner.raw());
                    self.answer(Event::Appended { owner, done: !failed }, at_once);
                }
            }
            Op::Expire { owner, before, life } => {
                if failed {
                    self.end("expire failed");
                } else {
                    self.end("expired");
                    self.store.retain(|record| record.at >= before);
                }
                self.observe(Seen::Forgot { before, done: !failed });
                if life == self.life {
                    self.ops.end(owner.raw());
                    self.answer(Event::Expired { owner, done: !failed }, at_once);
                }
            }
        }
        self.serve();
    }

    fn send(&mut self, at: Time, delivery: Delivery) {
        self.in_flight += 1;
        assert!(
            self.in_flight <= MAX_WIRE,
            "seed {}: no more than {MAX_WIRE} deliveries in flight",
            self.settings.seed
        );
        self.wire.send(at, delivery);
    }

    fn log(&mut self, line: String) {
        assert!(
            self.trace.lines().len() < MAX_TRACE,
            "seed {}: a trace keeps no more than {MAX_TRACE} lines",
            self.settings.seed
        );
        self.trace.log(self.now, line);
    }

    fn end(&mut self, ending: &'static str) {
        *self.stats.endings.entry(ending).or_default() += 1;
    }

    /// The referee observes `seen`, which ends the test if it breaks an
    /// expectation. This world's stimuli come from the referee's moments,
    /// never at once.
    fn observe(&mut self, seen: Seen) {
        let mut stimuli = Vec::new();
        self.referee.observe(self.now, seen, &mut stimuli);
        self.referee.assert_holding(self.settings.seed);
        assert!(stimuli.is_empty(), "the referee injects at its moments only");
    }

    fn below(&mut self, bound: u32) -> u32 {
        u32::try_from(self.rng.below(u64::from(bound))).expect("below a u32")
    }

    fn pick(&mut self, len: usize) -> usize {
        usize::try_from(self.rng.below(u64::try_from(len).expect("small"))).expect("small")
    }

    fn has_work_now(&self) -> bool {
        self.stage.has_events()
            || self.wire.is_due(self.now)
            || self.referee.is_due(self.now)
            || self.domain.is_due(self.now)
    }

    fn next_time(&self) -> Option<Time> {
        [self.wire.next_time(), self.referee.next_deadline(), self.domain.next_deadline()].into_iter().flatten().min()
    }

    /// The invariants of a world where nothing is left to happen: nothing in
    /// flight or held anywhere, the referee passed, and the views' counters
    /// of what they lost what the referee saw lost.
    fn assert_settled(&self) {
        assert!(self.wire.is_empty() && !self.stage.has_events(), "nothing is on its way");
        assert!(self.queue.is_empty() && !self.busy, "the store has nothing left to do");
        assert!(self.store.is_empty(), "the store keeps nothing past its retention");
        assert!(self.runs.is_empty() && self.people.is_empty(), "every run finished and every watch ended");
        self.watches.assert_settled();
        self.deliveries.assert_settled();
        self.ops.assert_settled();
        assert_eq!(self.domain.runs(), 0, "the views follow no run");
        assert_eq!(self.domain.watchers(), 0, "the views hold no watcher");
        assert_eq!(self.domain.batched(), 0, "the views batch nothing");
        assert_eq!(self.domain.ops(), 0, "the views have no store operation in flight");
        assert!(!self.domain.is_sweeping(), "the views have nothing left to expire");
        assert_eq!(self.domain.next_deadline(), None, "no deadline runs");
        self.referee.assert_passed(self.settings.seed);
        let seed = self.settings.seed;
        let now = self.domain.lost();
        let (runs, reports, chunks, records) = (
            self.lost.runs + now.runs,
            self.lost.reports + now.reports,
            self.lost.chunks + now.chunks,
            self.lost.records + now.records,
        );
        let tally = self.referee.expectations().tally;
        assert_eq!(runs, tally.runs, "seed {seed}: the views count the runs they turned away");
        assert_eq!(reports, tally.reports, "seed {seed}: the views count the reports they dropped");
        assert!(
            chunks >= tally.chunks && chunks <= tally.chunks + tally.abandoned,
            "seed {seed}: the views count the chunks watchers missed: {chunks} of {tally:?}"
        );
        let unsent = tally.traced - self.sent - self.batched;
        assert_eq!(records, unsent + self.failed, "seed {seed}: the views count the records their traces lost");
    }
}

/// What the referee sees of an event the views take; the store's terminals
/// it sees as the store does what they end.
fn taken(event: &Event) -> Option<Seen> {
    Some(match event {
        Event::Started { run, attempt, item, policy } => {
            Seen::Started { run: run.raw(), attempt: attempt.raw(), item: item.raw(), policy: *policy }
        }
        Event::Reported { run, kind, content } => {
            Seen::Reported { run: run.raw(), kind: *kind, content: content.to_vec() }
        }
        Event::Finished { run } => Seen::Finished { run: run.raw() },
        Event::Phase { item, repository, phase } => {
            Seen::Phase { item: item.raw(), repository: *repository, phase: *phase }
        }
        Event::Watch { watcher, subject, snapshot } => {
            Seen::Watch { watcher: watcher.raw(), subject: *subject, snapshot: snapshot.to_vec() }
        }
        Event::Unwatch { watcher } => Seen::Unwatch { watcher: watcher.raw() },
        Event::Delivered { watcher, done } => Seen::Delivered { watcher: watcher.raw(), done: *done },
        Event::Turn { .. } | Event::TaskPhase { .. } | Event::Appended { .. } | Event::Expired { .. } => return None,
    })
}

/// What the referee sees of a request as it leaves the views; an append it
/// sees as the store keeps it.
fn emitted(request: &Request) -> Option<Seen> {
    Some(match request {
        Request::Watching { watcher } => Seen::Watching { watcher: watcher.raw() },
        Request::Refused { watcher, refusal } => Seen::Refused { watcher: watcher.raw(), refusal: *refusal },
        Request::Deliver { watcher, missed, chunks } => {
            Seen::Deliver { watcher: watcher.raw(), missed: *missed, chunks: chunks.iter().map(copy).collect() }
        }
        Request::Ended { watcher, end } => Seen::Ended { watcher: watcher.raw(), end: *end },
        Request::Expire { before, .. } => Seen::Expire { before: *before },
        Request::Append { .. } => return None,
    })
}

fn describe(event: &Event) -> String {
    match event {
        Event::Started { run, attempt, item, policy } => {
            format!("started {} attempt {} for {} {policy:?}", run.raw(), attempt.raw(), item.raw())
        }
        Event::Reported { run, kind, content } => format!("reported {} {kind:?} {} bytes", run.raw(), content.len()),
        Event::Turn { run, attempt, number } => format!("turn {} attempt {} number {number}", run.raw(), attempt.raw()),
        Event::TaskPhase { item, phase, .. } => format!("task phase {} {phase}", item.raw()),
        Event::Finished { run } => format!("finished {}", run.raw()),
        Event::Phase { item, repository, phase } => format!("phase {} of {repository} {phase}", item.raw()),
        Event::Watch { watcher, subject, snapshot } => {
            format!("watch {} {subject:?} {} bytes", watcher.raw(), snapshot.len())
        }
        Event::Unwatch { watcher } => format!("unwatch {}", watcher.raw()),
        Event::Delivered { watcher, done } => format!("delivered {} {done}", watcher.raw()),
        Event::Appended { owner, done } => format!("appended {} {done}", owner.raw()),
        Event::Expired { owner, done } => format!("expired {} {done}", owner.raw()),
    }
}

fn describe_request(request: &Request) -> String {
    match request {
        Request::Watching { watcher } => format!("watching {}", watcher.raw()),
        Request::Refused { watcher, refusal } => format!("refused {} {refusal:?}", watcher.raw()),
        Request::Deliver { watcher, missed, chunks } => {
            format!("deliver {} {missed} missed {} chunks", watcher.raw(), chunks.len())
        }
        Request::Ended { watcher, end } => format!("ended {} {end:?}", watcher.raw()),
        Request::Append { owner, records } => format!("append {} {} records", owner.raw(), records.len()),
        Request::Expire { owner, before } => format!("expire {} before {}", owner.raw(), before.as_nanos()),
    }
}
