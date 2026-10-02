use std::collections::{BTreeMap, BTreeSet};

use temper_lib::{Duration, Rng, Time, Token};
use temper_worker_model_host::{self as host, Event, Fact, Limits, Reason, Reply, Request};
use temper_world::{Ledger, Schedule, Span, Stage, Trace};

use crate::engine::{self, Act, Engine, Plan};
use crate::parent::{self, Out, Parent};

/// Room in the host's output queue beyond what one step may emit. Small, so
/// the loop's flow control (take an event only while there is room for what
/// it may produce) is exercised.
const SPARE: u32 = 2;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Settings {
    /// Seeds the world, which seeds the engine and the parent.
    pub seed: u64,
    pub host: Limits,
    pub engine: engine::Script,
    pub parent: parent::Script,
    /// One-way latency between the engine and the worker.
    pub network: Span,
    /// One-way latency between the host and its siblings, through the top
    /// level.
    pub hop: Span,
    /// A spell without contact with the engine, if there is one.
    pub outage: Option<Outage>,
    /// When the worker shuts down, if it does.
    pub shutdown: Option<Span>,
}

/// A spell without contact with the engine: when it starts, how long it
/// lasts, and the grace past which the top level cancels every run. Messages
/// either way wait for contact to come back; on coming back, the top level
/// reports what the host hosts, and the engine keeps or cancels each.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Outage {
    pub at: Span,
    pub length: Span,
    pub grace: Duration,
}

impl Settings {
    /// A world where nothing goes wrong: room for every assignment, all
    /// within the limits, workspaces that prepare, agents that start, work a
    /// little and end (or park, idle past their time after a yield), and no
    /// cancels.
    #[must_use]
    pub const fn calm(seed: u64) -> Settings {
        Settings {
            seed,
            host: Limits {
                slots: 4,
                repositories: 3,
                name_bytes: 64,
                charter_bytes: 4096,
                snapshot_bytes: 1024,
                outcome_bytes: 512,
                detail_bytes: 64,
                held: 2,
                event_bytes: 64,
                run_calls: 2,
                facts: 64,
            },
            engine: engine::Script {
                assignments: 8,
                spacing: Span::millis(1_000, 20_000),
                invalid: 0,
                repeats: 0,
                repositories: 3,
                writable: 500,
                saves: 500,
                snapshots: 200,
                events: 3,
                event_gap: Span::millis(100, 10_000),
                large: 0,
                cancels: 0,
                cancel_after: Span::millis(0, 30_000),
                stale: 0,
                relay: Span::millis(10, 2_000),
                forget: 0,
            },
            parent: parent::Script {
                prepare: Span::millis(100, 5_000),
                transient: 0,
                permanent: 0,
                start: Span::millis(50, 500),
                unstarted: 0,
                steps: 6,
                step: Span::millis(100, 5_000),
                relays: 300,
                pushes: 200,
                yields: 200,
                idle: Span::millis(5_000, 60_000),
                fates: parent::Fates {
                    ended: 1,
                    parked: 0,
                    failed: 0,
                    exited: 0,
                    hung: 0,
                    overrun: 0,
                    rules: 0,
                    oversized: 0,
                },
                exit: Span::millis(10, 500),
                kill: Span::millis(100, 2_000),
                wind: Span::millis(100, 3_000),
                watchdog: Span::millis(10_000, 60_000),
                late: 0,
                push: Span::millis(100, 3_000),
                changes: 600,
                moved: 0,
                failed: 0,
            },
            network: Span::millis(1, 50),
            hop: Span::millis(0, 2),
            outage: None,
            shutdown: None,
        }
    }

    /// A world where everything that can go wrong does, now and then: a
    /// world for the random sweep.
    #[must_use]
    pub const fn rough(seed: u64) -> Settings {
        let calm = Settings::calm(seed);
        Settings {
            host: Limits { slots: 2, held: 1, run_calls: 1, facts: 8, ..calm.host },
            engine: engine::Script {
                assignments: 12,
                spacing: Span::millis(0, 10_000),
                invalid: 100,
                repeats: 100,
                events: 4,
                large: 50,
                cancels: 250,
                stale: 300,
                forget: 300,
                ..calm.engine
            },
            parent: parent::Script {
                transient: 60,
                permanent: 60,
                unstarted: 60,
                relays: 350,
                pushes: 250,
                fates: parent::Fates {
                    ended: 4,
                    parked: 3,
                    failed: 6,
                    exited: 1,
                    hung: 1,
                    overrun: 1,
                    rules: 1,
                    oversized: 1,
                },
                late: 300,
                moved: 150,
                failed: 150,
                ..calm.parent
            },
            outage: Some(Outage {
                at: Span::millis(0, 120_000),
                length: Span::millis(1_000, 90_000),
                grace: Duration::from_secs(30),
            }),
            shutdown: None,
            ..calm
        }
    }
}

/// What the world counted.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct Stats {
    pub engine: engine::Tally,
    pub parent: parent::Tally,
    /// Answers, by kind.
    pub endings: BTreeMap<&'static str, u32>,
    /// Facts the host told, by kind, and how many it dropped.
    pub facts: BTreeMap<&'static str, u32>,
    pub facts_lost: u64,
    /// Stale messages the host took, each changing nothing; reports it made;
    /// replies it gave as unavailable; cancels of every run.
    pub stale: u32,
    pub reports: u32,
    pub unavailable: u32,
    pub busy: u32,
    pub cancel_alls: u32,
    /// The most runs hosted at once.
    pub peak: u32,
}

/// Something on its way, delivered at its time.
#[derive(Debug)]
enum Delivery {
    /// An event reaching the host: `stale` when it names an attempt never
    /// assigned, so it must change nothing.
    Host { event: Event, stale: bool },
    /// A request of the host's reaching the engine.
    Engine(Request),
    /// The engine's own plan.
    Plan(Plan),
    /// An agent's wake.
    Wake { agent: Token, wake: u64 },
    /// Contact with the engine comes back.
    Back,
    /// The worker shuts down.
    Shutdown,
}

/// An event on its way into the host, with what the world knows of it.
#[derive(Debug)]
struct Arrival {
    event: Event,
    stale: bool,
}

/// The three one-way channels whose order matters, each delivering in the
/// order it was given.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Lane {
    EngineToHost,
    HostToEngine,
    ParentToHost,
}

/// What one step of the host took, for the world to note as it routes what
/// the step made: in the order the steps were taken, so that each request is
/// judged against what came before it.
#[derive(Clone, Copy, Debug)]
enum Taken {
    Assign { run: Token, attempt: Token },
    Stale,
    Started { owner: Token, agent: Token },
    Called { owner: Token, call: Token },
    Gone { owner: Token },
    Pushed { call: Token },
    Saved { owner: Token },
    Other,
}

/// A hosted run as the world sees it.
#[derive(Debug)]
struct Hosted {
    agent: Option<Token>,
    /// The place of the last inbound event delivered to it.
    delivered: Option<u64>,
}

pub struct World {
    now: Time,
    /// Draws the latencies.
    rng: Rng,
    settings: Settings,

    host: host::Model,
    stage: Stage<Limits, Arrival, Request>,

    engine: Engine,
    parent: Parent,

    wire: Schedule<Delivery>,
    lanes: [Time; 3],
    /// When contact is lost and comes back, if it is.
    outage: Option<(Time, Time)>,

    /// Hosted runs, by the host's tokens for them; those admitted and not yet
    /// answered, by the engine's names; agents' runs; agents whose runs left
    /// live; and host calls in flight, by agent and call.
    hosted: BTreeMap<Token, Hosted>,
    admitted: BTreeMap<(Token, Token), Token>,
    agents: BTreeMap<Token, Token>,
    left: BTreeSet<Token>,
    calls: Ledger<(Token, Token), Token>,

    stats: Stats,
    trace: Trace,
}

impl World {
    #[must_use]
    pub fn new(settings: Settings) -> World {
        assert!(host::worst_case(&settings.host).is_some(), "the shell refuses limits it cannot provision");
        let mut rng = Rng::new(settings.seed);
        let engine = Engine::new(settings.engine, settings.host, rng.next_u64());
        let parent = Parent::new(settings.parent, settings.host, rng.next_u64());
        let max_out = host::max_out(&settings.host);
        let latencies = Rng::new(rng.next_u64());
        let mut world = World {
            now: Time::ZERO,
            rng: latencies,
            settings,
            host: host::Model::new(&settings.host),
            stage: Stage::new(settings.host, max_out, max_out + SPARE),
            engine,
            parent,
            wire: Schedule::new(),
            lanes: [Time::ZERO; 3],
            outage: None,
            hosted: BTreeMap::new(),
            admitted: BTreeMap::new(),
            agents: BTreeMap::new(),
            left: BTreeSet::new(),
            calls: Ledger::new("host call"),
            stats: Stats::default(),
            trace: Trace::default(),
        };
        if let Some(outage) = settings.outage {
            let lost = Time::ZERO.saturating_add(outage.at.draw(&mut rng));
            let length = outage.length.draw(&mut rng);
            let back = lost.saturating_add(length);
            world.outage = Some((lost, back));
            if length > outage.grace {
                let event = Event::CancelAll { reason: Reason::Contact };
                world.wire.send(lost.saturating_add(outage.grace), Delivery::Host { event, stale: false });
            }
            world.wire.send(back, Delivery::Back);
        }
        if let Some(at) = settings.shutdown {
            let at = Time::ZERO.saturating_add(at.draw(&mut rng));
            world.wire.send(at, Delivery::Shutdown);
        }
        let begin = world.engine.begin();
        world.acts(begin);
        world
    }

    #[must_use]
    pub fn now(&self) -> Time {
        self.now
    }

    #[must_use]
    pub fn stats(&self) -> Stats {
        Stats {
            engine: self.engine.tally(),
            parent: self.parent.tally(),
            endings: self.engine.endings().clone(),
            facts_lost: self.host.facts_lost(),
            ..self.stats.clone()
        }
    }

    /// What crossed between the host and the world, in order, with times.
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
            let Some(next) = self.wire.next_time() else {
                self.assert_settled();
                return;
            };
            assert!(next > self.now, "time moves forward");
            self.now = next;
        }
        panic!("the world did not settle in {iterations} iterations");
    }

    /// One iteration of the loop, as the shell would run it.
    fn iterate(&mut self) {
        self.stage.tick(self.now);
        self.deliver();
        let mut made = Vec::new();
        // The ready list first, at the start of the stage, then the events.
        while self.stage.has_room() && self.host.is_ready() {
            self.trace.log(self.now, "host resumes");
            let before = self.stage.out.len();
            host::resume(&mut self.host, &self.stage.env, &mut self.stage.out);
            made.push((Taken::Other, self.stage.out.len() - before));
        }
        while let Some(Arrival { event, stale }) = self.stage.next_event() {
            self.trace.log(self.now, format!("host <- {event:?}"));
            let taken = self.take(&event, stale);
            let before = self.stage.out.len();
            host::step(&mut self.host, &self.stage.env, event, &mut self.stage.out);
            made.push((taken, self.stage.out.len() - before));
        }
        for (taken, count) in made {
            match taken {
                Taken::Stale => {
                    assert_eq!(count, 0, "a stale attempt never acts");
                    self.stats.stale += 1;
                }
                Taken::Assign { run, attempt } => {
                    assert_eq!(count, 1, "an assignment is refused or prepared");
                    let request = self.stage.out.pop().expect("counted");
                    self.assigned(run, attempt, &request);
                    self.route(request);
                }
                Taken::Started { .. }
                | Taken::Called { .. }
                | Taken::Gone { .. }
                | Taken::Pushed { .. }
                | Taken::Saved { .. }
                | Taken::Other => {
                    self.note(taken);
                    for _ in 0..count {
                        let request = self.stage.out.pop().expect("counted");
                        self.route(request);
                    }
                }
            }
        }
        assert!(self.stage.out.is_empty(), "every request was routed");
        while let Some(fact) = self.host.pop_fact() {
            *self.stats.facts.entry(fact_kind(fact)).or_default() += 1;
        }
        self.host.reclaim();
        let hosted = u32::try_from(self.admitted.len()).expect("fits");
        assert_eq!(self.host.hosted(), hosted, "a slot is taken from admission until its run has answered");
        self.stats.peak = self.stats.peak.max(hosted);
    }

    /// What the host takes as `event`, for the world to note.
    fn take(&mut self, event: &Event, stale: bool) -> Taken {
        if stale {
            return Taken::Stale;
        }
        match event {
            Event::Assign { reply_to: _, assignment } => {
                Taken::Assign { run: assignment.run, attempt: assignment.attempt }
            }
            Event::Started { owner, agent } => Taken::Started { owner: *owner, agent: *agent },
            Event::Called { owner, call, ask: _ } => Taken::Called { owner: *owner, call: *call },
            Event::Gone { owner, detail: _ } => Taken::Gone { owner: *owner },
            Event::Pushed { owner, push: _ } => Taken::Pushed { call: *owner },
            Event::Saved { owner, save: _ } => Taken::Saved { owner: *owner },
            Event::CancelAll { reason: _ } => {
                self.stats.cancel_alls += 1;
                Taken::Other
            }
            Event::Report => {
                self.stats.reports += 1;
                Taken::Other
            }
            Event::Inbound { .. }
            | Event::Cancel { .. }
            | Event::Relayed { .. }
            | Event::Prepared { .. }
            | Event::Unprepared { .. }
            | Event::Yielded { .. }
            | Event::Finished { .. }
            | Event::Faulted { .. } => Taken::Other,
        }
    }

    /// Notes what a step took, before routing what it made.
    fn note(&mut self, taken: Taken) {
        match taken {
            Taken::Started { owner, agent } => {
                self.hosted.get_mut(&owner).expect("a start is of a hosted run").agent = Some(agent);
                self.agents.insert(agent, owner);
            }
            Taken::Called { owner, call } => {
                let agent = self.hosted.get(&owner).expect("a call is of a hosted run").agent;
                let agent = agent.expect("a call is made by a started agent");
                self.calls.open((agent, call), agent);
            }
            // Its run leaves live, if it was: what it had in flight is
            // answered now.
            Taken::Gone { owner } => {
                if let Some(agent) = self.hosted.get(&owner).expect("a gone agent is of a hosted run").agent {
                    self.left.insert(agent);
                }
            }
            Taken::Pushed { call } => self.parent.pushed(call),
            Taken::Saved { owner } => self.parent.saved(owner),
            Taken::Assign { .. } | Taken::Stale | Taken::Other => {}
        }
    }

    /// The host's one request in the step that took the assignment for
    /// `run`'s attempt `attempt`: a refusal, or a prepare that admits it.
    fn assigned(&mut self, run: Token, attempt: Token, request: &Request) {
        match request {
            Request::Answer { to: _, run: answered, attempt: of, answer } => {
                assert_eq!((*answered, *of), (run, attempt), "an assignment is answered as itself");
                match answer {
                    host::Answer::Refused(_) => {}
                    host::Answer::Ended { .. } | host::Answer::Parked { .. } | host::Answer::Failed { .. } => {
                        panic!("an assignment answered at once is refused: {answer:?}")
                    }
                }
            }
            Request::Prepare { owner, workspace: _ } => {
                let entry = Hosted { agent: None, delivered: None };
                assert!(self.hosted.insert(*owner, entry).is_none(), "a hosted run's token is its own");
                assert!(self.admitted.insert((run, attempt), *owner).is_none(), "an attempt is admitted once");
            }
            Request::Relay { .. }
            | Request::Bounced { .. }
            | Request::Hosting { .. }
            | Request::Start { .. }
            | Request::Deliver { .. }
            | Request::Reply { .. }
            | Request::Stop { .. }
            | Request::Push { .. }
            | Request::Save { .. }
            | Request::Release { .. } => panic!("an assignment is refused or prepared: {request:?}"),
        }
    }

    /// Hands the host's `request` to whom it is for, checking it on the way.
    fn route(&mut self, request: Request) {
        self.trace.log(self.now, format!("host -> {request:?}"));
        match request {
            Request::Answer { to, run, attempt, answer } => {
                if let Some(owner) = self.admitted.remove(&(run, attempt)) {
                    assert!(self.parent.settled(owner), "a run answers once all of it has settled and is released");
                    let hosted = self.hosted.get(&owner).expect("admitted runs are hosted");
                    if let Some(agent) = hosted.agent {
                        assert!(self.left.contains(&agent), "a run answers once it has left live");
                    }
                }
                self.send_engine(Request::Answer { to, run, attempt, answer });
            }
            Request::Relay { .. } | Request::Bounced { .. } => self.send_engine(request),
            Request::Hosting { runs } => {
                let mut hosting = BTreeSet::new();
                for entry in &runs {
                    hosting.insert((entry.run, entry.attempt));
                }
                let admitted: BTreeSet<(Token, Token)> = self.admitted.keys().copied().collect();
                assert_eq!(hosting, admitted, "the report lists every run admitted and not answered");
                let acts = self.engine.reconnected(&runs);
                self.acts(acts);
            }
            Request::Deliver { agent, event } => {
                let owner = *self.agents.get(&agent).expect("a delivery is to a started agent");
                let len = u64::try_from(event.len()).expect("fits");
                assert!(len <= self.settings.host.event_bytes, "an event within the limits");
                let hosted = self.hosted.get_mut(&owner).expect("an agent's run is hosted");
                let head = event.get(..8).expect("an event begins with its place");
                let place = u64::from_be_bytes(head.try_into().expect("eight bytes"));
                assert!(hosted.delivered < Some(place), "inbound events are delivered once, in the order sent");
                hosted.delivered = Some(place);
                self.parcel(Request::Deliver { agent, event });
            }
            Request::Reply { agent, call, reply } => {
                self.calls.end((agent, call));
                if self.left.contains(&agent) {
                    assert_eq!(reply, Reply::Unavailable, "a call of a run that has left live is unavailable");
                }
                match reply {
                    Reply::Unavailable => self.stats.unavailable += 1,
                    Reply::Busy => self.stats.busy += 1,
                    Reply::Relayed { .. } | Reply::Pushed(_) => {}
                }
                self.parcel(Request::Reply { agent, call, reply });
            }
            Request::Stop { agent } => {
                for of in self.calls.values() {
                    assert!(*of != agent, "a run's calls in flight are answered as it leaves live");
                }
                self.left.insert(agent);
                self.parcel(Request::Stop { agent });
            }
            Request::Prepare { .. }
            | Request::Start { .. }
            | Request::Push { .. }
            | Request::Save { .. }
            | Request::Release { .. } => self.parcel(request),
        }
    }

    fn send_engine(&mut self, request: Request) {
        let at = self.lane(Lane::HostToEngine, self.settings.network);
        self.wire.send(at, Delivery::Engine(request));
    }

    /// Hands `request` to the parent, and sends on what it does.
    fn parcel(&mut self, request: Request) {
        let outs = self.parent.take(request);
        self.outs(outs);
    }

    fn outs(&mut self, outs: Vec<Out>) {
        for out in outs {
            match out {
                Out::Host { after, event } => {
                    let at = self.lane_after(Lane::ParentToHost, self.settings.hop, after);
                    self.wire.send(at, Delivery::Host { event, stale: false });
                }
                Out::Wake { after, agent, wake } => {
                    self.wire.send(self.now.saturating_add(after), Delivery::Wake { agent, wake });
                }
            }
        }
    }

    fn acts(&mut self, acts: Vec<Act>) {
        for act in acts {
            match act {
                Act::Host { after, event, stale } => {
                    let at = self.lane_after(Lane::EngineToHost, self.settings.network, after);
                    self.wire.send(at, Delivery::Host { event, stale });
                }
                Act::Later { after, plan } => {
                    self.wire.send(self.now.saturating_add(after), Delivery::Plan(plan));
                }
            }
        }
    }

    /// Hands over what is due now.
    fn deliver(&mut self) {
        while let Some(delivery) = self.wire.next(self.now) {
            match delivery {
                Delivery::Host { event, stale } => self.stage.push(Arrival { event, stale }),
                Delivery::Engine(request) => {
                    let acts = self.engine.take(request);
                    self.acts(acts);
                }
                Delivery::Plan(plan) => {
                    let acts = self.engine.plan(plan);
                    self.acts(acts);
                }
                Delivery::Wake { agent, wake } => {
                    let outs = self.parent.wake(agent, wake);
                    self.outs(outs);
                }
                Delivery::Back => self.stage.push(Arrival { event: Event::Report, stale: false }),
                Delivery::Shutdown => {
                    self.engine.shut();
                    self.stage.push(Arrival { event: Event::CancelAll { reason: Reason::Shutdown }, stale: false });
                }
            }
        }
    }

    /// When something sent now on `lane` arrives, `span` after now and after
    /// what was sent on it before.
    fn lane(&mut self, lane: Lane, span: Span) -> Time {
        self.lane_after(lane, span, Duration::ZERO)
    }

    fn lane_after(&mut self, lane: Lane, span: Span, after: Duration) -> Time {
        let mut at = self.now.saturating_add(after).saturating_add(span.draw(&mut self.rng));
        let (index, engine) = match lane {
            Lane::EngineToHost => (0, true),
            Lane::HostToEngine => (1, true),
            Lane::ParentToHost => (2, false),
        };
        // Messages to and from the engine wait for contact to come back.
        if let Some((lost, back)) = self.outage
            && engine
            && at >= lost
            && at < back
        {
            at = back;
        }
        at = at.max(self.lanes[index]);
        self.lanes[index] = at;
        at
    }

    fn has_work_now(&self) -> bool {
        self.stage.has_events() || self.host.is_ready() || self.wire.is_due(self.now)
    }

    /// Checks the invariants of a world with nothing left to happen.
    fn assert_settled(&self) {
        assert!(self.wire.is_empty(), "nothing is in flight");
        assert!(!self.stage.has_events(), "the host has taken everything");
        assert!(!self.host.is_ready(), "no run waits on the ready list");
        assert_eq!(self.host.hosted(), 0, "every slot is free");
        assert_eq!(self.host.calls(), 0, "no host call is in flight");
        assert!(self.admitted.is_empty(), "every admitted run has answered");
        self.calls.assert_settled();
        self.engine.assert_settled();
        self.parent.assert_settled();
        let facts = &self.stats.facts;
        if self.host.facts_lost() == 0 {
            let admitted = facts.get("admitted").copied().unwrap_or(0);
            let answered: u32 =
                ["ended", "parked", "failed"].iter().map(|kind| facts.get(kind).copied().unwrap_or(0)).sum();
            assert_eq!(admitted, answered, "every admitted run tells how it answered");
        }
    }
}

fn fact_kind(fact: Fact) -> &'static str {
    match fact {
        Fact::Admitted { .. } => "admitted",
        Fact::Prepared { .. } => "prepared",
        Fact::Started { .. } => "started",
        Fact::Parked { .. } => "parked",
        Fact::Ended { .. } => "ended",
        Fact::Failed { .. } => "failed",
    }
}
