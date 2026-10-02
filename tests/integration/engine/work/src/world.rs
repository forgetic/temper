use std::collections::{BTreeMap, BTreeSet, VecDeque};

use temper_engine_model_work::{
    self as work, Acted, Answer, Applied, Class, Due, Event, Fact, Hold, Item, Lifecycle, Limits, Model, Phase, Read,
    Refusal, Request, Retries, Retry, Then, Wrote,
};
use temper_lib::{Duration, ReplyTo, Rng, Time, Token};
use temper_world::{Ledger, Referee, Schedule, Span, Stage, Trace};

use crate::referee::{Key, Seen, Stimulus, Work};

/// Room in the hub's output queue beyond what one step may emit. Small, so
/// the loop's flow control is exercised.
const SLACK: u32 = 2;

/// The repositories items are handed in to.
const REPOSITORIES: u32 = 2;

/// Bounds on the world's own state: a world past them is stuck, and fails
/// with its seed rather than growing.
const MAX_PENDING: u64 = 20_000;
const MAX_TRACE: usize = 400_000;

/// How often a person releases one item at most.
const RELEASES: u32 = 2;

/// The hub's limits in the calm world: room for every item at once.
pub const LIMITS: Limits = Limits {
    items: 16,
    grace: Duration::from_secs(20),
    retries: Retries {
        transient: Retry { retries: 3, base: Duration::from_secs(1), max: Duration::from_secs(8) },
        permanent: Retry { retries: 0, base: Duration::from_secs(1), max: Duration::from_secs(1) },
        run: Retry { retries: 2, base: Duration::from_secs(2), max: Duration::from_secs(8) },
        agent: Retry { retries: 2, base: Duration::from_secs(2), max: Duration::from_secs(8) },
        lost: Retry { retries: 2, base: Duration::from_secs(1), max: Duration::from_secs(4) },
        invalid: Retry { retries: 1, base: Duration::from_secs(1), max: Duration::from_secs(1) },
    },
    facts: 256,
};

/// How a world's items and runs ended, by kind, for the sweep.
pub const ENDINGS: [&str; 18] = [
    "done",
    "held: plan",
    "held: failures",
    "held: stopped",
    "held: acceptance",
    "held: writes",
    "held: record",
    "released",
    "parked",
    "lost",
    "stale",
    "invalid",
    "acted",
    "adopted",
    "resumed",
    "full",
    "stale answer",
    "cancelled",
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Settings {
    /// Seeds the world.
    pub seed: u64,
    pub limits: Limits,
    /// Items people hand in, the time between them, and the outcomes each
    /// needs at most.
    pub items: u32,
    pub hand_in_gap: Span,
    pub needed: u32,
    /// The plan: how long it takes to decide; per mille of its decisions
    /// that wait (until a time drawn from `wait`), that act, and that hold;
    /// and how many waits and actions an item takes at most.
    pub deciding: Span,
    pub waits: u32,
    pub wait: Span,
    pub acts: u32,
    pub holds: u32,
    pub detours: u32,
    /// Applying an outcome: per mille stale, invalid, and needing a person's
    /// acceptance, as the plan and the rules judge it; and per mille a write
    /// of an application or an action that fails for good.
    pub stale: u32,
    pub invalid: u32,
    pub accepting: u32,
    pub broken: u32,
    /// The forge: how long a request takes to be served and its answer to
    /// come back; per mille late, and by how much; per mille failing
    /// transiently (served again a latency later) and refused for good.
    pub latency: Span,
    pub late: u32,
    pub lateness: Span,
    pub transient: u32,
    pub refused: u32,
    /// The fleet: its workers, how long a run takes, and per mille the runs
    /// that park and that fail (the rest end with an outcome); the channel's
    /// latency; per mille the runs whose worker drops as they run, and of
    /// those, the workers that come back within the fleet's grace; how long
    /// they take.
    pub workers: u32,
    pub run_time: Span,
    pub parks: u32,
    pub fails: u32,
    pub channel: Span,
    pub drops: u32,
    pub returns: u32,
    pub grace: Duration,
    /// People: messages on items, and the time between them; stops; per
    /// mille the holds a person releases, and how long after.
    pub messages: u32,
    pub message_gap: Span,
    pub stops: u32,
    pub releases: u32,
    pub release_after: Span,
    /// Engine restarts, and the time between them.
    pub restarts: u32,
    pub restart_gap: Span,
}

impl Settings {
    /// A world where nothing goes wrong: items handed in, some waits and
    /// actions, runs that end or park, a forge and workers that answer in
    /// time, no stop, no restart.
    #[must_use]
    pub fn calm(seed: u64) -> Settings {
        Settings {
            seed,
            limits: LIMITS,
            items: 8,
            hand_in_gap: Span::millis(100, 3000),
            needed: 3,
            deciding: Span::millis(0, 5),
            waits: 150,
            wait: Span::millis(100, 5000),
            acts: 100,
            holds: 0,
            detours: 3,
            stale: 0,
            invalid: 0,
            accepting: 0,
            broken: 0,
            latency: Span::millis(5, 100),
            late: 0,
            lateness: Span::millis(0, 0),
            transient: 0,
            refused: 0,
            workers: 3,
            run_time: Span::millis(500, 10_000),
            parks: 200,
            fails: 0,
            channel: Span::millis(1, 50),
            drops: 0,
            returns: 1000,
            grace: Duration::from_secs(10),
            messages: 20,
            message_gap: Span::millis(100, 4000),
            stops: 0,
            releases: 0,
            release_after: Span::millis(100, 2000),
            restarts: 0,
            restart_gap: Span::millis(1000, 20_000),
        }
    }

    /// A world of its own for `seed`: every fault at chances drawn from the
    /// seed, people who stop and release, and engine restarts.
    #[must_use]
    pub fn random(seed: u64) -> Settings {
        let mut rng = Rng::new(seed ^ 0x5EED_0000_0000_3A1B);
        let mut chance = |most: u64| u32::try_from(rng.below(most + 1)).expect("a chance per mille");
        let calm = Settings::calm(seed);
        Settings {
            items: 4 + chance(12),
            hand_in_gap: Span::millis(1, 100 + u64::from(chance(5000))),
            needed: 1 + chance(3),
            waits: chance(300),
            acts: chance(250),
            holds: chance(80),
            stale: chance(200),
            invalid: chance(200),
            accepting: chance(150),
            broken: chance(60),
            latency: Span::millis(1, 10 + u64::from(chance(500))),
            late: chance(200),
            lateness: Span::millis(0, u64::from(chance(5000))),
            transient: chance(200),
            refused: chance(30),
            workers: 1 + chance(3),
            parks: chance(300),
            fails: chance(400),
            drops: chance(200),
            returns: chance(1000),
            messages: chance(40),
            stops: chance(6),
            releases: 300 + chance(700),
            restarts: chance(3),
            limits: Limits { items: 2 + chance(10), ..LIMITS },
            ..calm
        }
    }
}

/// What the world counted, as it crossed the boundary.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct Stats {
    /// Items handed in, and how items and runs ended, by kind.
    pub items: u32,
    pub endings: BTreeMap<&'static str, u32>,
    /// Requests served by the plan and the forge; those failing transiently,
    /// refused for good, and late.
    pub decisions: u32,
    pub writes: u32,
    pub transients: u32,
    pub refusals: u32,
    pub lates: u32,
    /// Runs started and answered, inbox events relayed, snapshots kept.
    pub runs: u32,
    pub answers: u32,
    pub relayed: u32,
    pub kept: u32,
    /// People's messages, stops and releases; engine restarts.
    pub messages: u32,
    pub stops: u32,
    pub releases: u32,
    pub restarts: u32,
    /// The hub's facts.
    pub facts: u32,
}

/// Something on its way, delivered at its time.
#[derive(Debug)]
enum Delivery {
    /// A person hands an item in.
    HandIn,
    /// A person writes on an item.
    Message,
    /// A person stops an item's run.
    Stop,
    /// A person releases a held item.
    Release { item: Item },
    /// The plan or the forge serves the request `serial`.
    Serve { serial: u64 },
    /// An event reaches the hub of `generation`.
    Hub { generation: u64, event: Event },
    /// The message `serial` reaches its worker.
    Send { serial: u64 },
    /// The course of a run on its worker ends.
    Ends { item: Item, attempt: u64, serial: u64 },
    /// A worker's channel drops.
    Drop { worker: usize },
    /// A worker reconnects, after its drop `episode`.
    Reconnect { worker: usize, episode: u64 },
    /// The fleet's grace for a worker that dropped is over.
    Grace { worker: usize, episode: u64, generation: u64 },
}

/// What the hub asked of the plan or the forge, until it is served.
#[derive(Clone, Copy, Debug)]
enum Op {
    Due { owner: Token },
    Write { owner: Token, record: Record },
    Record { owner: Token, attempt: u64 },
    Apply { owner: Token, attempt: u64, outcome: u64 },
    Act { owner: Token, done: bool },
}

#[derive(Clone, Copy, Debug)]
struct Serving {
    generation: u64,
    item: Item,
    op: Op,
}

/// What the fleet sends a worker.
#[derive(Clone, Copy, Debug)]
enum Message {
    Start,
    Cancel,
    Relay,
    /// The engine has the answer, or drops it: the worker may forget it.
    Forget,
}

#[derive(Clone, Copy, Debug)]
struct Sending {
    worker: usize,
    /// The worker's channel it was sent on: lost if that one drops.
    episode: u64,
    item: Item,
    attempt: u64,
    message: Message,
}

/// An item on the forge.
#[derive(Debug)]
struct Issue {
    /// The engine's record: its part, and the plan's.
    record: Option<Record>,
    closed: bool,
    /// The outcomes posted on it, by attempt: their comments.
    outcomes: BTreeMap<u64, u64>,
    /// The scripted plan: the outcomes the item needs, the waits and actions
    /// it may still take, and whether it may still be held.
    needed: u32,
    detours: u32,
    holds: u32,
    /// The attempts whose outcomes a person accepted, and the releases left.
    accepted: BTreeSet<u64>,
    releases: u32,
}

/// An item's record, as the parent composes it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Record {
    lifecycle: Lifecycle,
    plan: Plan,
}

/// The plan's part of a record: the outcomes applied so far, and the actions
/// made.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
struct Plan {
    progress: u32,
    acts: u32,
}

/// A worker, and what it keeps of its runs.
#[derive(Debug, Default)]
struct Worker {
    connected: bool,
    /// Counts its drops, so that a reconnect or a grace of an earlier one is
    /// ignored.
    episode: u64,
    runs: BTreeSet<(Item, u64)>,
    /// Answers kept until the engine says to forget them.
    answers: BTreeMap<(Item, u64), Answer>,
}

/// A run on a worker: how it is to end, and when.
#[derive(Debug)]
struct Course {
    worker: usize,
    answer: Answer,
    serial: u64,
}

/// What the engine's fleet knows, in memory: lost as the engine stops.
#[derive(Debug, Default)]
struct Fleet {
    /// Runs assigned, and answers heard, by the worker that has them.
    assigned: BTreeMap<(Item, u64), usize>,
    heard: BTreeMap<(Item, u64), usize>,
    /// Starts waiting for a worker to connect.
    queued: VecDeque<(Item, u64)>,
}

/// A call the parent made, until it is answered.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Call {
    Take(Item),
    Stop,
    Release(Item),
}

pub struct World {
    now: Time,
    rng: Rng,
    settings: Settings,
    /// Counts the engine's processes: what is on its way to an earlier one is
    /// dropped.
    generation: u64,

    model: Model,
    stage: Stage<Limits, Event, Request>,
    /// Deliveries in flight, whose count names everything else, and how many
    /// are on their way.
    wire: Schedule<Delivery>,
    pending: u64,
    serving: BTreeMap<u64, Serving>,
    sending: BTreeMap<u64, Sending>,

    /// The forge's items, and its keyed creations.
    issues: BTreeMap<Item, Issue>,
    created: BTreeSet<Key>,
    /// The items the engine holds since a write of their record was
    /// refused, which their records do not show.
    refused: BTreeSet<Item>,
    /// The parent, in memory: the plan's part of each record taken in, the
    /// items waiting at the entrance and whether one is being taken in, the
    /// items taken in, the engine actions decided, and the calls and requests
    /// in flight.
    plans: BTreeMap<Item, Plan>,
    entrance: VecDeque<Item>,
    taking: bool,
    taken: BTreeSet<Item>,
    actions: BTreeMap<u64, bool>,
    calls: Ledger<u64, Call>,
    ops: Ledger<Item, ()>,
    fleet: Fleet,
    /// The workers, and the courses of the runs they host.
    workers: Vec<Worker>,
    courses: BTreeMap<(Item, u64), Course>,

    items_left: u32,
    messages_left: u32,
    stops_left: u32,
    numbers: u64,

    referee: Referee<Work>,
    stats: Stats,
    trace: Trace,
}

impl World {
    #[must_use]
    pub fn new(settings: Settings) -> World {
        assert!(work::worst_case(&settings.limits).is_some(), "the shell refuses limits it cannot provision");
        let max_out = work::max_out(&settings.limits);
        let mut referee = Referee::new(Work::new(end_within(&settings)));
        let mut rng = Rng::new(settings.seed);
        let mut at = Time::ZERO;
        for _ in 0..settings.restarts {
            at = at.saturating_add(settings.restart_gap.draw(&mut rng));
            referee.inject(at, Stimulus::Restart);
        }
        let mut workers = Vec::new();
        for _ in 0..settings.workers {
            workers.push(Worker { connected: true, ..Worker::default() });
        }
        let mut world = World {
            now: Time::ZERO,
            rng,
            generation: 0,
            model: Model::new(&settings.limits, settings.seed),
            stage: Stage::new(settings.limits, max_out, max_out + SLACK),
            wire: Schedule::new(),
            pending: 0,
            serving: BTreeMap::new(),
            sending: BTreeMap::new(),
            issues: BTreeMap::new(),
            created: BTreeSet::new(),
            refused: BTreeSet::new(),
            plans: BTreeMap::new(),
            entrance: VecDeque::new(),
            taking: false,
            taken: BTreeSet::new(),
            actions: BTreeMap::new(),
            calls: Ledger::new("call"),
            ops: Ledger::new("request of an item"),
            fleet: Fleet::default(),
            workers,
            courses: BTreeMap::new(),
            items_left: settings.items,
            messages_left: settings.messages,
            stops_left: settings.stops,
            numbers: 0,
            referee,
            stats: Stats::default(),
            trace: Trace::default(),
            settings,
        };
        world.schedule(Time::ZERO, Delivery::HandIn);
        if settings.messages > 0 {
            world.after(settings.message_gap, Delivery::Message);
        }
        if settings.stops > 0 {
            world.after(settings.run_time, Delivery::Stop);
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

    /// What crossed between the hub and the world, in order, with times.
    #[must_use]
    pub fn trace(&self) -> &[String] {
        self.trace.lines()
    }

    /// Runs started and plan decisions asked that the referee judged.
    #[must_use]
    pub fn judged(&self) -> (u64, u64) {
        let work = self.referee.expectations();
        (work.starts, work.asks)
    }

    /// How the items ended: those done, and those held.
    #[must_use]
    pub fn ended(&self) -> (u32, u32) {
        let mut ended = (0, 0);
        for issue in self.issues.values() {
            match stands(issue.record) {
                Stands::Done => ended.0 += 1,
                Stands::Held(_) => ended.1 += 1,
                Stands::Live | Stands::Claimed | Stands::Applying => {}
            }
        }
        ended
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
        panic!("seed {}: the world did not settle in {iterations} iterations", self.settings.seed);
    }

    /// One iteration of the loop, as the shell would run it.
    fn iterate(&mut self) {
        let now = self.now;
        let seed = self.settings.seed;
        assert!(self.trace.lines().len() <= MAX_TRACE, "seed {seed}: the trace outgrew {MAX_TRACE} lines");
        self.stage.tick(now);
        let mut delivered = 0;
        while let Some(delivery) = self.wire.next(now) {
            self.pending -= 1;
            delivered += 1;
            assert!(delivered <= MAX_PENDING, "seed {seed}: more than {MAX_PENDING} deliveries in one iteration");
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
            self.trace.log(now, format!("work <- {event:?}"));
            work::step(&mut self.model, &self.stage.env, event, &mut self.stage.out);
        }
        while self.stage.has_room() && self.model.is_due(now) {
            self.trace.log(now, "work <- alarm");
            let before = self.model.next_deadline();
            work::fire(&mut self.model, &self.stage.env, &mut self.stage.out);
            let fired = self.model.next_deadline() != before || !self.stage.out.is_empty();
            assert!(fired, "seed {seed}: an alarm due fires");
        }
        while let Some(request) = self.stage.out.pop() {
            self.trace.log(now, format!("work -> {request:?}"));
            self.request(request);
        }
        while let Some(fact) = self.model.pop_fact() {
            self.stats.facts += 1;
            self.fact(fact);
        }
        // The reclaim point.
        self.model.reclaim();
    }

    /// Hands `delivery` to its destination.
    fn deliver(&mut self, delivery: Delivery) {
        match delivery {
            Delivery::HandIn => self.hand_in(),
            Delivery::Message => self.message(),
            Delivery::Stop => self.stop(),
            Delivery::Release { item } => self.release(item),
            Delivery::Serve { serial } => {
                if let Some(serving) = self.serving.remove(&serial) {
                    self.serve(serial, serving);
                }
            }
            Delivery::Hub { generation, event } => {
                if generation == self.generation {
                    self.stage.push(event);
                }
            }
            Delivery::Send { serial } => {
                if let Some(sending) = self.sending.remove(&serial) {
                    self.receive(sending);
                }
            }
            Delivery::Ends { item, attempt, serial } => self.ends(item, attempt, serial),
            Delivery::Drop { worker } => self.drop_channel(worker),
            Delivery::Reconnect { worker, episode } => self.reconnect(worker, episode),
            Delivery::Grace { worker, episode, generation } => self.grace(worker, episode, generation),
        }
    }

    /// Routes what the hub asks for.
    fn request(&mut self, request: Request) {
        match request {
            Request::Taken { to } => match self.calls.end(to.into_token().raw()) {
                Call::Take(item) => self.taken_in(item, None),
                Call::Stop | Call::Release(_) => panic!("only a take is answered as taken"),
            },
            Request::Refused { to, refusal } => match self.calls.end(to.into_token().raw()) {
                Call::Take(item) => self.taken_in(item, Some(refusal)),
                Call::Stop | Call::Release(_) => {}
            },
            Request::Stopped { to } => {
                self.calls.end(to.into_token().raw());
            }
            Request::Released { to } => {
                let Call::Release(item) = self.calls.end(to.into_token().raw()) else {
                    panic!("a release is answered as one");
                };
                self.end("released");
                self.refused.remove(&item);
                self.observe(Seen::Released { item });
            }
            Request::Due { owner, item } => {
                self.observe(Seen::Asked { item });
                self.forge(item, Op::Due { owner }, self.settings.deciding);
            }
            Request::Write { owner, item, lifecycle } => {
                let plan = self.plans.get(&item).copied().unwrap_or_default();
                self.forge(item, Op::Write { owner, record: Record { lifecycle, plan } }, self.settings.latency);
            }
            Request::Record { owner, item, attempt, outcome: _ } => {
                self.forge(item, Op::Record { owner, attempt }, self.settings.latency);
            }
            Request::Apply { owner, item, attempt, outcome } => {
                self.forge(item, Op::Apply { owner, attempt, outcome }, self.settings.latency);
            }
            Request::Act { owner, item, action } => {
                let done = self.actions.remove(&action.raw()).expect("an action is one the plan decided");
                self.forge(item, Op::Act { owner, done }, self.settings.latency);
            }
            Request::Start { item, attempt, run: _ } => self.start(item, attempt),
            Request::Cancel { item, attempt } => self.tell_worker(item, attempt, Message::Cancel),
            Request::Relay { item, attempt, event: _ } => self.tell_worker(item, attempt, Message::Relay),
            Request::Keep { .. } => self.stats.kept += 1,
            Request::Acknowledge { item, attempt } => self.forget(item, attempt),
            Request::Stale { item, attempt } => {
                self.end("stale answer");
                self.forget(item, attempt);
            }
            Request::Left { item } => {
                assert!(self.taken.remove(&item), "an item leaves once it was taken in");
                self.plans.remove(&item);
                self.admit();
            }
        }
    }

    /// What the hub tells, counted by kind.
    fn fact(&mut self, fact: Fact) {
        match fact {
            Fact::Held { why, .. } => self.end(match why {
                Hold::Plan(_) => "held: plan",
                Hold::Failures(_) => "held: failures",
                Hold::Stopped => "held: stopped",
                Hold::Acceptance { .. } => "held: acceptance",
                Hold::Writes => "held: writes",
                Hold::Record => "held: record",
            }),
            Fact::Done { .. } => self.end("done"),
            Fact::Parked { .. } => self.end("parked"),
            Fact::Failed { class: Class::Lost, .. } => self.end("lost"),
            Fact::Failed { class: Class::Invalid, .. } => self.end("invalid"),
            Fact::Stale { .. } => self.end("stale"),
            Fact::Taken { .. }
            | Fact::Claimed { .. }
            | Fact::Running { .. }
            | Fact::Ended { .. }
            | Fact::Failed { .. }
            | Fact::Applied { .. }
            | Fact::Released { .. } => {}
        }
    }

    // People.

    /// A person hands an item in, to a repository drawn, needing outcomes
    /// drawn.
    fn hand_in(&mut self) {
        self.items_left -= 1;
        if self.items_left > 0 {
            self.after(self.settings.hand_in_gap, Delivery::HandIn);
        }
        self.stats.items += 1;
        self.numbers += 1;
        let repository = u32::try_from(self.rng.below(u64::from(REPOSITORIES))).expect("few");
        let item = Item { repository, number: self.numbers };
        let issue = Issue {
            record: None,
            closed: false,
            outcomes: BTreeMap::new(),
            needed: u32::try_from(self.rng.between(1, u64::from(self.settings.needed))).expect("few"),
            detours: self.settings.detours,
            holds: 1,
            accepted: BTreeSet::new(),
            releases: RELEASES,
        };
        self.issues.insert(item, issue);
        self.entrance.push_back(item);
        self.admit();
    }

    /// The parent takes the next item at the entrance in, one at a time.
    fn admit(&mut self) {
        if self.taking {
            return;
        }
        let Some(item) = self.entrance.pop_front() else {
            return;
        };
        let issue = &self.issues[&item];
        let read = match issue.record {
            Some(record) => Read::Record(record.lifecycle),
            None => Read::New,
        };
        let plan = match issue.record {
            Some(record) => record.plan,
            None => Plan::default(),
        };
        match stands(issue.record) {
            Stands::Applying => self.end("resumed"),
            Stands::Live | Stands::Claimed | Stands::Held(_) | Stands::Done => {}
        }
        self.plans.insert(item, plan);
        self.taking = true;
        let token = self.wire.name();
        self.calls.open(token, Call::Take(item));
        self.stage.push(Event::Take { reply_to: ReplyTo::new(Token::new(token)), item, read });
    }

    /// The answer to a take: the item is taken in, or waits at the entrance.
    fn taken_in(&mut self, item: Item, refusal: Option<Refusal>) {
        self.taking = false;
        match refusal {
            None => {
                assert!(self.taken.insert(item), "an item is taken in once");
                self.observe(Seen::Taken { item });
                self.admit();
            }
            Some(Refusal::Full) => {
                self.end("full");
                self.plans.remove(&item);
                self.entrance.push_front(item);
            }
            Some(refusal @ (Refusal::Taken | Refusal::Done | Refusal::Unknown | Refusal::Idle | Refusal::Unheld)) => {
                panic!("an item at the entrance is refused only when the working set is full: {refusal:?}")
            }
        }
    }

    /// A person writes on an item drawn: an inbox event, which wakes it now,
    /// later, or not by itself.
    fn message(&mut self) {
        self.messages_left -= 1;
        if self.messages_left > 0 {
            self.after(self.settings.message_gap, Delivery::Message);
        }
        let live: Vec<Item> = self.taken.iter().copied().collect();
        if live.is_empty() {
            return;
        }
        self.stats.messages += 1;
        let item = live[self.pick(live.len())];
        let wake = match self.rng.below(4) {
            0 => None,
            1 => Some(self.now.saturating_add(self.settings.wait.draw(&mut self.rng))),
            _ => Some(self.now),
        };
        let event = Token::new(self.wire.name());
        self.stage.push(Event::Inbox { item, event, wake });
    }

    /// A person stops the run of an item drawn among those claimed.
    fn stop(&mut self) {
        self.stops_left -= 1;
        if self.stops_left > 0 {
            self.after(self.settings.run_time, Delivery::Stop);
        }
        let mut claimed = Vec::new();
        for (item, issue) in &self.issues {
            match stands(issue.record) {
                Stands::Claimed if self.taken.contains(item) => claimed.push(*item),
                Stands::Live | Stands::Claimed | Stands::Applying | Stands::Held(_) | Stands::Done => {}
            }
        }
        if claimed.is_empty() {
            return;
        }
        self.stats.stops += 1;
        let item = claimed[self.pick(claimed.len())];
        let token = self.wire.name();
        self.calls.open(token, Call::Stop);
        self.stage.push(Event::Stop { reply_to: ReplyTo::new(Token::new(token)), item });
    }

    /// A person releases `item`, accepting what waits for their acceptance.
    fn release(&mut self, item: Item) {
        let issue = self.issues.get_mut(&item).expect("a released item was handed in");
        match stands(issue.record) {
            Stands::Held(Hold::Acceptance { .. }) => {
                let attempts = issue.record.expect("held, so recorded").lifecycle.attempts;
                issue.accepted.insert(attempts);
            }
            Stands::Held(Hold::Plan(_) | Hold::Failures(_) | Hold::Stopped | Hold::Writes | Hold::Record)
            | Stands::Live
            | Stands::Claimed
            | Stands::Applying
            | Stands::Done => {}
        }
        self.stats.releases += 1;
        let token = self.wire.name();
        self.calls.open(token, Call::Release(item));
        self.stage.push(Event::Release { reply_to: ReplyTo::new(Token::new(token)), item });
    }

    /// The item is held: a person may release it, a while later.
    fn held(&mut self, item: Item) {
        let issue = self.issues.get_mut(&item).expect("a held item was handed in");
        if issue.releases == 0 || !self.rng.chance(self.settings.releases) {
            return;
        }
        issue.releases -= 1;
        self.after(self.settings.release_after, Delivery::Release { item });
    }

    // The plan and the forge.

    /// Sends `op` for `item` to the plan or the forge, served after `latency`.
    fn forge(&mut self, item: Item, op: Op, latency: Span) {
        self.ops.open(item, ());
        let serial = self.wire.name();
        self.serving.insert(serial, Serving { generation: self.generation, item, op });
        let at = self.now.saturating_add(latency.draw(&mut self.rng));
        self.schedule(at, Delivery::Serve { serial });
    }

    /// Serves the request `serial`: the plan decides, or the forge writes,
    /// unless it fails transiently, and is served again, or for good.
    fn serve(&mut self, serial: u64, serving: Serving) {
        let Serving { generation, item, op } = serving;
        self.trace.log(self.now, format!("forge serves {item:?} {op:?}"));
        let op = match op {
            Op::Due { owner } => {
                self.stats.decisions += 1;
                let due = self.decide(item);
                return self.answer(generation, item, Event::Decided { owner, due }, Span::millis(0, 0));
            }
            op @ (Op::Write { .. } | Op::Record { .. } | Op::Apply { .. } | Op::Act { .. }) => op,
        };
        if self.rng.chance(self.settings.transient) {
            self.stats.transients += 1;
            self.serving.insert(serial, Serving { generation, item, op });
            let at = self.now.saturating_add(self.settings.latency.draw(&mut self.rng));
            self.schedule(at, Delivery::Serve { serial });
            return;
        }
        self.stats.writes += 1;
        let refused = self.rng.chance(self.settings.refused);
        let event = match op {
            Op::Due { .. } => unreachable!("decided above"),
            Op::Write { owner, record } => {
                if refused {
                    self.stats.refusals += 1;
                    self.refused.insert(item);
                    self.observe(Seen::Refused { item });
                    self.held(item);
                    Event::Written { owner, wrote: Wrote::Failed }
                } else {
                    self.write(item, record);
                    Event::Written { owner, wrote: Wrote::Done }
                }
            }
            Op::Record { owner, attempt } => {
                let comment = if refused { None } else { Some(self.post(item, attempt)) };
                Event::Recorded { owner, comment }
            }
            Op::Apply { owner, attempt, outcome } => {
                let applied = self.apply(item, attempt, outcome, refused);
                Event::Applied { owner, applied }
            }
            Op::Act { owner, done } => {
                let acted = self.act(item, done, refused);
                Event::Acted { owner, acted }
            }
        };
        self.answer(generation, item, event, self.settings.latency);
    }

    /// Sends the terminal `event` of an item's request back to the hub of
    /// `generation`, after `latency`, late now and then.
    fn answer(&mut self, generation: u64, item: Item, event: Event, latency: Span) {
        if generation != self.generation {
            return;
        }
        self.ops.end(item);
        let mut back = latency.draw(&mut self.rng);
        if self.rng.chance(self.settings.late) {
            self.stats.lates += 1;
            back = back.saturating_add(self.settings.lateness.draw(&mut self.rng));
        }
        self.schedule(self.now.saturating_add(back), Delivery::Hub { generation, event });
    }

    /// The scripted plan decides what is due for `item`, from the plan's part
    /// of its record as the parent holds it.
    fn decide(&mut self, item: Item) -> Due {
        let plan = self.plans.get(&item).copied().unwrap_or_default();
        let settings = self.settings;
        let draw = u32::try_from(self.rng.below(1000)).expect("per mille");
        let issue = self.issues.get_mut(&item).expect("a decision is about an item handed in");
        if plan.progress >= issue.needed {
            let token = self.wire.name();
            self.actions.insert(token, true);
            return Due::Done { action: Token::new(token) };
        }
        // What the draw falls on, if the item may still take it; a run
        // otherwise.
        let (waits, acts) = (settings.waits, settings.waits + settings.acts);
        if draw < waits && issue.detours > 0 {
            issue.detours -= 1;
            let until = self.now.saturating_add(settings.wait.draw(&mut self.rng));
            return Due::Nothing { until: Some(until) };
        }
        if draw >= waits && draw < acts && issue.detours > 0 {
            issue.detours -= 1;
            let token = self.wire.name();
            self.actions.insert(token, false);
            return Due::Act { action: Token::new(token) };
        }
        if draw >= acts && draw < acts + settings.holds && issue.holds > 0 {
            issue.holds -= 1;
            return Due::Hold { why: Token::new(1) };
        }
        Due::Run { run: Token::new(self.wire.name()) }
    }

    /// The forge stores the item's record.
    fn write(&mut self, item: Item, record: Record) {
        let issue = self.issues.get_mut(&item).expect("a record is written on an item handed in");
        issue.record = Some(record);
        self.observe(Seen::Recorded { item, lifecycle: record.lifecycle });
        match stands(Some(record)) {
            Stands::Held(_) => self.held(item),
            Stands::Live | Stands::Claimed | Stands::Applying | Stands::Done => {}
        }
    }

    /// The forge posts the outcome of the item's `attempt`, keyed by it: one
    /// posted already is found.
    fn post(&mut self, item: Item, attempt: u64) -> u64 {
        let issue = self.issues.get_mut(&item).expect("an outcome is posted on an item handed in");
        if let Some(comment) = issue.outcomes.get(&attempt) {
            return *comment;
        }
        let comment = self.wire.name();
        issue.outcomes.insert(attempt, comment);
        self.create(Key::Outcome { item, attempt });
        comment
    }

    /// The parent applies the outcome of the item's `attempt`: the plan and
    /// the rules judge it, from what the forge holds now (the same each
    /// time), and its writes are made, keyed, those made already found; one
    /// of them may be refused for good.
    fn apply(&mut self, item: Item, attempt: u64, outcome: u64, refused: bool) -> Applied {
        let issue = &self.issues[&item];
        assert_eq!(issue.outcomes.get(&attempt), Some(&outcome), "the outcome applied is the one posted");
        let accepted = issue.accepted.contains(&attempt);
        let mut judge = Rng::new(self.settings.seed ^ item.number.rotate_left(17) ^ attempt.rotate_left(40));
        let settings = self.settings;
        let draw = u32::try_from(judge.below(1000)).expect("per mille");
        let writes = u32::try_from(judge.between(1, 3)).expect("few");
        if draw < settings.stale {
            return Applied::Stale;
        }
        if draw < settings.stale + settings.invalid {
            return Applied::Invalid;
        }
        if !accepted && draw < settings.stale + settings.invalid + settings.accepting {
            return Applied::Accepting;
        }
        let made = if refused { u32::try_from(self.rng.below(u64::from(writes))).expect("few") } else { writes };
        for index in 0..made {
            self.create(Key::Write { item, attempt, index });
        }
        if refused {
            self.stats.refusals += 1;
            return Applied::Failed;
        }
        let plan = self.plans.entry(item).or_default();
        plan.progress += 1;
        if judge.chance(settings.holds) { Applied::Made(Then::Hold(Token::new(2))) } else { Applied::Made(Then::Wait) }
    }

    /// The parent makes an engine action's writes, keyed by the actions made
    /// so far: a merge, or closing a done item.
    fn act(&mut self, item: Item, done: bool, refused: bool) -> Acted {
        let settings = self.settings;
        if self.rng.chance(settings.stale) {
            return Acted::Stale;
        }
        if !done && self.rng.chance(settings.accepting) {
            return Acted::Accepting;
        }
        if refused || self.rng.chance(settings.broken) {
            self.stats.refusals += 1;
            return Acted::Failed;
        }
        let plan = self.plans.get(&item).copied().unwrap_or_default();
        if done {
            self.create(Key::Done { item });
            let issue = self.issues.get_mut(&item).expect("a done item was handed in");
            if !issue.closed {
                issue.closed = true;
                self.observe(Seen::Closed { item });
            }
        } else {
            self.create(Key::Action { item, nth: plan.acts });
            self.plans.entry(item).or_default().acts += 1;
            self.end("acted");
        }
        Acted::Made
    }

    /// The forge makes the creation `key`, unless it finds it made.
    fn create(&mut self, key: Key) {
        if self.created.insert(key) {
            self.observe(Seen::Created { key });
        }
    }

    // The fleet and its workers.

    /// The fleet assigns the item's attempt to a connected worker, or queues
    /// it until one connects.
    fn start(&mut self, item: Item, attempt: u64) {
        let connected: Vec<usize> = (0..self.workers.len()).filter(|worker| self.workers[*worker].connected).collect();
        if connected.is_empty() {
            self.fleet.queued.push_back((item, attempt));
            return;
        }
        let worker = connected[self.pick(connected.len())];
        self.assign(worker, item, attempt);
    }

    fn assign(&mut self, worker: usize, item: Item, attempt: u64) {
        self.fleet.assigned.insert((item, attempt), worker);
        self.send(worker, item, attempt, Message::Start);
    }

    /// The fleet sends `message` about the item's attempt to the worker that
    /// has it, if it knows one.
    fn tell_worker(&mut self, item: Item, attempt: u64, message: Message) {
        if let Some(worker) = self.fleet.assigned.get(&(item, attempt)).copied() {
            self.send(worker, item, attempt, message);
        }
    }

    /// The engine has the answer of the item's attempt, or drops it: the
    /// worker that sent it may forget it.
    fn forget(&mut self, item: Item, attempt: u64) {
        if let Some(worker) = self.fleet.heard.remove(&(item, attempt)) {
            self.send(worker, item, attempt, Message::Forget);
        }
    }

    fn send(&mut self, worker: usize, item: Item, attempt: u64, message: Message) {
        let serial = self.wire.name();
        let episode = self.workers[worker].episode;
        self.sending.insert(serial, Sending { worker, episode, item, attempt, message });
        self.after(self.settings.channel, Delivery::Send { serial });
    }

    /// A message reaches its worker, if its channel is up.
    fn receive(&mut self, sending: Sending) {
        let Sending { worker, episode, item, attempt, message } = sending;
        if !self.workers[worker].connected || self.workers[worker].episode != episode {
            return;
        }
        match message {
            Message::Start => self.take_run(worker, item, attempt),
            Message::Cancel => self.cancel_run(item, attempt),
            Message::Relay => {
                if self.courses.contains_key(&(item, attempt)) {
                    self.stats.relayed += 1;
                }
            }
            Message::Forget => {
                self.workers[worker].answers.remove(&(item, attempt));
            }
        }
    }

    /// A worker takes a run, and draws its course: how long it takes, how it
    /// answers, and whether the worker drops as it runs.
    fn take_run(&mut self, worker: usize, item: Item, attempt: u64) {
        self.trace.log(self.now, format!("worker {worker} takes {item:?} {attempt}"));
        self.stats.runs += 1;
        self.observe(Seen::Started { item, attempt });
        self.workers[worker].runs.insert((item, attempt));
        let settings = self.settings;
        let draw = u32::try_from(self.rng.below(1000)).expect("per mille");
        let answer = if draw < settings.parks {
            let snapshot = if self.rng.chance(500) { Some(Token::new(self.wire.name())) } else { None };
            Answer::Parked { snapshot }
        } else if draw < settings.parks + settings.fails {
            let classes = [Class::Transient, Class::Permanent, Class::Run, Class::Agent];
            Answer::Failed(classes[self.pick(classes.len())])
        } else {
            Answer::Ended { outcome: Token::new(self.wire.name()) }
        };
        let took = self.settings.run_time.draw(&mut self.rng);
        let serial = self.wire.name();
        self.schedule(self.now.saturating_add(took), Delivery::Ends { item, attempt, serial });
        self.courses.insert((item, attempt), Course { worker, answer, serial });
        if self.rng.chance(settings.drops) {
            let at = Duration::from_nanos(self.rng.below(took.as_nanos().max(1)));
            self.schedule(self.now.saturating_add(at), Delivery::Drop { worker });
        }
        self.tell_engine(worker, Event::Running { item, attempt });
    }

    /// A worker cancels a run it hosts: it answers as cancelled soon, unless
    /// its own answer wins the race.
    fn cancel_run(&mut self, item: Item, attempt: u64) {
        let Some(course) = self.courses.get_mut(&(item, attempt)) else {
            return;
        };
        if self.rng.chance(300) {
            return;
        }
        course.answer = Answer::Failed(Class::Transient);
        let serial = self.wire.name();
        course.serial = serial;
        self.end("cancelled");
        self.after(Span::millis(1, 200), Delivery::Ends { item, attempt, serial });
    }

    /// A run's course ends: its worker answers, and keeps the answer until
    /// the engine says to forget it.
    fn ends(&mut self, item: Item, attempt: u64, serial: u64) {
        self.trace.log(self.now, format!("course ends {item:?} {attempt} {serial}"));
        let ended = match self.courses.get(&(item, attempt)) {
            Some(course) => course.serial == serial,
            None => false,
        };
        if !ended {
            return;
        }
        let Course { worker, answer, .. } = self.courses.remove(&(item, attempt)).expect("checked above");
        self.observe(Seen::Finished { item, attempt });
        let hosted = &mut self.workers[worker];
        hosted.runs.remove(&(item, attempt));
        hosted.answers.insert((item, attempt), answer);
        if hosted.connected {
            self.answer_engine(worker, item, attempt, answer);
        }
    }

    fn answer_engine(&mut self, worker: usize, item: Item, attempt: u64, answer: Answer) {
        self.stats.answers += 1;
        self.tell_engine(worker, Event::Answered { item, attempt, answer });
    }

    /// The fleet hears `event` from `worker`, and hands it to the hub.
    fn tell_engine(&mut self, worker: usize, event: Event) {
        match &event {
            Event::Running { item, attempt } => {
                self.fleet.assigned.insert((*item, *attempt), worker);
            }
            Event::Answered { item, attempt, .. } => {
                self.fleet.assigned.remove(&(*item, *attempt));
                self.fleet.heard.insert((*item, *attempt), worker);
            }
            Event::Take { .. }
            | Event::Stop { .. }
            | Event::Release { .. }
            | Event::Inbox { .. }
            | Event::Decided { .. }
            | Event::Written { .. }
            | Event::Recorded { .. }
            | Event::Applied { .. }
            | Event::Acted { .. } => unreachable!("a worker says only that a run runs or answers"),
        }
        let generation = self.generation;
        self.after(self.settings.channel, Delivery::Hub { generation, event });
    }

    /// A worker's channel drops. It comes back within the fleet's grace with
    /// its runs, or it crashed: its runs and the answers it kept are gone, and
    /// it comes back empty, later.
    fn drop_channel(&mut self, worker: usize) {
        if !self.workers[worker].connected {
            return;
        }
        let returns = self.rng.chance(self.settings.returns);
        self.disconnect(worker, returns, self.settings.grace);
        let generation = self.generation;
        let episode = self.workers[worker].episode;
        let at = self.now.saturating_add(self.settings.grace);
        self.schedule(at, Delivery::Grace { worker, episode, generation });
    }

    /// Takes `worker` off its channel; it comes back within `grace` if it
    /// `returns`, and crashed otherwise.
    fn disconnect(&mut self, worker: usize, returns: bool, grace: Duration) {
        self.trace.log(self.now, format!("worker {worker} drops, returns {returns}"));
        let hosted = &mut self.workers[worker];
        hosted.connected = false;
        hosted.episode += 1;
        let episode = hosted.episode;
        let back = if returns {
            Duration::from_nanos(self.rng.below(grace.as_nanos() / 2).max(1))
        } else {
            let runs = std::mem::take(&mut hosted.runs);
            hosted.answers.clear();
            for (item, attempt) in runs {
                self.courses.remove(&(item, attempt));
                self.observe(Seen::Finished { item, attempt });
            }
            grace.saturating_add(self.settings.channel.draw(&mut self.rng))
        };
        self.schedule(self.now.saturating_add(back), Delivery::Reconnect { worker, episode });
    }

    /// A worker reconnects: it says which runs it hosts and sends again the
    /// answers it kept; an assigned run it does not have is lost. Queued
    /// starts go to it.
    fn reconnect(&mut self, worker: usize, episode: u64) {
        self.trace.log(self.now, format!("worker {worker} reconnects {episode}"));
        if self.workers[worker].episode != episode {
            return;
        }
        self.workers[worker].connected = true;
        let runs: Vec<(Item, u64)> = self.workers[worker].runs.iter().copied().collect();
        let answers: Vec<((Item, u64), Answer)> =
            self.workers[worker].answers.iter().map(|(run, answer)| (*run, *answer)).collect();
        let assigned: Vec<(Item, u64)> =
            self.fleet.assigned.iter().filter(|(_, at)| **at == worker).map(|(run, _)| *run).collect();
        for (item, attempt) in assigned {
            if !self.workers[worker].runs.contains(&(item, attempt))
                && !self.workers[worker].answers.contains_key(&(item, attempt))
            {
                self.lost(item, attempt);
            }
        }
        for (item, attempt) in runs {
            if self.generation > 0 && !self.fleet.assigned.contains_key(&(item, attempt)) {
                self.end("adopted");
            }
            self.tell_engine(worker, Event::Running { item, attempt });
        }
        for ((item, attempt), answer) in answers {
            self.answer_engine(worker, item, attempt, answer);
        }
        while let Some((item, attempt)) = self.fleet.queued.pop_front() {
            self.assign(worker, item, attempt);
        }
    }

    /// The fleet's grace for a worker that dropped is over: if it has not come
    /// back, its runs are presumed lost.
    fn grace(&mut self, worker: usize, episode: u64, generation: u64) {
        let hosted = &self.workers[worker];
        if generation != self.generation || hosted.episode != episode || hosted.connected {
            return;
        }
        let assigned: Vec<(Item, u64)> =
            self.fleet.assigned.iter().filter(|(_, at)| **at == worker).map(|(run, _)| *run).collect();
        for (item, attempt) in assigned {
            self.lost(item, attempt);
        }
    }

    fn lost(&mut self, item: Item, attempt: u64) {
        self.trace.log(self.now, format!("fleet loses {item:?} {attempt}"));
        self.fleet.assigned.remove(&(item, attempt));
        let generation = self.generation;
        self.after(
            Span::millis(0, 0),
            Delivery::Hub { generation, event: Event::Answered { item, attempt, answer: Answer::Lost } },
        );
    }

    // Restarts.

    /// The engine stops and starts again, cold: what it asked of the forge
    /// and had not been served lands now or never; what it sent the workers
    /// arrives now or never; its memory is gone. Every worker's channel
    /// drops: each comes back within the hub's grace with its runs, or
    /// crashed. The new process takes in every live item from its record.
    fn restart(&mut self) {
        self.trace.log(self.now, "the engine restarts");
        self.stats.restarts += 1;
        // What was in flight lands, or arrives, for the old process: what a
        // worker says back goes to it, and is lost.
        let serving = std::mem::take(&mut self.serving);
        for (_, Serving { item, op, .. }) in serving {
            if self.rng.chance(500) {
                self.land(item, op);
            }
        }
        let sending = std::mem::take(&mut self.sending);
        for (_, sending) in sending {
            if self.rng.chance(500) {
                self.receive(sending);
            }
        }
        self.generation += 1;
        self.refused.clear();
        self.observe(Seen::Restarted);
        let grace = self.settings.limits.grace;
        for worker in 0..self.workers.len() {
            if self.workers[worker].connected {
                let returns = self.rng.chance(self.settings.returns);
                self.disconnect(worker, returns, grace);
            }
        }
        // The new process.
        let seed = self.settings.seed ^ self.generation.rotate_left(32);
        self.model = Model::new(&self.settings.limits, seed);
        let max_out = work::max_out(&self.settings.limits);
        let limits = self.settings.limits;
        self.stage = Stage::new(limits, max_out, max_out + SLACK);
        // It starts within this iteration: every step in it sees its time.
        self.stage.tick(self.now);
        self.plans.clear();
        self.taken.clear();
        self.taking = false;
        self.actions.clear();
        self.calls = Ledger::new("call");
        self.ops = Ledger::new("request of an item");
        self.fleet = Fleet::default();
        // Every live item, those with a record first: they were taken in
        // before, so they fit.
        let mut recorded = Vec::new();
        let mut new = Vec::new();
        for (item, issue) in &self.issues {
            match (issue.record, stands(issue.record)) {
                (_, Stands::Done) => {}
                (Some(_), Stands::Live | Stands::Claimed | Stands::Applying | Stands::Held(_)) => recorded.push(*item),
                (None, Stands::Live | Stands::Claimed | Stands::Applying | Stands::Held(_)) => new.push(*item),
            }
        }
        assert!(
            u32::try_from(recorded.len()).expect("few") <= self.settings.limits.items,
            "the items with a record were all taken in before"
        );
        self.entrance = recorded.into_iter().chain(new).collect();
        self.admit();
    }

    /// A request the old process made lands without its answer.
    fn land(&mut self, item: Item, op: Op) {
        match op {
            Op::Due { .. } => {}
            Op::Write { record, .. } => self.write(item, record),
            Op::Record { attempt, .. } => {
                self.post(item, attempt);
            }
            Op::Apply { attempt, outcome, .. } => {
                let refused = self.rng.chance(500);
                self.apply(item, attempt, outcome, refused);
            }
            Op::Act { done, .. } => {
                self.act(item, done, false);
            }
        }
    }

    // What the world shares.

    fn after(&mut self, span: Span, delivery: Delivery) {
        let at = self.now.saturating_add(span.draw(&mut self.rng));
        self.schedule(at, delivery);
    }

    /// Sends `delivery`, due at `at`. What is on its way is bounded: a world
    /// that outgrows it is stuck, and fails with its seed.
    fn schedule(&mut self, at: Time, delivery: Delivery) {
        self.pending += 1;
        let seed = self.settings.seed;
        assert!(self.pending <= MAX_PENDING, "seed {seed}: more than {MAX_PENDING} deliveries on their way");
        self.wire.send(at, delivery);
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
        for stimulus in stimuli {
            match stimulus {
                Stimulus::Restart => self.restart(),
            }
        }
    }

    fn pick(&mut self, len: usize) -> usize {
        usize::try_from(self.rng.below(u64::try_from(len).expect("small"))).expect("small")
    }

    fn has_work_now(&self) -> bool {
        self.stage.has_events()
            || self.model.is_due(self.now)
            || self.wire.is_due(self.now)
            || self.referee.is_due(self.now)
    }

    fn next_time(&self) -> Option<Time> {
        [self.wire.next_time(), self.referee.next_deadline(), self.model.next_deadline()].into_iter().flatten().min()
    }

    /// The invariants of a world where nothing is left to happen.
    fn assert_settled(&self) {
        let seed = self.settings.seed;
        assert!(self.wire.is_empty() && !self.stage.has_events(), "seed {seed}: nothing is on its way");
        self.calls.assert_settled();
        self.ops.assert_settled();
        assert!(!self.taking, "seed {seed}: no take is in flight");
        let mut held = 0;
        for (item, issue) in &self.issues {
            let ended = match stands(issue.record) {
                Stands::Done => true,
                Stands::Held(_) => {
                    held += 1;
                    true
                }
                Stands::Live | Stands::Claimed | Stands::Applying => {
                    let refused = self.refused.contains(item);
                    held += u32::from(refused);
                    refused || self.entrance.contains(item)
                }
            };
            assert!(ended, "seed {seed}: {item:?} is done, held, or waits at the entrance");
        }
        // Held items are live work, kept until a person releases them: only
        // a working set full of them keeps an item at the entrance.
        if !self.entrance.is_empty() {
            assert_eq!(held, self.settings.limits.items, "seed {seed}: items wait at the entrance only for held ones");
        }
        for (worker, hosted) in self.workers.iter().enumerate() {
            assert!(hosted.runs.is_empty(), "seed {seed}: worker {worker} runs nothing");
            assert!(hosted.answers.is_empty(), "seed {seed}: worker {worker} keeps no answer: {:?}", hosted.answers);
        }
        assert_eq!(self.model.next_deadline(), None, "seed {seed}: no item waits for an alarm");
        self.referee.assert_passed(seed);
    }
}

/// How long an item may take to end: every outcome it needs, each after its
/// detours and as many failed runs as its classes allow, each run as long as
/// a run takes, its retries backing off, its worker lost past the grace, the
/// engine restarting; each step of it a round of writes on a slow forge. With
/// room to spare.
fn end_within(settings: &Settings) -> Duration {
    let limits = settings.limits;
    let write = settings.latency.max.saturating_mul(2).saturating_add(settings.lateness.max).saturating_mul(4);
    let round = settings
        .run_time
        .max
        .saturating_add(settings.grace.saturating_mul(2))
        .saturating_add(limits.grace)
        .saturating_add(settings.wait.max)
        .saturating_add(write.saturating_mul(8));
    let retries = Duration::from_secs(8).saturating_add(round).saturating_mul(12);
    let outcomes = u64::from(settings.needed + settings.detours + 2);
    round.saturating_mul(outcomes).saturating_add(retries).saturating_mul(u64::from(settings.restarts) + 2)
}

/// Where an item stands, as its record on the forge says.
#[derive(Clone, Copy, Debug)]
enum Stands {
    /// No record yet, or waiting, parked or retrying.
    Live,
    Claimed,
    Applying,
    Held(Hold),
    Done,
}

fn stands(record: Option<Record>) -> Stands {
    let Some(record) = record else {
        return Stands::Live;
    };
    match record.lifecycle.phase {
        Phase::Waiting | Phase::Parked | Phase::Retrying(_) => Stands::Live,
        Phase::Claimed => Stands::Claimed,
        Phase::Applying { .. } => Stands::Applying,
        Phase::Held(why) => Stands::Held(why),
        Phase::Done => Stands::Done,
    }
}
