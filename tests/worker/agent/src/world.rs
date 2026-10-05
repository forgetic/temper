use std::collections::{BTreeMap, BTreeSet};

use skein_lib::{Duration, Rng, Time, Token};
use temper_worker_domain_agent::channel::{Ask, Down, Finish, Reply, Up};
use temper_worker_domain_agent::{self as agent, Bounce, End, Event, Fact, Fault, Limits, Request, Signal};
use temper_world::{Schedule, Span, Stage, Trace};

use crate::client::{self, Client};
use crate::script::{self, Fates, Sizes};
use crate::tree::{self, Tree};

/// Room in the domain's output queue beyond what one step may emit. Small, so
/// the loop's flow control (take an event only while there is room for what
/// it may produce) is exercised.
const SPARE: u32 = 2;

/// How long past the grace, `kill_after` and a pipe's latency an agent may
/// take to go once it is stopping: the kill, and io's terminals a hop each.
const SLACK: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Settings {
    /// Seeds the world, which seeds the client and the process trees.
    pub seed: u64,
    pub agent: Limits,
    pub client: client::Script,
    pub tree: tree::Script,
    pub script: script::Script,
    /// One-way latency between the domain and its neighbours: the client,
    /// through the top level, and io, through the protocol layer.
    pub hop: Span,
}

impl Settings {
    /// A world where nothing goes wrong: agents that spawn, work a while
    /// (facts, calls, long operations, waits for inbound events) and end,
    /// park or fail as they say, then exit; a client that answers in good
    /// time and stops each agent once told how its run finishes.
    #[must_use]
    pub const fn calm(seed: u64) -> Settings {
        Settings {
            seed,
            agent: Limits {
                accounts: 4,
                repositories: 8,
                name_bytes: 256,
                agents: 4,
                charter_bytes: 256,
                snapshot_bytes: 128,
                transcript_bytes: 0,
                turn_bytes: 0,
                conflicts: 0,
                path_bytes: 0,
                event_bytes: 32,
                events: 2,
                calls: 2,
                call_bytes: 64,
                answer_bytes: 64,
                fact_bytes: 32,
                outcome_bytes: 64,
                detail_bytes: 32,
                spawn_timeout: Duration::from_secs(1),
                no_progress: Duration::from_secs(10),
                long_span: Duration::from_secs(120),
                wall_time: Duration::from_secs(900),
                grace: Duration::from_secs(5),
                kill_after: Duration::from_secs(2),
                facts: 64,
            },
            client: client::Script {
                spawns: 8,
                spacing: Span::millis(0, 20_000),
                invalid: 0,
                snapshots: 300,
                events: 3,
                event_gap: Span::millis(100, 10_000),
                large: 0,
                burst: 0,
                answer: Span::millis(50, 2_000),
                slow: 0,
                slow_answer: Span::millis(15_000, 30_000),
                oversized: 0,
                stops: 0,
                stop_after: Span::millis(0, 30_000),
                stop_on_end: 1000,
            },
            tree: tree::Script {
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
            },
            script: script::Script {
                steps: 8,
                step: Span::millis(100, 3_000),
                calls: 250,
                longs: 100,
                waits: 150,
                pushes: 300,
                blocking: 300,
                call_deadline: Span::millis(5_000, 20_000),
                long: Span::millis(5_000, 60_000),
                idle: Span::millis(5_000, 60_000),
                fates: Fates {
                    ended: 3,
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
            },
            hop: Span::millis(0, 20),
        }
    }

    /// A world where everything that can go wrong does, now and then: a
    /// world for the random sweep.
    #[must_use]
    pub const fn rough(seed: u64) -> Settings {
        let calm = Settings::calm(seed);
        Settings {
            agent: Limits {
                accounts: 4,
                repositories: 8,
                name_bytes: 256,
                agents: 2,
                events: 1,
                calls: 1,
                wall_time: Duration::from_secs(120),
                facts: 8,
                ..calm.agent
            },
            client: client::Script {
                spawns: 12,
                spacing: Span::millis(0, 10_000),
                invalid: 100,
                events: 6,
                large: 100,
                burst: 300,
                slow: 200,
                oversized: 100,
                stops: 250,
                stop_on_end: 800,
                ..calm.client
            },
            tree: tree::Script {
                // Some spawns take longer than io's deadline for them.
                spawn: Span::millis(10, 1_200),
                unspawned: 60,
                children: 2,
                lingering: 300,
                holding: 300,
                stubborn: 200,
                ..calm.tree
            },
            script: script::Script {
                fates: Fates {
                    ended: 4,
                    parked: 2,
                    failed: 2,
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
                slow_exits: 200,
                deaf_to_cancel: 200,
                mute: 150,
                // Some wind down past the grace.
                wind: Span::millis(100, 8_000),
                stubborn: 200,
                ..calm.script
            },
            ..calm
        }
    }
}

/// What the world counted.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct Stats {
    pub client: client::Tally,
    pub tree: tree::Tally,
    /// How agents went, runs finished and agents failed, by kind.
    pub endings: BTreeMap<&'static str, u32>,
    pub finishes: BTreeMap<&'static str, u32>,
    pub faults: BTreeMap<&'static str, u32>,
    /// Breaches of the channel's rules caught, by kind.
    pub breaches: BTreeMap<&'static str, u32>,
    /// Inbound events bounced, by why.
    pub bounces: BTreeMap<&'static str, u32>,
    /// Calls the domain answered as busy itself, and answers that went down
    /// as too large.
    pub busy: u32,
    pub too_large: u32,
    /// Paths taken that the sweep must reach, by name.
    pub paths: BTreeMap<&'static str, u32>,
    /// Facts the domain told, by kind, and how many it dropped.
    pub facts: BTreeMap<&'static str, u32>,
    pub facts_lost: u64,
    /// The most agents at once.
    pub peak: u32,
}

/// Something on its way, delivered at its time.
#[derive(Debug)]
enum Delivery {
    /// An event reaching the domain.
    Domain(Event),
    /// A record of the domain's reaching the client.
    Client(Request),
    /// The client's own plan.
    Plan(client::Plan),
    /// Something of the process trees' own.
    Tree(tree::Due),
}

/// The one-way channels whose order matters, each delivering in the order it
/// was given. io's other terminals each take a latency of their own.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Lane {
    /// The reads of a channel, one at a time.
    Reads,
    /// A process's exit, then its reap.
    Exits,
    ClientToDomain,
    DomainToClient,
}

/// io's requests, by kind: each ends with exactly one terminal.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Kind {
    Spawn,
    Send,
    Read,
    Signal,
    Wait,
    Reap,
}

/// An agent as the world follows it, from what crossed the domain's boundary:
/// what the domain can only be expected to do knowing what it was told.
#[derive(Debug)]
#[expect(clippy::struct_excessive_bools, reason = "what crossed the boundary, each fact on its own")]
struct Mirror {
    client: Token,
    /// When its run started.
    started: Option<Time>,
    /// The run listens: started, and its channel neither ended nor dropped.
    listening: bool,
    /// The client was told how its run finishes, or how it failed.
    finished: bool,
    told: bool,
    /// The client stopped it.
    stopped: bool,
    /// Its tree was terminated.
    terminating: bool,
    /// The names of its calls in flight, as the domain passed them on and sent
    /// their answers down; those the client has not answered, and those of
    /// them the run withdrew.
    flight: BTreeSet<Token>,
    asked: BTreeSet<Token>,
    withdrawn: BTreeSet<Token>,
    /// Inbound events the domain sent down.
    sent: BTreeSet<u64>,
    /// When it first had a reason to stop, and when the client stopped it.
    first_stop: Option<Time>,
    stopped_at: Option<Time>,
    /// The fault the client was told.
    fault: Option<Fault>,
    /// io's requests in flight.
    pending: BTreeMap<Kind, u32>,
}

/// What the world judges of a message the domain reads, before the domain
/// does.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Verdict {
    /// The agent is being terminated: what it says is dropped.
    Unjudged,
    Fine,
    Breach(&'static str),
}

/// What one step of the domain took, for the world to check what it made.
#[derive(Clone, Copy, Debug)]
enum Taken {
    Spawn { client: Token, full: bool, invalid: bool },
    Read { owner: Token, verdict: Verdict },
    Other,
}

/// What one step of the domain made, without its payloads.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Made {
    Started {
        client: Token,
        agent: Token,
    },
    Called {
        client: Token,
        call: Token,
    },
    Withdrawn {
        client: Token,
        call: Token,
    },
    Said,
    Finished {
        client: Token,
        kind: &'static str,
    },
    Faulted {
        client: Token,
        fault: Fault,
    },
    Bounced {
        bounce: Bounce,
    },
    Gone {
        client: Token,
        end: End,
    },
    Spawn {
        owner: Token,
    },
    /// A send, with the call it answers, whether the domain answered it
    /// itself as busy, or as too large; or whether it is an event, or the
    /// cancel.
    Send {
        owner: Token,
        answer: Option<Token>,
        busy: bool,
        too_large: bool,
        event: bool,
        name: Option<u64>,
        cancel: bool,
    },
    Io {
        owner: Token,
        kind: Kind,
    },
    Signal {
        owner: Token,
        signal: Signal,
    },
}

pub struct World {
    now: Time,
    /// Draws the latencies.
    rng: Rng,
    settings: Settings,

    domain: agent::Domain,
    stage: Stage<Limits, Event, Request>,

    client: Client,
    tree: Tree,

    wire: Schedule<Delivery>,
    lanes: [Time; 4],

    /// The agents the domain spawned and that have not gone, by its tokens;
    /// and those tokens, by the client's.
    mirrors: BTreeMap<Token, Mirror>,
    owners: BTreeMap<Token, Token>,

    stats: Stats,
    trace: Trace,
}

impl World {
    #[must_use]
    pub fn new(settings: Settings) -> World {
        assert!(agent::worst_case(&settings.agent).is_some(), "the shell refuses limits it cannot provision");
        let mut rng = Rng::new(settings.seed);
        let limits = settings.agent;
        let sizes = Sizes {
            call: limits.call_bytes,
            fact: limits.fact_bytes,
            outcome: limits.outcome_bytes,
            snapshot: limits.snapshot_bytes,
            long: limits.long_span,
        };
        let client = Client::new(settings.client, limits, rng.next_u64());
        let tree = Tree::new(settings.tree, settings.script, sizes, rng.next_u64());
        let latencies = Rng::new(rng.next_u64());
        let mut world = World {
            now: Time::ZERO,
            rng: latencies,
            settings,
            domain: agent::Domain::new(&limits),
            stage: Stage::new(limits, agent::MAX_OUT, agent::MAX_OUT + SPARE),
            client,
            tree,
            wire: Schedule::new(),
            lanes: [Time::ZERO; 4],
            mirrors: BTreeMap::new(),
            owners: BTreeMap::new(),
            stats: Stats::default(),
            trace: Trace::default(),
        };
        let begin = world.client.begin();
        world.client_outs(begin);
        world
    }

    #[must_use]
    pub fn now(&self) -> Time {
        self.now
    }

    #[must_use]
    pub fn stats(&self) -> Stats {
        Stats {
            client: self.client.tally(),
            tree: self.tree.tally(),
            facts_lost: self.domain.facts_lost(),
            ..self.stats.clone()
        }
    }

    /// What crossed between the domain and the world, in order, with times.
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

    /// One iteration of the loop, as the shell would run it.
    fn iterate(&mut self) {
        self.stage.tick(self.now);
        self.deliver();
        // The stage takes its events, then fires its alarms, while it has
        // room for what one more may produce.
        while let Some(event) = self.stage.next_event() {
            self.trace.log(self.now, format!("agent <- {event:?}"));
            let taken = self.take(&event);
            let before = self.stage.out.len();
            agent::step(&mut self.domain, &self.stage.env, event, &mut self.stage.out);
            self.check(taken, before);
        }
        while self.stage.has_room() && self.domain.is_due(self.now) {
            self.trace.log(self.now, "agent alarm");
            let before = self.stage.out.len();
            agent::fire(&mut self.domain, &self.stage.env, &mut self.stage.out);
            self.check(Taken::Other, before);
        }
        // The facts, drained as the shell would write them out.
        while let Some(fact) = self.domain.pop_fact() {
            *self.stats.facts.entry(fact_kind(fact)).or_default() += 1;
        }
        // What the steps asked for, submitted at the end of the iteration.
        while let Some(request) = self.stage.out.pop() {
            self.trace.log(self.now, format!("agent -> {request:?}"));
            self.route(request);
        }
        self.domain.reclaim();
        let agents = self.domain.agents();
        assert!(agents <= self.settings.agent.agents, "agents stay within their slots");
        self.stats.peak = self.stats.peak.max(agents);
    }

    /// What the domain takes as `event`, noted before it takes it.
    fn take(&mut self, event: &Event) -> Taken {
        let limits = self.settings.agent;
        match event {
            temper_worker_domain_agent::Event::SpawnV2 { .. }
            | temper_worker_domain_agent::Event::TurnCredit { .. } => unreachable!("this script runs version one"),

            Event::Spawn { client, spawn } => {
                let full = self.domain.agents() >= limits.agents;
                let snapshot = spawn.snapshot.as_ref().map_or(0, |snapshot| len(snapshot));
                let invalid = len(&spawn.charter) > limits.charter_bytes || snapshot > limits.snapshot_bytes;
                Taken::Spawn { client: *client, full, invalid }
            }
            Event::Stop { agent } => {
                if let Some(mirror) = self.mirrors.get_mut(agent) {
                    mirror.stopped = true;
                    mirror.first_stop.get_or_insert(self.now);
                    mirror.stopped_at.get_or_insert(self.now);
                }
                Taken::Other
            }
            Event::Answer { agent, call, reply: _ } => {
                if let Some(mirror) = self.mirrors.get_mut(agent)
                    && mirror.listening
                {
                    mirror.asked.remove(call);
                    mirror.withdrawn.remove(call);
                }
                Taken::Other
            }
            Event::Deliver { .. } | Event::Grant { .. } => Taken::Other,
            Event::Spawned { owner, .. } | Event::Unspawned { owner, .. } => {
                self.ended(*owner, Kind::Spawn);
                Taken::Other
            }
            Event::Sent { owner } => {
                self.ended(*owner, Kind::Send);
                Taken::Other
            }
            Event::Unsent { owner } => {
                self.ended(*owner, Kind::Send);
                self.quiet(*owner);
                Taken::Other
            }
            Event::Received { owner, message } => {
                self.ended(*owner, Kind::Read);
                let verdict = self.judge(*owner, Some(message));
                Taken::Read { owner: *owner, verdict }
            }
            Event::Malformed { owner } => {
                self.ended(*owner, Kind::Read);
                let verdict = self.judge(*owner, None);
                self.quiet(*owner);
                Taken::Read { owner: *owner, verdict }
            }
            Event::Hangup { owner } => {
                self.ended(*owner, Kind::Read);
                let mirror = self.mirrors.get(owner).expect("io speaks of a spawned agent");
                if mirror.listening {
                    let path = if mirror.stopped { "hangup while cancelled" } else { "hangup while live" };
                    *self.stats.paths.entry(path).or_default() += 1;
                }
                self.quiet(*owner);
                Taken::Other
            }
            Event::Signalled { owner } => {
                self.ended(*owner, Kind::Signal);
                Taken::Other
            }
            Event::Exited { owner } => {
                self.ended(*owner, Kind::Wait);
                self.quiet(*owner);
                Taken::Other
            }
            Event::Reaped { owner, .. } => {
                self.ended(*owner, Kind::Reap);
                Taken::Other
            }
        }
    }

    /// Whether what the domain reads of an agent breaks the channel's rules,
    /// as far as the world can tell from what crossed the boundary.
    fn judge(&self, owner: Token, message: Option<&Up>) -> Verdict {
        let mirror = self.mirrors.get(&owner).expect("a read of a spawned agent");
        if mirror.terminating {
            return Verdict::Unjudged;
        }
        let Some(message) = message else {
            return Verdict::Breach("malformed");
        };
        if oversized(message, &self.settings.agent) {
            return Verdict::Breach("oversized");
        }
        if mirror.finished {
            return Verdict::Breach("after the finish");
        }
        if !mirror.listening {
            return Verdict::Fine;
        }
        match message {
            temper_worker_domain_agent::channel::Up::Turn { .. }
            | temper_worker_domain_agent::channel::Up::FinishV2 { .. } => unreachable!("this script runs version one"),

            Up::Call { call, .. } if mirror.flight.contains(call) => Verdict::Breach("reused name"),
            Up::Withdraw { call } if mirror.withdrawn.contains(call) => Verdict::Breach("withdrawn twice"),
            Up::Waiting { heard } if *heard != 0 && !mirror.sent.contains(heard) => Verdict::Breach("heard too much"),
            Up::Call { .. }
            | Up::Withdraw { .. }
            | Up::Fact { .. }
            | Up::Long { .. }
            | Up::LongDone
            | Up::Waiting { .. }
            | Up::Finish { .. }
            | Up::Rejected { .. }
            | Up::Exhausted { .. } => Verdict::Fine,
        }
    }

    /// The agent stopped talking: io said it exited, its channel ended, or it
    /// no longer reads it.
    fn quiet(&mut self, owner: Token) {
        let mirror = self.mirrors.get_mut(&owner).expect("io speaks of a spawned agent");
        mirror.listening = false;
        mirror.first_stop.get_or_insert(self.now);
    }

    /// io ended one of the requests of the agent `owner`.
    fn ended(&mut self, owner: Token, kind: Kind) {
        let mirror = self.mirrors.get_mut(&owner).expect("io speaks of a spawned agent");
        let count = mirror.pending.get_mut(&kind).expect("a terminal ends a request in flight");
        assert!(*count > 0, "each request ends once: {kind:?} of {owner:?}");
        *count -= 1;
    }

    /// Checks what a step made against what it took, then notes it.
    fn check(&mut self, taken: Taken, before: u32) {
        let made: Vec<Made> = self.stage.out.iter().skip(usize::try_from(before).expect("fits")).map(made).collect();
        let rules = made.iter().any(Made::is_rules);
        match taken {
            Taken::Spawn { client, full, invalid } => {
                let [made] = made.as_slice() else {
                    panic!("a spawn is refused, or asked of io: {made:?}");
                };
                match *made {
                    Made::Gone { client: refused, end } => {
                        assert_eq!(refused, client, "a refusal is the spawn's");
                        match end {
                            End::Busy => assert!(full, "busy only when every slot is taken"),
                            End::Invalid(_) => assert!(!full && invalid, "invalid only beyond the limits"),
                            End::Unspawned | End::Stopped => panic!("a spawn is refused at once, or spawned"),
                        }
                    }
                    Made::Spawn { owner } => {
                        assert!(!full && !invalid, "a spawn within the limits and the slots is asked of io");
                        self.mirrors.insert(owner, Mirror::new(client));
                        self.owners.insert(client, owner);
                    }
                    Made::Started { .. }
                    | Made::Called { .. }
                    | Made::Withdrawn { .. }
                    | Made::Said
                    | Made::Finished { .. }
                    | Made::Faulted { .. }
                    | Made::Bounced { .. }
                    | Made::Send { .. }
                    | Made::Io { .. }
                    | Made::Signal { .. } => panic!("a spawn is refused, or asked of io: {made:?}"),
                }
            }
            Taken::Read { owner, verdict } => {
                let mirror = self.mirrors.get(&owner).expect("a read of a spawned agent");
                let terminated = made.contains(&Made::Signal { owner, signal: Signal::Terminate });
                match verdict {
                    Verdict::Breach(kind) => {
                        assert!(terminated, "a breach ({kind}) of the channel's rules stops the agent");
                        let live = !mirror.told && !mirror.stopped;
                        assert_eq!(
                            rules, live,
                            "a breach ({kind}) is told as such while the run is live, and only then"
                        );
                        *self.stats.breaches.entry(kind).or_default() += 1;
                    }
                    Verdict::Fine => assert!(!terminated && !rules, "a message within the rules breaks none"),
                    Verdict::Unjudged => assert!(!rules, "what a tree being stopped says is dropped"),
                }
            }
            Taken::Other => assert!(!rules, "only a breach read is told as one"),
        }
        for made in made {
            self.note(made);
        }
    }

    /// Notes what a step made, checking what it may.
    #[expect(clippy::too_many_lines, reason = "each finite emitted record has its own referee transition")]
    fn note(&mut self, made: Made) {
        let now = self.now;
        let limits = self.settings.agent;
        match made {
            Made::Started { client, agent } => {
                let mirror = self.mirrors.get_mut(&agent).expect("a started agent was spawned");
                assert_eq!(mirror.client, client, "an agent is started for its spawn's client");
                assert!(mirror.started.is_none(), "an agent starts once");
                mirror.started = Some(now);
                mirror.listening = true;
            }
            Made::Called { client, call } => {
                let mirror = self.mirror_of(client);
                assert!(mirror.listening, "a call is passed on only while the run listens");
                assert!(mirror.flight.insert(call), "a call reusing a name in flight is never passed on");
                mirror.asked.insert(call);
            }
            Made::Withdrawn { client, call } => {
                let mirror = self.mirror_of(client);
                assert!(mirror.listening, "a withdraw is passed on only while the run listens");
                assert!(mirror.asked.contains(&call), "only a call the client has to answer is withdrawn");
                assert!(mirror.withdrawn.insert(call), "a call is withdrawn once");
            }
            Made::Said => {}
            Made::Finished { client, kind } => {
                let mirror = self.mirror_of(client);
                assert!(!mirror.told, "a client hears at most one finish or fault");
                let late = mirror.terminating;
                mirror.told = true;
                mirror.finished = true;
                mirror.listening = false;
                mirror.first_stop.get_or_insert(now);
                *self.stats.finishes.entry(kind).or_default() += 1;
                if late {
                    *self.stats.paths.entry("finish while terminating").or_default() += 1;
                }
            }
            Made::Faulted { client, fault } => self.faulted(client, fault),
            Made::Bounced { bounce } => {
                let kind = match bounce {
                    Bounce::TooLarge => "too large",
                    Bounce::Full => "full",
                    Bounce::Ending => "ending",
                };
                *self.stats.bounces.entry(kind).or_default() += 1;
            }
            Made::Gone { client, end } => {
                *self.stats.endings.entry(end_kind(end)).or_default() += 1;
                // A spawn refused at the entrance was never mirrored.
                let Some(owner) = self.owners.remove(&client) else {
                    return;
                };
                let mirror = self.mirrors.remove(&owner).expect("mirrored with its owner");
                let pending: Vec<_> = mirror.pending.iter().filter(|(_, count)| **count > 0).collect();
                assert!(pending.is_empty(), "an agent goes once nothing asked of io is in flight: {pending:?}");
                match end {
                    End::Stopped => {
                        assert!(self.tree.is_gone(owner), "stopped only once the process exited and its tree is empty");
                        let first = mirror.first_stop.expect("an agent stops for a reason");
                        // Past the kill, what it wrote is still read out
                        // of the pipe.
                        let bound = first
                            .saturating_add(limits.grace)
                            .saturating_add(limits.kill_after)
                            .saturating_add(self.settings.tree.pipe.max);
                        assert!(
                            now <= bound.saturating_add(SLACK),
                            "a stopping agent is terminated, then killed, in time"
                        );
                        self.silenced(owner, &mirror);
                    }
                    End::Unspawned => assert!(mirror.started.is_none(), "an agent unspawned never started"),
                    End::Busy | End::Invalid(_) => panic!("a spawn asked of io is not refused"),
                }
            }
            Made::Spawn { owner } => self.opened(owner, Kind::Spawn),
            Made::Send { owner, answer, busy, too_large, event: _, name, cancel } => {
                self.opened(owner, Kind::Send);
                let mirror = self.mirrors.get_mut(&owner).expect("mirrored");
                if let Some(call) = answer {
                    mirror.flight.remove(&call);
                }
                if let Some(name) = name {
                    mirror.sent.insert(name);
                }
                if cancel {
                    mirror.first_stop.get_or_insert(now);
                }
                self.stats.busy += u32::from(busy);
                self.stats.too_large += u32::from(too_large);
            }
            Made::Io { owner, kind } => self.opened(owner, kind),
            Made::Signal { owner, signal } => {
                self.opened(owner, Kind::Signal);
                let mirror = self.mirrors.get_mut(&owner).expect("mirrored");
                match signal {
                    Signal::Terminate => {
                        assert!(!mirror.terminating, "a tree is terminated once");
                        mirror.terminating = true;
                        mirror.listening = false;
                        mirror.first_stop.get_or_insert(now);
                    }
                    Signal::Kill => assert!(mirror.terminating, "a tree is killed only once terminated"),
                }
            }
        }
    }

    /// The client is told the agent failed, for `fault`: only while its run
    /// is live, and only as the fault allows.
    fn faulted(&mut self, client: Token, fault: Fault) {
        let now = self.now;
        let limits = self.settings.agent;
        let owner = *self.owners.get(&client).expect("a faulted agent was spawned");
        let mirror = self.mirror_of(client);
        assert!(!mirror.told && !mirror.stopped, "a fault is told only while the run is live");
        mirror.told = true;
        mirror.fault = Some(fault);
        mirror.listening = false;
        mirror.first_stop.get_or_insert(now);
        let started = mirror.started.expect("a faulted agent started");
        match fault {
            Fault::NoProgress => {
                let view = self.tree.view(owner);
                assert!(!view.blocked, "the watchdog never fires while the run waits for a call's answer");
                assert!(!view.waiting, "nor while it waits for an inbound event");
                let silent = view.last_write.saturating_add(limits.no_progress);
                assert!(now >= silent, "nor before the run has been silent past the deadline: {view:?}");
                let held = view.long_until.saturating_add(limits.no_progress);
                assert!(now >= held, "nor before a long operation's deadline: {view:?}");
            }
            Fault::WallTime => {
                assert!(now >= started.saturating_add(limits.wall_time), "the wall time is up");
            }
            Fault::Exited => assert!(self.tree.quiet(owner), "an agent failed for exiting stopped talking"),
            Fault::Rules => {}
        }
        *self.stats.faults.entry(fault_kind(fault)).or_default() += 1;
    }

    /// An agent whose script hung while live, with no call waiting for an
    /// answer, was stopped by the watchdog: unless the client stopped it, or
    /// its wall time came, before the watchdog could fire.
    fn silenced(&mut self, owner: Token, mirror: &Mirror) {
        let limits = self.settings.agent;
        let Some(hung) = self.tree.view(owner).hung else {
            return;
        };
        if mirror.fault == Some(Fault::NoProgress) {
            *self.stats.paths.entry("silence caught").or_default() += 1;
            return;
        }
        let deadline = hung.saturating_add(limits.no_progress).saturating_add(SLACK);
        let started = mirror.started.expect("a hung agent started");
        let stopped = mirror.stopped_at.is_some_and(|at| at <= deadline);
        let overdue = started.saturating_add(limits.wall_time) <= deadline;
        assert!(stopped || overdue, "a run silent while live is stopped by the watchdog: {mirror:?}");
    }

    fn opened(&mut self, owner: Token, kind: Kind) {
        let mirror = self.mirrors.get_mut(&owner).expect("io is asked for a spawned agent");
        *mirror.pending.entry(kind).or_default() += 1;
        match kind {
            Kind::Signal => {}
            Kind::Spawn | Kind::Send | Kind::Read | Kind::Wait | Kind::Reap => {
                assert_eq!(mirror.pending[&kind], 1, "one {kind:?} at a time");
            }
        }
    }

    fn mirror_of(&mut self, client: Token) -> &mut Mirror {
        let owner = self.owners.get(&client).expect("the client's agent was spawned");
        self.mirrors.get_mut(owner).expect("mirrored with its owner")
    }

    fn route(&mut self, request: Request) {
        match request {
            temper_worker_domain_agent::Request::Turn { .. }
            | temper_worker_domain_agent::Request::FinishedV2 { .. } => unreachable!("this script runs version one"),

            Request::Started { .. }
            | Request::Called { .. }
            | Request::Withdrawn { .. }
            | Request::Waiting { .. }
            | Request::Told { .. }
            | Request::Finished { .. }
            | Request::Faulted { .. }
            | Request::Bounced { .. }
            | Request::Rejected { .. }
            | Request::Exhausted { .. }
            | Request::Gone { .. } => {
                let at = self.lane(Lane::DomainToClient, Duration::ZERO);
                self.wire.send(at, Delivery::Client(request));
            }
            Request::Spawn { .. }
            | Request::Send { .. }
            | Request::Read { .. }
            | Request::Signal { .. }
            | Request::Wait { .. }
            | Request::Reap { .. } => {
                let outs = self.tree.take(self.now, request);
                self.tree_outs(outs);
            }
        }
    }

    fn tree_outs(&mut self, outs: Vec<tree::Out>) {
        for out in outs {
            match out {
                tree::Out::Domain { after, event } => {
                    let lane = match event {
                        temper_worker_domain_agent::Event::SpawnV2 { .. }
                        | temper_worker_domain_agent::Event::TurnCredit { .. } => {
                            unreachable!("this script runs version one")
                        }

                        Event::Received { .. } | Event::Malformed { .. } | Event::Hangup { .. } => Some(Lane::Reads),
                        Event::Exited { .. } | Event::Reaped { .. } => Some(Lane::Exits),
                        Event::Spawned { .. }
                        | Event::Unspawned { .. }
                        | Event::Sent { .. }
                        | Event::Unsent { .. }
                        | Event::Signalled { .. } => None,
                        Event::Spawn { .. }
                        | Event::Deliver { .. }
                        | Event::Answer { .. }
                        | Event::Stop { .. }
                        | Event::Grant { .. } => {
                            unreachable!("io ends requests")
                        }
                    };
                    let at = match lane {
                        Some(lane) => self.lane(lane, after),
                        None => self.now.saturating_add(after).saturating_add(self.settings.hop.draw(&mut self.rng)),
                    };
                    self.wire.send(at, Delivery::Domain(event));
                }
                tree::Out::Due { after, due } => {
                    self.wire.send(self.now.saturating_add(after), Delivery::Tree(due));
                }
                // What an agent writes reaches the domain through the channel.
                tree::Out::Wrote { .. } => {}
            }
        }
    }

    fn client_outs(&mut self, outs: Vec<client::Out>) {
        for out in outs {
            match out {
                client::Out::Domain(event) => {
                    let at = self.lane(Lane::ClientToDomain, Duration::ZERO);
                    self.wire.send(at, Delivery::Domain(event));
                }
                client::Out::Later { after, plan } => {
                    self.wire.send(self.now.saturating_add(after), Delivery::Plan(plan));
                }
            }
        }
    }

    /// Hands over what is due now.
    fn deliver(&mut self) {
        while let Some(delivery) = self.wire.next(self.now) {
            match delivery {
                Delivery::Domain(event) => self.stage.push(event),
                Delivery::Client(request) => {
                    let outs = self.client.take(request);
                    self.client_outs(outs);
                }
                Delivery::Plan(plan) => {
                    let outs = self.client.plan(plan);
                    self.client_outs(outs);
                }
                Delivery::Tree(due) => {
                    let outs = self.tree.due(self.now, due);
                    self.tree_outs(outs);
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
            Lane::ClientToDomain => 2,
            Lane::DomainToClient => 3,
        };
        let at = self.now.saturating_add(after).saturating_add(self.settings.hop.draw(&mut self.rng));
        let at = at.max(self.lanes[index]);
        self.lanes[index] = at;
        at
    }

    fn has_work_now(&self) -> bool {
        self.stage.has_events() || self.domain.is_due(self.now) || self.wire.is_due(self.now)
    }

    fn next_time(&self) -> Option<Time> {
        [self.wire.next_time(), self.domain.next_deadline()].into_iter().flatten().min()
    }

    /// Checks the invariants of a world with nothing left to happen.
    fn assert_settled(&self) {
        assert!(self.wire.is_empty(), "nothing is in flight");
        assert!(!self.stage.has_events(), "the domain has taken everything");
        assert_eq!(self.domain.agents(), 0, "every slot is free");
        assert_eq!(self.domain.next_deadline(), None, "no alarm outlives its agent");
        assert!(self.mirrors.is_empty() && self.owners.is_empty(), "every agent spawned has gone");
        self.client.assert_settled();
        self.tree.assert_settled();
        if self.domain.facts_lost() == 0 {
            let facts = &self.stats.facts;
            let count = |kind| facts.get(kind).copied().unwrap_or(0);
            assert_eq!(count("started"), self.client.tally().started, "every start is told");
            assert_eq!(count("gone"), self.client.tally().spawns, "every agent's end is told");
        }
    }
}

impl Made {
    fn is_rules(&self) -> bool {
        match self {
            Made::Faulted { client: _, fault } => *fault == Fault::Rules,
            Made::Started { .. }
            | Made::Called { .. }
            | Made::Withdrawn { .. }
            | Made::Said
            | Made::Finished { .. }
            | Made::Bounced { .. }
            | Made::Gone { .. }
            | Made::Spawn { .. }
            | Made::Send { .. }
            | Made::Io { .. }
            | Made::Signal { .. } => false,
        }
    }
}

impl Mirror {
    fn new(client: Token) -> Mirror {
        Mirror {
            client,
            started: None,
            listening: false,
            finished: false,
            told: false,
            stopped: false,
            terminating: false,
            flight: BTreeSet::new(),
            asked: BTreeSet::new(),
            withdrawn: BTreeSet::new(),
            sent: BTreeSet::new(),
            first_stop: None,
            stopped_at: None,
            fault: None,
            pending: BTreeMap::new(),
        }
    }
}

fn made(request: &Request) -> Made {
    match request {
        temper_worker_domain_agent::Request::Turn { .. } | temper_worker_domain_agent::Request::FinishedV2 { .. } => {
            unreachable!("this script runs version one")
        }

        Request::Started { client, agent } => Made::Started { client: *client, agent: *agent },
        Request::Called { client, call, ask: _ } => Made::Called { client: *client, call: *call },
        Request::Withdrawn { client, call } => Made::Withdrawn { client: *client, call: *call },
        Request::Waiting { .. } | Request::Told { .. } | Request::Rejected { .. } | Request::Exhausted { .. } => {
            Made::Said
        }
        Request::Finished { client, finish } => {
            let kind = match finish {
                Finish::Ended { .. } => "ended",
                Finish::Parked { .. } => "parked",
                Finish::Failed { .. } => "failed",
            };
            Made::Finished { client: *client, kind }
        }
        Request::Faulted { client, fault } => Made::Faulted { client: *client, fault: *fault },
        Request::Bounced { name: _, client: _, bounce } => Made::Bounced { bounce: *bounce },
        Request::Gone { client, end, detail: _ } => Made::Gone { client: *client, end: *end },
        Request::Spawn { owner, workspace: _, deadline: _ } => Made::Spawn { owner: *owner },
        Request::Send { owner, process: _, message } => {
            let (answer, busy, too_large) = match message {
                temper_worker_domain_agent::channel::Down::StartV2 { .. } => {
                    unreachable!("this script runs version one")
                }

                Down::Answer { call, reply } => match reply {
                    Reply::Busy => (Some(*call), true, false),
                    Reply::TooLarge => (Some(*call), false, true),
                    Reply::Relayed { .. } | Reply::Pushed(_) | Reply::Unavailable | Reply::Withdrawn => {
                        (Some(*call), false, false)
                    }
                },
                Down::Start { .. } | Down::Event { .. } | Down::Cancel | Down::Grant { .. } => (None, false, false),
            };
            let (event, cancel) = match message {
                temper_worker_domain_agent::channel::Down::StartV2 { .. } => {
                    unreachable!("this script runs version one")
                }

                Down::Event { .. } => (true, false),
                Down::Cancel => (false, true),
                Down::Start { .. } | Down::Answer { .. } | Down::Grant { .. } => (false, false),
            };
            Made::Send {
                owner: *owner,
                answer,
                busy,
                too_large,
                event,
                name: match message {
                    temper_worker_domain_agent::channel::Down::StartV2 { .. } => {
                        unreachable!("this script runs version one")
                    }

                    Down::Event { name, .. } => Some(name.raw()),
                    Down::Start { .. } | Down::Answer { .. } | Down::Cancel | Down::Grant { .. } => None,
                },
                cancel,
            }
        }
        Request::Read { owner, .. } => Made::Io { owner: *owner, kind: Kind::Read },
        Request::Signal { owner, process: _, signal } => Made::Signal { owner: *owner, signal: *signal },
        Request::Wait { owner, .. } => Made::Io { owner: *owner, kind: Kind::Wait },
        Request::Reap { owner, .. } => Made::Io { owner: *owner, kind: Kind::Reap },
    }
}

fn oversized(message: &Up, limits: &Limits) -> bool {
    match message {
        temper_worker_domain_agent::channel::Up::Turn { .. }
        | temper_worker_domain_agent::channel::Up::FinishV2 { .. } => unreachable!("this script runs version one"),

        Up::Call { call: _, ask } => {
            let body = match ask {
                temper_worker_domain_agent::channel::Ask::PushV2 { .. } => unreachable!("this script runs version one"),

                Ask::Push { message } => message,
                Ask::Relay { body } => body,
            };
            len(body) > limits.call_bytes
        }
        Up::Fact { fact } => len(fact) > limits.fact_bytes,
        Up::Long { span } => *span > limits.long_span,
        Up::Withdraw { .. } | Up::LongDone | Up::Waiting { .. } | Up::Rejected { .. } | Up::Exhausted { .. } => false,
        Up::Finish { finish } => match finish {
            Finish::Ended { outcome } => len(outcome) > limits.outcome_bytes,
            Finish::Parked { snapshot } => {
                snapshot.as_ref().is_some_and(|snapshot| len(snapshot) > limits.snapshot_bytes)
            }
            Finish::Failed { .. } => false,
        },
    }
}

fn len(bytes: &[u8]) -> u64 {
    u64::try_from(bytes.len()).expect("fits")
}

fn fact_kind(fact: Fact) -> &'static str {
    match fact {
        Fact::Started { .. } => "started",
        Fact::Finished { .. } => "finished",
        Fact::Cancelled { .. } => "cancelled",
        Fact::Overdue { .. } => "overdue",
        Fact::Faulted { .. } => "faulted",
        Fact::Terminated { .. } => "terminated",
        Fact::Killed { .. } => "killed",
        Fact::Gone { .. } => "gone",
    }
}

fn fault_kind(fault: Fault) -> &'static str {
    match fault {
        Fault::Exited => "exited",
        Fault::Rules => "rules",
        Fault::NoProgress => "no progress",
        Fault::WallTime => "wall time",
    }
}

fn end_kind(end: End) -> &'static str {
    match end {
        End::Busy => "busy",
        End::Invalid(_) => "invalid",
        End::Unspawned => "unspawned",
        End::Stopped => "stopped",
    }
}
