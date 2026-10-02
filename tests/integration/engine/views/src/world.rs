use std::collections::{BTreeMap, VecDeque};

use temper_engine_model_views::{
    self as views, Capture, Dropped, End, Event, Fact, Kept, Kind, Limits, Model, Phase, Policy, Record, Refusal,
    Request, Subject,
};
use temper_lib::{Duration, Rng, Time, Token};
use temper_world::{Ledger, Referee, Schedule, Span, Stage, Trace};

use crate::referee::{Bounds, Seen, Stimulus, Views, copy, copy_record};

/// Room in the views' output queue beyond what one step may emit. Small, so
/// the loop's flow control is exercised.
const SLACK: u32 = 2;

/// The most lines a trace keeps, and deliveries in flight: a world past
/// either fails with its seed rather than grow.
const MAX_TRACE: usize = 400_000;
const MAX_WIRE: usize = 4096;

/// Every kind of report, every phase, and every capture.
pub const KINDS: [Kind; 5] = [Kind::Text, Kind::Progress, Kind::Call, Kind::Tool, Kind::Usage];
pub const PHASES: [Phase; 7] =
    [Phase::Waiting, Phase::Due, Phase::Claimed, Phase::Running, Phase::Applying, Phase::Held, Phase::Done];
const CAPTURES: [Capture; 3] = [Capture::Nothing, Capture::Shape, Capture::Content];

/// The views' limits in the calm world: room for every run and watch, a
/// backlog a few chunks long, batches of a few reports.
pub const LIMITS: Limits = Limits {
    runs: 6,
    watchers: 16,
    backlog: 4,
    report_bytes: 48,
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
pub const ENDINGS: [&str; 15] = [
    "delivered",
    "missed",
    "busy",
    "unknown",
    "unwatched",
    "finished",
    "appended",
    "append failed",
    "partly kept",
    "expired",
    "expire failed",
    "oversized",
    "late",
    "unfollowed",
    "lost",
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Settings {
    /// Seeds the world.
    pub seed: u64,
    pub limits: Limits,
    /// The runs the parent starts and the time between starts; the most
    /// reports a run makes, and the time between them; per mille reports
    /// past the limits, and runs that report once more after they finished.
    pub runs: u32,
    pub run_gap: Span,
    pub reports: u32,
    pub report_gap: Span,
    pub oversized: u32,
    pub late: u32,
    /// The items and the repositories they are in; the phase changes in all,
    /// and the time between them.
    pub items: u32,
    pub repositories: u32,
    pub changes: u32,
    pub change_gap: Span,
    /// The watches people start, the time between them and how long each
    /// lasts; their streams' latency, per mille slow ones and theirs; and per
    /// mille watches of a run that is not followed.
    pub watches: u32,
    pub watch_gap: Span,
    pub watch_for: Span,
    pub latency: Span,
    pub slow: u32,
    pub slow_latency: Span,
    pub stale: u32,
    /// How long the store takes over an operation, and per mille those that
    /// fail.
    pub store_latency: Span,
    pub store_failures: u32,
}

impl Settings {
    /// A world where nothing goes wrong: runs and watches within the limits,
    /// people who keep up, a store that is quick and never fails.
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
            items: 5,
            repositories: 2,
            changes: 40,
            change_gap: Span::millis(100, 2000),
            watches: 20,
            watch_gap: Span::millis(200, 3000),
            watch_for: Span::millis(5000, 30_000),
            latency: Span::millis(1, 10),
            slow: 0,
            slow_latency: Span::millis(0, 0),
            stale: 0,
            store_latency: Span::millis(1, 50),
            store_failures: 0,
        }
    }

    /// A world of its own for `seed`: runs that report fast, past the
    /// limits and late; more runs and watches than the limits hold; people
    /// who are slow; a store that is slow and fails, at chances drawn from
    /// the seed.
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
            items: 1 + chance(8),
            repositories: 1 + chance(3),
            changes: chance(80),
            change_gap: Span::millis(1, 10 + u64::from(chance(3000))),
            watches: 4 + chance(30),
            watch_gap: Span::millis(1, 10 + u64::from(chance(3000))),
            watch_for: Span::millis(1, 100 + u64::from(chance(30_000))),
            latency: Span::millis(1, 1 + u64::from(chance(50))),
            slow: chance(500),
            slow_latency: Span::millis(50, 100 + u64::from(chance(3000))),
            stale: chance(150),
            store_latency: Span::millis(1, 5 + u64::from(chance(2000))),
            store_failures: chance(300),
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
        let reach = self.latency.max.max(self.slow_latency.max).saturating_add(tick);
        let end = Duration::from_nanos(reach.as_nanos().saturating_mul(2));
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
    /// The run reports its next report, or finishes.
    Report { run: u64 },
    /// The run, finished, reports once more: a fact that came late.
    Late { run: u64 },
    /// An item's phase changes.
    Change,
    /// A person starts watching.
    Watch,
    /// The person stops watching.
    Unwatch { watcher: u64 },
    /// The person's stream has taken its delivery.
    Delivered { watcher: u64 },
    /// The store is done with the operation in hand.
    Stored,
}

/// A run the parent plays: the reports it has made and has left to make.
#[derive(Debug)]
struct Script {
    reported: u32,
    left: u32,
}

/// An item, by the parent's token, its repository and its phase.
#[derive(Debug)]
struct Item {
    token: u64,
    repository: u32,
    phase: Phase,
}

/// An operation on the store, by the views' owner token.
#[derive(Debug)]
enum Op {
    /// An append, which the world names `name`.
    Append {
        owner: Token,
        name: u64,
        records: Box<[Record]>,
    },
    Expire {
        owner: Token,
        before: Time,
    },
}

pub struct World {
    now: Time,
    rng: Rng,
    settings: Settings,

    model: Model,
    stage: Stage<Limits, Event, Request>,

    /// Deliveries in flight, and their count; the count names runs, watches
    /// and appends too.
    wire: Schedule<Delivery>,
    in_flight: usize,

    /// The parent's script: runs left to start, those reporting and those
    /// finished; the items and the changes left; the watches left to start,
    /// and each person's stream latency.
    runs_left: u32,
    runs: BTreeMap<u64, Script>,
    finished: Vec<u64>,
    items: Vec<Item>,
    changes_left: u32,
    watches_left: u32,
    people: BTreeMap<u64, Span>,

    /// The watches from their watch to their end, with whether each was
    /// answered; the deliveries in flight, by watcher; and the store's
    /// operations, by owner.
    watches: Ledger<u64, bool>,
    deliveries: Ledger<u64, ()>,
    ops: Ledger<u64, ()>,

    /// The store: what it has yet to do, in order, whether it is doing it,
    /// and the records it keeps.
    queue: VecDeque<Op>,
    busy: bool,
    store: Vec<Record>,

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
                phase: Phase::Waiting,
            })
            .collect();
        let mut world = World {
            now: Time::ZERO,
            rng: Rng::new(settings.seed),
            model: Model::new(&settings.limits),
            stage: Stage::new(settings.limits, max_out, max_out + SLACK),
            wire: Schedule::new(),
            in_flight: 0,
            runs_left: settings.runs,
            runs: BTreeMap::new(),
            finished: Vec::new(),
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
            referee: Referee::new(Views::new(settings.limits, settings.bounds())),
            stats: Stats::default(),
            trace: Trace::default(),
            settings,
        };
        if settings.runs > 0 {
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

    /// One iteration of the loop, as the shell would run it.
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
            for stimulus in &stimuli {
                inject(stimulus);
            }
        }
        while let Some(event) = self.stage.next_event() {
            self.log(format!("views <- {}", describe(&event)));
            if let Some(seen) = taken(&event) {
                self.observe(seen);
            }
            let before = self.stage.out.len();
            views::step(&mut self.model, &self.stage.env, event, &mut self.stage.out);
            self.emitted(before);
        }
        while self.stage.has_room() && self.model.is_due(now) {
            let before = self.stage.out.len();
            views::fire(&mut self.model, &self.stage.env, &mut self.stage.out);
            self.emitted(before);
        }
        // What the steps asked for, at the end of the iteration.
        while let Some(request) = self.stage.out.pop() {
            self.request(request);
        }
        while let Some(fact) = self.model.pop_fact() {
            self.stats.facts += 1;
            match fact {
                Fact::Unfollowed => self.end("unfollowed"),
                Fact::Reported { kept: Kept::Lost, .. } => self.end("lost"),
                Fact::Followed
                | Fact::Reported { kept: Kept::Nothing | Kept::Shape | Kept::Content, .. }
                | Fact::Dropped { dropped: Dropped::Unfollowed | Dropped::Oversized }
                | Fact::Changed { .. }
                | Fact::Watching
                | Fact::Refused { .. }
                | Fact::Ended { .. }
                | Fact::Overflowed { .. }
                | Fact::Delivered { .. }
                | Fact::Appending { .. }
                | Fact::Appended { .. }
                | Fact::Expiring
                | Fact::Expired { .. } => {}
            }
        }
        // The reclaim point.
        self.model.reclaim();
    }

    /// The referee sees what a step or an alarm emitted since `before`, as
    /// it leaves the views.
    fn emitted(&mut self, before: u32) {
        let seen: Vec<Seen> =
            self.stage.out.iter().skip(usize::try_from(before).expect("small")).filter_map(emitted).collect();
        for seen in seen {
            self.observe(seen);
        }
    }

    /// Hands `delivery` to its destination.
    fn deliver(&mut self, delivery: Delivery) {
        match delivery {
            Delivery::Start => self.start(),
            Delivery::Report { run } => self.report(run),
            Delivery::Late { run } => {
                self.end("late");
                let content = self.content(run, u32::MAX);
                self.stage.push(Event::Reported { run: Token::new(run), kind: Kind::Progress, content });
            }
            Delivery::Change => self.change(),
            Delivery::Watch => self.watch(),
            Delivery::Unwatch { watcher } => {
                // The parent forgets a watch once it has ended.
                if self.watches.get(watcher) == Some(&true) {
                    self.stage.push(Event::Unwatch { watcher: Token::new(watcher) });
                }
            }
            Delivery::Delivered { watcher } => {
                self.deliveries.end(watcher);
                self.stage.push(Event::Delivered { watcher: Token::new(watcher) });
            }
            Delivery::Stored => self.stored(),
        }
    }

    /// The parent starts a run, for an item drawn, under a policy drawn.
    fn start(&mut self) {
        self.runs_left -= 1;
        if self.runs_left > 0 {
            let at = self.now.saturating_add(self.settings.run_gap.draw(&mut self.rng));
            self.send(at, Delivery::Start);
        }
        self.stats.runs += 1;
        let run = self.wire.name();
        let item = self.pick(self.items.len());
        let mut capture = || CAPTURES[usize::try_from(self.rng.below(3)).expect("small")];
        let policy =
            Policy { text: capture(), progress: capture(), calls: capture(), tools: capture(), usage: capture() };
        let left = 1 + self.below(self.settings.reports);
        self.runs.insert(run, Script { reported: 0, left });
        let token = self.items[item].token;
        self.stage.push(Event::Started { run: Token::new(run), item: Token::new(token), policy });
        let at = self.now.saturating_add(self.settings.report_gap.draw(&mut self.rng));
        self.send(at, Delivery::Report { run });
    }

    /// The run reports, of a kind drawn, past the limits now and then; or,
    /// with nothing left to report, finishes.
    fn report(&mut self, run: u64) {
        let script = self.runs.get_mut(&run).expect("a run reporting is in the script");
        if script.left == 0 {
            self.runs.remove(&run);
            self.finished.push(run);
            self.stage.push(Event::Finished { run: Token::new(run) });
            if self.rng.chance(self.settings.late) {
                let at = self.now.saturating_add(self.settings.report_gap.draw(&mut self.rng));
                self.send(at, Delivery::Late { run });
            }
            return;
        }
        script.left -= 1;
        script.reported += 1;
        let reported = script.reported;
        self.stats.reports += 1;
        let kind = KINDS[self.pick(KINDS.len())];
        let content = self.content(run, reported);
        if content.len() > self.settings.limits.report_bytes as usize {
            self.end("oversized");
        }
        self.stage.push(Event::Reported { run: Token::new(run), kind, content });
        let at = self.now.saturating_add(self.settings.report_gap.draw(&mut self.rng));
        self.send(at, Delivery::Report { run });
    }

    /// A report's content: its run and number, which make it unique, then
    /// letters, within the limits or, now and then, past them.
    fn content(&mut self, run: u64, number: u32) -> Box<[u8]> {
        let mut content = format!("r{run}#{number}:").into_bytes();
        let most = self.settings.limits.report_bytes as usize;
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
        let phase = PHASES[self.pick(PHASES.len())];
        let item = &mut self.items[index];
        item.phase = phase;
        let event = Event::Phase { item: Token::new(item.token), repository: item.repository, phase };
        self.stage.push(event);
    }

    /// A person starts watching a run, an item or a board, drawn; now and
    /// then a run that is not followed; and stops after a time drawn.
    fn watch(&mut self) {
        self.watches_left -= 1;
        if self.watches_left > 0 {
            let at = self.now.saturating_add(self.settings.watch_gap.draw(&mut self.rng));
            self.send(at, Delivery::Watch);
        }
        self.stats.watches += 1;
        let watcher = self.wire.name();
        let live: Vec<u64> = self.runs.keys().copied().collect();
        let subject = if self.rng.chance(self.settings.stale) {
            match self.finished.len() {
                0 => Subject::Run(Token::new(u64::MAX)),
                len => {
                    let index = self.pick(len);
                    Subject::Run(Token::new(self.finished[index]))
                }
            }
        } else {
            match self.pick(3) {
                0 if !live.is_empty() => Subject::Run(Token::new(live[self.pick(live.len())])),
                0 | 1 if !self.items.is_empty() => {
                    let index = self.pick(self.items.len());
                    Subject::Item(Token::new(self.items[index].token))
                }
                _ => Subject::Board(self.below(self.settings.repositories + 1)),
            }
        };
        let latency =
            if self.rng.chance(self.settings.slow) { self.settings.slow_latency } else { self.settings.latency };
        self.people.insert(watcher, latency);
        self.watches.open(watcher, false);
        self.stage.push(Event::Watch { watcher: Token::new(watcher), subject });
        let at = self.now.saturating_add(self.settings.watch_for.draw(&mut self.rng));
        self.send(at, Delivery::Unwatch { watcher });
    }

    /// Takes `request` from the views' queue: an answer or an end to a
    /// watch, a delivery to a person's stream, or an operation on the store.
    fn request(&mut self, request: Request) {
        self.log(format!("views -> {}", describe_request(&request)));
        match request {
            Request::Watching { watcher } => {
                let answered = self.watches.get_mut(watcher.raw()).expect("a watch is answered while open");
                assert!(!*answered, "a watch is answered once");
                *answered = true;
            }
            Request::Refused { watcher, refusal } => {
                let answered = self.watches.end(watcher.raw());
                assert!(!answered, "a watch is answered once");
                self.people.remove(&watcher.raw());
                self.end(match refusal {
                    Refusal::Busy => "busy",
                    Refusal::Unknown => "unknown",
                });
            }
            Request::Deliver { watcher, missed, chunks } => {
                assert!(!chunks.is_empty(), "a delivery delivers something");
                assert!(self.watches.get(watcher.raw()) == Some(&true), "a delivery goes to a watch that is open");
                self.deliveries.open(watcher.raw(), ());
                self.end("delivered");
                if missed > 0 {
                    self.end("missed");
                }
                let latency = self.people[&watcher.raw()].draw(&mut self.rng);
                self.send(self.now.saturating_add(latency), Delivery::Delivered { watcher: watcher.raw() });
            }
            Request::Ended { watcher, end } => {
                let answered = self.watches.end(watcher.raw());
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
                let name = self.wire.name();
                self.queue.push_back(Op::Append { owner, name, records });
                self.serve();
            }
            Request::Expire { owner, before } => {
                self.ops.open(owner.raw(), ());
                self.queue.push_back(Op::Expire { owner, before });
                self.serve();
            }
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
    fn stored(&mut self) {
        self.busy = false;
        let op = self.queue.pop_front().expect("the store does what it was asked");
        let failed = self.rng.chance(self.settings.store_failures);
        match op {
            Op::Append { owner, name, records } => {
                let mut records = records.into_vec();
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
                self.ops.end(owner.raw());
                self.stage.push(Event::Appended { owner, done: !failed });
            }
            Op::Expire { owner, before } => {
                if failed {
                    self.end("expire failed");
                } else {
                    self.end("expired");
                    self.store.retain(|record| record.at >= before);
                }
                self.observe(Seen::Forgot { before, done: !failed });
                self.ops.end(owner.raw());
                self.stage.push(Event::Expired { owner, done: !failed });
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
    /// expectation.
    fn observe(&mut self, seen: Seen) {
        let mut stimuli = Vec::new();
        self.referee.observe(self.now, seen, &mut stimuli);
        self.referee.assert_holding(self.settings.seed);
        for stimulus in &stimuli {
            inject(stimulus);
        }
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
            || self.model.is_due(self.now)
    }

    fn next_time(&self) -> Option<Time> {
        [self.wire.next_time(), self.referee.next_deadline(), self.model.next_deadline()].into_iter().flatten().min()
    }

    /// The invariants of a world where nothing is left to happen.
    fn assert_settled(&self) {
        assert!(self.wire.is_empty() && !self.stage.has_events(), "nothing is on its way");
        assert!(self.queue.is_empty() && !self.busy, "the store has nothing left to do");
        assert!(self.store.is_empty(), "the store keeps nothing past its retention");
        assert!(self.runs.is_empty() && self.people.is_empty(), "every run finished and every watch ended");
        self.watches.assert_settled();
        self.deliveries.assert_settled();
        self.ops.assert_settled();
        assert_eq!(self.model.runs(), 0, "the views follow no run");
        assert_eq!(self.model.watchers(), 0, "the views hold no watcher");
        assert_eq!(self.model.batched(), 0, "the views batch nothing");
        assert_eq!(self.model.ops(), 0, "the views have no store operation in flight");
        assert!(!self.model.is_sweeping(), "the views have nothing left to expire");
        assert_eq!(self.model.next_deadline(), None, "no deadline runs");
        self.referee.assert_passed(self.settings.seed);
    }
}

/// What the referee sees of an event the views take; the store's terminals
/// it sees as the store does what they end.
fn taken(event: &Event) -> Option<Seen> {
    Some(match event {
        Event::Started { run, item, policy } => Seen::Started { run: run.raw(), item: item.raw(), policy: *policy },
        Event::Reported { run, kind, content } => {
            Seen::Reported { run: run.raw(), kind: *kind, content: content.to_vec() }
        }
        Event::Finished { run } => Seen::Finished { run: run.raw() },
        Event::Phase { item, repository, phase } => {
            Seen::Phase { item: item.raw(), repository: *repository, phase: *phase }
        }
        Event::Watch { watcher, subject } => Seen::Watch { watcher: watcher.raw(), subject: *subject },
        Event::Unwatch { watcher } => Seen::Unwatch { watcher: watcher.raw() },
        Event::Delivered { watcher } => Seen::Delivered { watcher: watcher.raw() },
        Event::Appended { .. } | Event::Expired { .. } => return None,
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

/// What the referee injects: nothing, in this world.
fn inject(stimulus: &Stimulus) {
    match *stimulus {}
}

fn describe(event: &Event) -> String {
    match event {
        Event::Started { run, item, policy } => format!("started {} for {} {policy:?}", run.raw(), item.raw()),
        Event::Reported { run, kind, content } => format!("reported {} {kind:?} {} bytes", run.raw(), content.len()),
        Event::Finished { run } => format!("finished {}", run.raw()),
        Event::Phase { item, repository, phase } => format!("phase {} of {repository} {phase:?}", item.raw()),
        Event::Watch { watcher, subject } => format!("watch {} {subject:?}", watcher.raw()),
        Event::Unwatch { watcher } => format!("unwatch {}", watcher.raw()),
        Event::Delivered { watcher } => format!("delivered {}", watcher.raw()),
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
