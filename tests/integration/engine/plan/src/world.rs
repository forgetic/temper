use std::collections::BTreeMap;

use temper_engine_model_plan::{
    self as plan, Accept, Applied, Config, Due, Hold, Key, Limits, Outcome, Progress, Record, Repair, Stale, Then,
    Verdict, Waits, Why, Woken, Work, Write,
};
use temper_lib::{Duration, Env, Queue, Rng, Time};
use temper_world::{Ledger, Schedule, Span, Trace};

use crate::forge::{Forge, Item, Keyed, Made, Pull, Pushed, State};
use crate::referee::{Referee, Seen};
use crate::script::{self, Script};
use crate::translate::{self, Heard, approvals_needed, commit, count};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Settings {
    /// Seeds the world, which seeds the referee.
    pub seed: u64,
    pub limits: Limits,
    /// Goals begun: sessions opened with a person's first message.
    pub goals: u32,
    /// The time between one goal beginning and the next.
    pub spacing: Span,
    pub script: Script,
    /// Runs a step gets before its item is held for a person.
    pub attempts: u32,
    /// The wait before a failed run is tried again.
    pub backoff: Span,
    /// The chance, per mille, that the engine restarts as it applies an
    /// outcome.
    pub restarts: u32,
    /// How long, in simulated time, every goal and task has to end.
    pub bound: Duration,
    /// The most tasks the sessions make, all together.
    pub tasks: u32,
}

impl Settings {
    /// A world where nothing goes wrong: every plan is valid and accepted,
    /// CI passes, people approve and accept, nothing conflicts or moves, and
    /// no run fails or escalates.
    #[must_use]
    pub const fn calm(seed: u64) -> Settings {
        Settings {
            seed,
            limits: Limits {
                steps: 12,
                name_bytes: 32,
                dependencies: 3,
                gates: 2,
                targets: 2,
                instruction_bytes: 256,
                tasks: 2,
                events: 16,
                repairs: 3,
                budget: script::BUDGET,
            },
            goals: 2,
            spacing: Span::millis(0, 600_000),
            script: Script {
                steps: (2, 6),
                changes: 400,
                waits: 150,
                grows: 0,
                dependencies: 400,
                agent_reviews: 300,
                gates: 0,
                invalid: 0,
                growth: 0,
                beyond: 0,
                tasks: 0,
                escalations: 0,
                failures: 0,
                verdicts: 0,
                run: Span::millis(60_000, 1_800_000),
                ci_fails: 0,
                ci: Span::millis(60_000, 900_000),
                conflicts: 0,
                mergeable: Span::millis(1_000, 30_000),
                changes_asked: 0,
                people: Span::millis(60_000, 3_600_000),
                rejects: 0,
                proposals: 0,
                closes: 0,
                pushes: 0,
            },
            attempts: 3,
            backoff: Span::millis(10_000, 600_000),
            restarts: 0,
            bound: Duration::from_secs(30 * 86_400),
            tasks: 0,
        }
    }

    /// A world where everything that can go wrong does, now and then: a
    /// world for the random sweep.
    #[must_use]
    pub const fn rough(seed: u64) -> Settings {
        let calm = Settings::calm(seed);
        Settings {
            goals: 3,
            script: Script {
                grows: 300,
                gates: 250,
                invalid: 250,
                growth: 300,
                beyond: 300,
                tasks: 200,
                escalations: 15,
                failures: 60,
                verdicts: 250,
                ci_fails: 200,
                conflicts: 100,
                changes_asked: 150,
                rejects: 80,
                proposals: 300,
                closes: 10,
                pushes: 150,
                ..calm.script
            },
            restarts: 150,
            tasks: 3,
            ..calm
        }
    }
}

/// What happened, for the tests to look at.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct Stats {
    /// How each goal and task ended.
    pub endings: BTreeMap<String, u32>,
    /// The paths items took, by name, and how many times.
    pub paths: BTreeMap<String, u32>,
    /// Items made.
    pub items: u32,
    /// Observations the referee checked.
    pub checked: u32,
}

/// What an item is doing, as the engine keeps it in memory.
#[derive(Debug)]
enum Phase {
    Idle,
    /// A run of `attempt` is out, taking the first `relayed` events of the
    /// inbox.
    Running {
        attempt: u64,
        relayed: usize,
    },
    /// Its outcome waits for a person to accept it.
    Proposed {
        attempt: u64,
        outcome: Outcome,
        relayed: usize,
    },
    Held,
    Closed,
}

/// The engine's memory of a live item.
#[derive(Debug)]
struct Life {
    phase: Phase,
    /// The attempts its runs had: the record's count, which only grows.
    attempts: u64,
    /// Runs of its step that failed.
    failures: u32,
    /// What came in since its inbox position.
    inbox: Vec<(Heard, Time)>,
    last_turn: Time,
    /// When an alarm is armed for it, if one is.
    alarm: Option<Time>,
    /// Not before then: a failed run's backoff.
    retry: Option<Time>,
    /// Whether the engine holds a parked run's snapshot.
    snapshot: bool,
    /// Whether a person was asked to decide on it.
    asked: bool,
    /// Proposals of its a person rejected.
    rejected: u32,
    /// Whether its last outcome was invalid: its next run takes the feedback.
    fixing: bool,
}

impl Life {
    fn new(now: Time) -> Life {
        Life {
            phase: Phase::Idle,
            attempts: 0,
            failures: 0,
            inbox: Vec::new(),
            last_turn: now,
            alarm: None,
            retry: None,
            snapshot: false,
            asked: false,
            rejected: 0,
            fixing: false,
        }
    }
}

/// Something on its way, delivered at its time.
#[derive(Debug)]
enum Delivery {
    /// A goal begins: a person opens a session.
    Begin,
    /// A run ends.
    Ends { item: u64, attempt: u64 },
    /// CI reports on a head.
    Ci { item: u64, head: u64 },
    /// The forge works out whether a head merges into a head of its base.
    Mergeable { item: u64, head: u64, base: u64 },
    /// People review a head.
    Review { item: u64, head: u64 },
    /// A person decides on an item.
    Decide { item: u64 },
    /// A person decides on an outcome waiting for acceptance.
    Proposal { item: u64, attempt: u64 },
    /// A person closes a pull request.
    ClosePull { item: u64 },
    /// A person pushes to a branch changes land into.
    PushBase { repository: u32, base: Vec<u8> },
    /// A person pushes to an item's branch.
    PushBranch { item: u64 },
    /// An item's alarm.
    Alarm { item: u64 },
}

/// What a write comes from: an engine action, or an outcome.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Origin {
    Action,
    Outcome { attempt: u64, kind: Kind, accepted: bool },
}

/// What kind of outcome makes items, and so whose they are.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Plan,
    Steps,
    Tasks,
    Other,
}

pub struct World {
    now: Time,
    /// Draws what runs and people do, and latencies.
    rng: Rng,
    settings: Settings,
    config: Config,
    env: Env<Limits>,

    forge: Forge,
    lives: BTreeMap<u64, Life>,
    wire: Schedule<Delivery>,
    /// Runs out, each ending once.
    runs: Ledger<(u64, u64), Why>,
    referee: Referee,

    /// Names growth and tasks.
    serial: u64,
    tasks: u32,
    /// Items held, in the order they were.
    held: Vec<(u64, &'static str)>,
    /// Whether an item acted in the last pass: the loop goes round again.
    acted: bool,

    stats: Stats,
    trace: Trace,
}

impl World {
    #[must_use]
    pub fn new(settings: Settings) -> World {
        assert!(plan::worst_case(&settings.limits).is_some(), "the shell refuses limits it cannot provision");
        let mut rng = Rng::new(settings.seed);
        let referee = Referee::new(script::config(), rng.next_u64(), settings.restarts, settings.bound);
        let mut world = World {
            now: Time::ZERO,
            rng,
            settings,
            config: script::config(),
            env: Env { now: Time::ZERO, limits: settings.limits },
            forge: Forge::new(&script::BASES),
            lives: BTreeMap::new(),
            wire: Schedule::new(),
            runs: Ledger::new("run"),
            referee,
            serial: 0,
            tasks: 0,
            held: Vec::new(),
            acted: false,
            stats: Stats::default(),
            trace: Trace::default(),
        };
        let mut at = Time::ZERO;
        for _ in 0..settings.goals {
            at = at.saturating_add(settings.spacing.draw(&mut world.rng));
            world.wire.send(at, Delivery::Begin);
        }
        world
    }

    #[must_use]
    pub fn now(&self) -> Time {
        self.now
    }

    #[must_use]
    pub fn stats(&self) -> Stats {
        Stats { checked: self.referee.checked, ..self.stats.clone() }
    }

    /// What crossed between the plan and the world, in order, with times.
    #[must_use]
    pub fn trace(&self) -> &[String] {
        self.trace.lines()
    }

    /// Runs until nothing is left to happen, then checks the invariants of a
    /// settled world and the referee's verdict. Panics if it takes more than
    /// `iterations`.
    pub fn run(&mut self, iterations: u32) {
        for _ in 0..iterations {
            self.iterate();
            if self.acted || self.wire.is_due(self.now) {
                continue;
            }
            let next = match (self.wire.next_time(), self.referee.next_deadline()) {
                (Some(wire), Some(deadline)) => wire.min(deadline),
                (Some(at), None) | (None, Some(at)) => at,
                (None, None) => {
                    self.assert_settled();
                    return;
                }
            };
            assert!(next > self.now, "time moves forward");
            self.now = next;
        }
        panic!("the world did not settle in {iterations} iterations");
    }

    /// One iteration: what is due arrives, the referee's deadlines fire, and
    /// the engine asks the plan about every live item once.
    fn iterate(&mut self) {
        self.env.now = self.now;
        while let Some(delivery) = self.wire.next(self.now) {
            self.deliver(delivery);
        }
        self.referee.fire(self.now, &self.forge);
        self.acted = false;
        let live: Vec<u64> = self.lives.keys().copied().collect();
        for number in live {
            let acted = self.decide(number);
            self.acted = self.acted || acted;
        }
    }

    fn path(&mut self, path: impl Into<String>) {
        *self.stats.paths.entry(path.into()).or_default() += 1;
    }

    fn log(&mut self, line: impl std::fmt::Display) {
        self.trace.log(self.now, line);
    }

    fn observe(&mut self, seen: &Seen) {
        self.referee.observe(self.now, &self.forge, seen);
    }

    fn life(&mut self, number: u64) -> &mut Life {
        self.lives.get_mut(&number).expect("a live item")
    }

    /// Asks the plan what is due for item `number`, if it is idle, and does
    /// it. Returns whether it acted.
    fn decide(&mut self, number: u64) -> bool {
        let life = &self.lives[&number];
        match life.phase {
            Phase::Idle => {}
            Phase::Running { .. } | Phase::Proposed { .. } | Phase::Held | Phase::Closed => return false,
        }
        if life.retry.is_some_and(|retry| retry > self.now) {
            return false;
        }
        let snapshot = life.snapshot;
        let woken = match self.forge.item(number).record.step.work {
            Work::Session(_) => self.woken(number),
            Work::Agent(_) | Work::Change(_) | Work::Wait(_) => false,
        };
        let facts = translate::facts(&self.forge, number, snapshot, woken);
        let mut out = Queue::with_capacity(plan::max_out(&self.settings.limits));
        let due = plan::due(&self.config, &self.env, &self.forge.item(number).record, &facts, &mut out);
        let writes = drain(&mut out);
        match due {
            Due::Nothing(waits) => {
                self.waiting(number, waits);
                false
            }
            Due::Run(run) => {
                self.start(number, run.why, run.resume);
                true
            }
            Due::Act(action) => {
                self.log(format_args!("item {number}: {action:?}"));
                self.write(number, Origin::Action, writes);
                true
            }
            Due::Done => {
                self.log(format_args!("item {number}: done"));
                self.write(number, Origin::Action, writes);
                true
            }
            Due::Hold(hold) => {
                self.hold(number, hold_name(hold));
                true
            }
        }
    }

    /// Whether a session's inbox wakes it now; arms an alarm for later.
    fn woken(&mut self, number: u64) -> bool {
        let life = &self.lives[&number];
        let mut inbox = Vec::new();
        for (heard, at) in &life.inbox {
            inbox.push(translate::inbound(*heard, *at));
        }
        let rule = self.forge.item(number).record.step.wake;
        match plan::wake(&self.env, &rule, &inbox, life.last_turn) {
            Woken::Now => true,
            Woken::At(at) => {
                self.alarm(number, at);
                false
            }
            Woken::No => false,
        }
    }

    /// An item waits: on a time, which arms an alarm; on a person, who is
    /// asked once; on anything else, which comes by itself.
    fn waiting(&mut self, number: u64, waits: Waits) {
        match waits {
            Waits::Time(at) => self.alarm(number, at),
            Waits::Acceptance | Waits::Decision => {
                if !self.lives[&number].asked {
                    self.life(number).asked = true;
                    let at = self.later(self.settings.script.people);
                    self.wire.send(at, Delivery::Decide { item: number });
                }
            }
            Waits::Dependencies
            | Waits::Children
            | Waits::Ci
            | Waits::Review
            | Waits::Approvals
            | Waits::Mergeable
            | Waits::Wake => {}
        }
    }

    fn alarm(&mut self, number: u64, at: Time) {
        if self.lives[&number].alarm == Some(at) {
            return;
        }
        self.life(number).alarm = Some(at);
        self.wire.send(at, Delivery::Alarm { item: number });
    }

    fn later(&mut self, span: Span) -> Time {
        self.now.saturating_add(span.draw(&mut self.rng))
    }

    /// Starts a run for item `number`.
    fn start(&mut self, number: u64, why: Why, resume: bool) {
        let now = self.now;
        let life = self.life(number);
        life.attempts += 1;
        let attempt = life.attempts;
        life.phase = Phase::Running { attempt, relayed: life.inbox.len() };
        life.last_turn = now;
        self.runs.open((number, attempt), why);
        self.observe(&Seen::Ran(number));
        self.path(format!("run: {}", why_name(why)));
        if resume {
            self.path("run: resumes a snapshot");
        }
        self.log(format_args!("item {number}: run {attempt}, {}", why_name(why)));
        let at = self.later(self.settings.script.run);
        self.wire.send(at, Delivery::Ends { item: number, attempt });
        // A person may push to the branch of a change under review: the
        // verdict comes back on a head that is no longer the branch's.
        if let Why::Review { .. } = why
            && self.rng.chance(self.settings.script.pushes)
        {
            let during = self.now.saturating_add(Duration::from_millis(1));
            self.wire.send(during, Delivery::PushBranch { item: number });
        }
    }

    fn deliver(&mut self, delivery: Delivery) {
        match delivery {
            Delivery::Begin => self.begin(),
            Delivery::Ends { item, attempt } => self.ends(item, attempt),
            Delivery::Ci { item, head } => self.ci(item, head),
            Delivery::Mergeable { item, head, base } => {
                let conflicts = self.rng.chance(self.settings.script.conflicts);
                if let Some(pull) = self.forge.pulls.get_mut(&item) {
                    pull.conflicts.insert((head, base), conflicts);
                }
            }
            Delivery::Review { item, head } => self.review(item, head),
            Delivery::Decide { item } => {
                if self.forge.is_closed(item) {
                    return;
                }
                let accepted = !self.rng.chance(self.settings.script.rejects);
                self.forge.item_mut(item).decision = Some(accepted);
                self.log(format_args!("item {item}: a person decides, accepting: {accepted}"));
                self.path(if accepted { "decision: accepted" } else { "decision: rejected" });
            }
            Delivery::Proposal { item, attempt } => self.proposal(item, attempt),
            Delivery::ClosePull { item } => {
                let pull = self.forge.pulls.get_mut(&item).expect("a pull request opened");
                if pull.state == State::Open {
                    pull.state = State::Closed;
                    self.log(format_args!("item {item}: a person closes its pull request"));
                }
            }
            Delivery::PushBase { repository, base } => {
                let head = self.forge.move_base(repository, &base);
                self.log(format_args!("a person pushes {head} to {}", String::from_utf8_lossy(&base)));
                self.path("base moved by a person");
                self.base_moved(repository, &base, head);
            }
            Delivery::PushBranch { item } => {
                let repository = self.forge.item(item).repository;
                let base = self.forge.pulls[&item].base.clone();
                let on = self.forge.base(repository, &base);
                let head = self.forge.commit();
                self.forge.item_mut(item).branch = Some(Pushed { head, on });
                self.log(format_args!("item {item}: a person pushes {head}"));
                self.path("branch pushed by a person");
                self.watch(item, false);
            }
            Delivery::Alarm { item } => {
                if let Some(life) = self.lives.get_mut(&item)
                    && life.alarm == Some(self.now)
                {
                    life.alarm = None;
                }
            }
        }
    }

    /// A goal begins: a person opens a session, with a first message.
    fn begin(&mut self) {
        self.serial += 1;
        let item = Item {
            repository: 0,
            record: Record { step: script::session(self.serial), progress: Progress::NEW, goal: None },
            goal: None,
            parent: None,
            created: self.now,
            closed: None,
            held: None,
            branch: None,
            decision: None,
        };
        let (number, _) = self.forge.create(None, item);
        let mut life = Life::new(self.now);
        life.inbox.push((Heard::Message, self.now));
        self.lives.insert(number, life);
        self.stats.items += 1;
        self.observe(&Seen::Began(number));
        self.log(format_args!("item {number}: a person opens a session"));
    }

    /// A run ends: it failed, or finished with an outcome, which is applied.
    fn ends(&mut self, number: u64, attempt: u64) {
        let why = self.runs.end((number, attempt));
        let relayed = match self.lives[&number].phase {
            Phase::Running { attempt: running, relayed } if running == attempt => relayed,
            Phase::Running { .. } | Phase::Idle | Phase::Proposed { .. } | Phase::Held | Phase::Closed => {
                panic!("item {number}'s run {attempt} ends while it runs")
            }
        };
        if self.rng.chance(self.settings.script.failures) {
            self.failed(number);
            return;
        }
        if let Why::Turn = why {
            // A session parks after some turns, and the engine keeps its
            // snapshot.
            let parks = self.rng.chance(500);
            self.life(number).snapshot = parks;
        }
        let outcome = self.outcome(number, why);
        self.log(format_args!("item {number}: run {attempt} ends: {}", outcome_name(&outcome)));
        self.path(format!("outcome: {}", outcome_name(&outcome)));
        let fixing = self.lives[&number].fixing;
        self.life(number).fixing = false;
        let (applied, writes) = self.apply(number, &outcome);
        match applied {
            Applied::Writes { accept, then } => {
                let proposal = match &outcome {
                    Outcome::Plan(_) => true,
                    Outcome::Change { .. }
                    | Outcome::Verdict { .. }
                    | Outcome::Report
                    | Outcome::Steps(_)
                    | Outcome::Tasks(_)
                    | Outcome::Reply
                    | Outcome::Escalation => accept == Accept::Person,
                };
                if proposal {
                    self.path("proposed");
                    self.life(number).phase = Phase::Proposed { attempt, outcome, relayed };
                    let at = self.later(self.settings.script.people);
                    self.wire.send(at, Delivery::Proposal { item: number, attempt });
                    return;
                }
                let kind = kind(&outcome);
                self.commit(number, attempt, &outcome, kind, writes, then, relayed, false);
            }
            Applied::Stale(stale) => {
                self.path(format!("stale: {}", stale_name(stale)));
                self.life(number).phase = Phase::Idle;
            }
            Applied::Invalid(problems) => {
                assert!(!fixing, "item {number}'s run took the feedback: {problems:?}");
                self.path("invalid");
                self.log(format_args!("item {number}: invalid: {problems:?}"));
                let life = self.life(number);
                life.fixing = true;
                life.phase = Phase::Idle;
            }
        }
    }

    /// A run failed: tried again after a backoff, or, past its attempts, held.
    fn failed(&mut self, number: u64) {
        self.path("run failed");
        let backoff = self.settings.backoff.draw(&mut self.rng);
        let now = self.now;
        let attempts = self.settings.attempts;
        let life = self.life(number);
        life.failures += 1;
        life.phase = Phase::Idle;
        if life.failures >= attempts {
            self.hold(number, "attempts");
            return;
        }
        let retry = now.saturating_add(backoff);
        life.retry = Some(retry);
        self.alarm(number, retry);
    }

    /// The outcome a run of `why` finishes with, drawn; a change is pushed
    /// first.
    fn outcome(&mut self, number: u64, why: Why) -> Outcome {
        let script = self.settings.script;
        let escalates = self.rng.chance(script.escalations);
        let item = self.forge.item(number);
        match why {
            Why::Work => {
                if escalates {
                    return Outcome::Escalation;
                }
                let grows = match &item.record.step.work {
                    Work::Agent(spec) => spec.grows,
                    Work::Change(_) | Work::Wait(_) | Work::Session(_) => false,
                };
                if grows && let Some(steps) = self.growth(number) {
                    return Outcome::Steps(steps);
                }
                Outcome::Report
            }
            Why::Produce | Why::Repair(_) => {
                if escalates {
                    return Outcome::Escalation;
                }
                let Work::Change(spec) = &item.record.step.work else {
                    panic!("item {number} runs a change, and it is not one");
                };
                let on = self.forge.base(item.repository, &spec.base);
                let head = self.forge.commit();
                self.forge.item_mut(number).branch = Some(Pushed { head, on });
                self.log(format_args!("item {number}: pushed {head} on {on}"));
                if self.forge.pulls.get(&number).is_some_and(|pull| pull.state == State::Open) {
                    self.watch(number, false);
                    self.heard(number, Heard::Own);
                }
                let repair = match why {
                    Why::Repair(repair) => Some(repair),
                    Why::Produce | Why::Work | Why::Review { .. } | Why::Turn => None,
                };
                Outcome::Change { head: commit(head), repair }
            }
            Why::Review { head } => {
                let verdict = if self.rng.chance(script.verdicts) { Verdict::Changes } else { Verdict::Approve };
                Outcome::Verdict { head, verdict }
            }
            Why::Turn => {
                if item.record.goal.is_none() {
                    let invalid = !self.lives[&number].fixing && self.rng.chance(script.invalid);
                    let limits = self.settings.limits;
                    let plan = script::plan(&mut self.rng, &script, limits.steps, limits.dependencies, invalid);
                    return Outcome::Plan(plan);
                }
                if escalates {
                    return Outcome::Escalation;
                }
                if self.rng.chance(script.growth)
                    && let Some(steps) = self.growth(number)
                {
                    return Outcome::Steps(steps);
                }
                if self.tasks < self.settings.tasks && self.rng.chance(script.tasks) {
                    self.tasks += 1;
                    self.serial += 1;
                    return Outcome::Tasks(Box::new([script::task(&mut self.rng, &script, self.serial)]));
                }
                Outcome::Reply
            }
        }
    }

    /// Steps added to the plan of the goal item `number` is under, or
    /// supervises, if it has room.
    fn growth(&mut self, number: u64) -> Option<Box<[temper_engine_model_plan::Step]>> {
        let goal = self.forge.item(number).goal.unwrap_or(number);
        let plan = self.forge.item(goal).record.goal.as_ref()?;
        let mut done = Vec::new();
        for item in self.forge.items.values() {
            if item.goal == Some(goal) && item.closed.is_some() {
                done.push(item.record.step.name.clone());
            }
        }
        self.serial += 1;
        let script = self.settings.script;
        script::growth(&mut self.rng, &script, plan, &done, self.settings.limits.steps, self.serial)
    }

    /// What `outcome` writes for item `number`, from the forge read afresh.
    fn apply(&mut self, number: u64, outcome: &Outcome) -> (Applied, Vec<Write>) {
        let item = self.forge.item(number);
        let goal_item = item.goal.unwrap_or(number);
        let goal = self.forge.item(goal_item).record.goal.as_ref();
        let life = &self.lives[&number];
        let facts = translate::facts(&self.forge, number, life.snapshot, false);
        let mut out = Queue::with_capacity(plan::max_out(&self.settings.limits));
        let applied = plan::apply(&self.config, &self.env, &item.record, goal, &facts, outcome, &mut out);
        (applied, drain(&mut out))
    }

    /// Makes an outcome's writes, its record's last, and moves its item's
    /// inbox position past the events its run took. The referee may restart
    /// the engine after the forge's writes: it applies the outcome again from
    /// its start, finding what it made by its keys.
    #[expect(clippy::too_many_arguments, reason = "what an outcome's application needs, kept together")]
    fn commit(
        &mut self,
        number: u64,
        attempt: u64,
        outcome: &Outcome,
        kind: Kind,
        writes: Vec<Write>,
        then: Then,
        relayed: usize,
        accepted: bool,
    ) {
        let origin = Origin::Outcome { attempt, kind, accepted };
        let mut writes = writes;
        if self.referee.restarts() {
            let mut forge_writes = Vec::new();
            for write in &writes {
                match write {
                    Write::Progress(_) | Write::Goal(_) => {}
                    Write::Create { .. }
                    | Write::OpenPull { .. }
                    | Write::Merge { .. }
                    | Write::Close
                    | Write::DeleteBranch => forge_writes.push(write.clone()),
                }
            }
            self.write(number, origin, forge_writes);
            self.path("engine restarts as it applies");
            self.log(format_args!("item {number}: the engine restarts"));
            let (again, rewrites) = self.apply(number, outcome);
            match again {
                Applied::Writes { .. } => writes = rewrites,
                Applied::Stale(_) | Applied::Invalid(_) => {
                    panic!("item {number}'s outcome applies again as it did: {again:?}")
                }
            }
        }
        self.write(number, origin, writes);
        let life = self.life(number);
        life.inbox.drain(..relayed.min(life.inbox.len()));
        life.phase = Phase::Idle;
        match then {
            Then::Wait => {}
            Then::Hold(hold) => self.hold(number, hold_name(hold)),
        }
    }

    /// A person decides on the outcome waiting for their acceptance.
    fn proposal(&mut self, number: u64, attempt: u64) {
        let life = self.life(number);
        let phase = std::mem::replace(&mut life.phase, Phase::Idle);
        let Phase::Proposed { attempt: proposed, outcome, relayed } = phase else {
            panic!("item {number}'s proposal waits for a person");
        };
        assert_eq!(proposed, attempt, "item {number}'s proposal is the one decided");
        let accepts = life.rejected >= 2 || !self.rng.chance(self.settings.script.proposals);
        if !accepts {
            self.life(number).rejected += 1;
            self.heard(number, Heard::Message);
            self.path("proposal rejected");
            self.log(format_args!("item {number}: a person rejects its proposal"));
            return;
        }
        self.path("proposal accepted");
        self.log(format_args!("item {number}: a person accepts its proposal"));
        let (applied, writes) = self.apply(number, &outcome);
        match applied {
            Applied::Writes { then, .. } => {
                let kind = kind(&outcome);
                self.commit(number, attempt, &outcome, kind, writes, then, relayed, true);
            }
            Applied::Stale(stale) => self.path(format!("stale once accepted: {}", stale_name(stale))),
            // The plan grew meanwhile, past what this growth leaves room for.
            Applied::Invalid(problems) => {
                self.path("invalid once accepted");
                self.log(format_args!("item {number}: invalid once accepted: {problems:?}"));
            }
        }
    }

    /// Makes `writes` for item `number`, in order.
    fn write(&mut self, number: u64, origin: Origin, writes: Vec<Write>) {
        let mut added = Vec::new();
        for write in writes {
            match write {
                Write::Create { key, record } => {
                    let made = self.create(number, origin, key, *record);
                    added.push(made);
                }
                Write::OpenPull { base } => self.open_pull(number, &base),
                Write::Merge { head } => self.merge(number, count(head)),
                Write::Close => self.close(number),
                Write::DeleteBranch => self.path("branch deleted"),
                Write::Progress(progress) => self.forge.item_mut(number).record.progress = progress,
                Write::Goal(goal) => {
                    let Origin::Outcome { kind, accepted, .. } = origin else {
                        panic!("item {number}'s goal is written by an outcome");
                    };
                    let target = match kind {
                        Kind::Plan => number,
                        Kind::Steps | Kind::Tasks | Kind::Other => self.forge.item(number).goal.unwrap_or(number),
                    };
                    self.forge.item_mut(target).record.goal = Some(goal);
                    if kind == Kind::Steps {
                        self.path(if accepted { "growth: accepted beyond" } else { "growth: within" });
                        self.observe(&Seen::Grown { goal: target, added: added.clone(), accepted });
                    }
                }
            }
        }
    }

    /// Makes the item of a creation, or finds the one made before by its key.
    fn create(&mut self, number: u64, origin: Origin, key: Key, record: Record) -> u64 {
        let Origin::Outcome { attempt, kind, .. } = origin else {
            panic!("item {number}'s creations come from an outcome");
        };
        let (goal, parent) = match kind {
            Kind::Plan => (Some(number), Some(number)),
            Kind::Steps => (Some(self.forge.item(number).goal.unwrap_or(number)), Some(number)),
            Kind::Tasks | Kind::Other => (None, None),
        };
        let key = match key {
            Key::Step(name) => Made::Step(name.into_vec()),
            Key::Task(index) => Made::Task(index),
        };
        let item = Item {
            repository: record.step.repository.0,
            record,
            goal,
            parent,
            created: self.now,
            closed: None,
            held: None,
            branch: None,
            decision: None,
        };
        let (made, new) = self.forge.create(Some(Keyed { item: number, attempt, key }), item);
        if !new {
            self.path("found by its key");
            return made;
        }
        self.lives.insert(made, Life::new(self.now));
        self.stats.items += 1;
        self.observe(&Seen::Made(made));
        if goal.is_none() {
            self.observe(&Seen::Began(made));
        }
        let name = String::from_utf8_lossy(&self.forge.item(made).record.step.name).into_owned();
        self.log(format_args!("item {number}: makes item {made}, {name}"));
        made
    }

    fn open_pull(&mut self, number: u64, base: &[u8]) {
        let repository = self.forge.item(number).repository;
        let pull = Pull {
            repository,
            base: base.to_vec(),
            state: State::Open,
            ci: BTreeMap::new(),
            approvals: BTreeMap::new(),
            changes: std::collections::BTreeSet::new(),
            conflicts: BTreeMap::new(),
        };
        assert!(self.forge.pulls.insert(number, pull).is_none(), "item {number}'s pull request is opened once");
        self.path("pull request opened");
        self.observe(&Seen::PullOpened(number));
        self.watch(number, true);
    }

    /// CI and the forge work on the pull request's new head; people may close
    /// it, or push to its base, once it opens.
    fn watch(&mut self, number: u64, opened: bool) {
        let script = self.settings.script;
        let head = self.forge.item(number).branch.expect("a pull request has a head").head;
        let pull = &self.forge.pulls[&number];
        let (repository, base_name) = (pull.repository, pull.base.clone());
        let base = self.forge.base(repository, &base_name);
        let at = self.later(script.ci);
        self.wire.send(at, Delivery::Ci { item: number, head });
        let at = self.later(script.mergeable);
        self.wire.send(at, Delivery::Mergeable { item: number, head, base });
        if !opened {
            return;
        }
        if self.rng.chance(script.closes) {
            let at = self.later(script.people);
            self.wire.send(at, Delivery::ClosePull { item: number });
        }
        if self.rng.chance(script.pushes) {
            let at = self.later(script.people);
            self.wire.send(at, Delivery::PushBase { repository, base: base_name });
        }
    }

    fn ci(&mut self, number: u64, head: u64) {
        let passed = !self.rng.chance(self.settings.script.ci_fails);
        let pull = self.forge.pulls.get_mut(&number).expect("a pull request opened");
        pull.ci.insert(head, passed);
        self.path(if passed { "ci: passed" } else { "ci: failed" });
        let needed = approvals_needed(&self.forge.item(number).record.step);
        if passed && needed > 0 {
            let at = self.later(self.settings.script.people);
            self.wire.send(at, Delivery::Review { item: number, head });
        }
    }

    fn review(&mut self, number: u64, head: u64) {
        let asks = self.rng.chance(self.settings.script.changes_asked);
        let needed = approvals_needed(&self.forge.item(number).record.step);
        let pull = self.forge.pulls.get_mut(&number).expect("a pull request opened");
        if pull.state != State::Open {
            return;
        }
        if asks {
            pull.changes.insert(head);
            self.path("review: changes asked for");
        } else {
            pull.approvals.insert(head, needed);
            self.path("review: approved");
        }
    }

    fn merge(&mut self, number: u64, head: u64) {
        let pushed = self.forge.item(number).branch.expect("a change has a head");
        let pull = &self.forge.pulls[&number];
        let base = self.forge.base(pull.repository, &pull.base);
        let clean = pull.conflicts.get(&(head, base)) == Some(&false);
        if pull.state != State::Open || pushed.head != head || !clean {
            self.path("merge refused");
            return;
        }
        let (repository, base_name) = (pull.repository, pull.base.clone());
        self.forge.pulls.get_mut(&number).expect("a pull request opened").state = State::Merged;
        let moved = self.forge.move_base(repository, &base_name);
        self.path("merged");
        self.log(format_args!("item {number}: merged at {head}"));
        self.observe(&Seen::Merged { item: number, head });
        self.base_moved(repository, &base_name, moved);
    }

    /// The forge works out again whether the pull requests into a base that
    /// moved merge cleanly.
    fn base_moved(&mut self, repository: u32, base: &[u8], head: u64) {
        for item in self.forge.open_into(repository, base) {
            let pushed = self.forge.item(item).branch.expect("a pull request has a head");
            let at = self.later(self.settings.script.mergeable);
            self.wire.send(at, Delivery::Mergeable { item, head: pushed.head, base: head });
        }
    }

    /// Closes item `number`: its step is done. Its parent and the steps after
    /// it hear of it.
    fn close(&mut self, number: u64) {
        self.forge.item_mut(number).closed = Some(self.now);
        self.life(number).phase = Phase::Closed;
        self.observe(&Seen::Ended(number));
        self.relations_hear(number);
    }

    /// Holds item `number` for a person. Its parent hears of it.
    fn hold(&mut self, number: u64, why: &'static str) {
        self.forge.item_mut(number).held = Some(why);
        self.life(number).phase = Phase::Held;
        self.held.push((number, why));
        self.path(format!("held: {why}"));
        self.log(format_args!("item {number}: held, {why}"));
        self.observe(&Seen::Ended(number));
        if let Some(parent) = self.forge.item(number).parent {
            self.heard(parent, Heard::Child);
        }
    }

    fn relations_hear(&mut self, number: u64) {
        let item = self.forge.item(number);
        let (parent, goal, name) = (item.parent, item.goal, item.record.step.name.clone());
        if let Some(parent) = parent {
            self.heard(parent, Heard::Child);
        }
        let Some(goal) = goal else {
            return;
        };
        let mut after = Vec::new();
        for (other, item) in &self.forge.items {
            if item.goal == Some(goal) && item.record.step.after.contains(&name) {
                after.push(*other);
            }
        }
        for other in after {
            self.heard(other, Heard::Dependency);
        }
    }

    /// Something comes into item `number`'s inbox, if it is live.
    fn heard(&mut self, number: u64, heard: Heard) {
        let now = self.now;
        if let Some(life) = self.lives.get_mut(&number) {
            life.inbox.push((heard, now));
        }
    }

    /// Checks the invariants of a world with nothing left to happen, and the
    /// referee's verdict, and tells how each goal and task ended.
    fn assert_settled(&mut self) {
        assert!(self.wire.is_empty(), "nothing is in flight");
        self.runs.assert_settled();
        for (number, life) in &self.lives {
            match life.phase {
                Phase::Idle | Phase::Held | Phase::Closed => {}
                Phase::Running { .. } | Phase::Proposed { .. } => panic!("item {number} is still busy"),
            }
        }
        self.referee.verdict(&self.forge);
        let mut endings = BTreeMap::new();
        for (number, item) in &self.forge.items {
            if item.goal.is_some() {
                continue;
            }
            let what = match item.record.step.work {
                Work::Session(_) => "goal",
                Work::Agent(_) | Work::Change(_) | Work::Wait(_) => "task",
            };
            let ending = if item.closed.is_some() {
                format!("{what}: done")
            } else {
                let mut first = None;
                for (held, why) in &self.held {
                    let under = self.forge.item(*held).goal;
                    if first.is_none() && (*held == *number || under == Some(*number)) {
                        first = Some(*why);
                    }
                }
                let why = first.unwrap_or_else(|| panic!("goal {number} ended, so it is done or held"));
                format!("{what}: held, {why}")
            };
            *endings.entry(ending).or_default() += 1;
        }
        self.stats.endings = endings;
    }
}

fn drain(out: &mut Queue<Write>) -> Vec<Write> {
    let mut writes = Vec::new();
    while let Some(write) = out.pop() {
        writes.push(write);
    }
    writes
}

fn kind(outcome: &Outcome) -> Kind {
    match outcome {
        Outcome::Plan(_) => Kind::Plan,
        Outcome::Steps(_) => Kind::Steps,
        Outcome::Tasks(_) => Kind::Tasks,
        Outcome::Change { .. } | Outcome::Verdict { .. } | Outcome::Report | Outcome::Reply | Outcome::Escalation => {
            Kind::Other
        }
    }
}

fn hold_name(hold: Hold) -> &'static str {
    match hold {
        Hold::Rejected => "rejected",
        Hold::Repairs => "repairs",
        Hold::PullClosed => "pull request closed",
        Hold::Escalated => "escalated",
    }
}

fn why_name(why: Why) -> &'static str {
    match why {
        Why::Work => "work",
        Why::Produce => "produce",
        Why::Repair(Repair::CiFailed) => "repair, CI failed",
        Why::Repair(Repair::ChangesRequested) => "repair, changes asked for",
        Why::Repair(Repair::BaseMoved) => "repair, base moved",
        Why::Repair(Repair::Conflicts) => "repair, conflicts",
        Why::Review { .. } => "review",
        Why::Turn => "turn",
    }
}

fn outcome_name(outcome: &Outcome) -> &'static str {
    match outcome {
        Outcome::Change { .. } => "change",
        Outcome::Verdict { verdict: Verdict::Approve, .. } => "verdict, approve",
        Outcome::Verdict { verdict: Verdict::Changes, .. } => "verdict, changes",
        Outcome::Report => "report",
        Outcome::Plan(_) => "plan",
        Outcome::Steps(_) => "steps",
        Outcome::Tasks(_) => "tasks",
        Outcome::Reply => "reply",
        Outcome::Escalation => "escalation",
    }
}

fn stale_name(stale: Stale) -> &'static str {
    match stale {
        Stale::Moved => "moved",
        Stale::Landed => "landed",
        Stale::Closed => "closed",
        Stale::Finished => "finished",
    }
}
