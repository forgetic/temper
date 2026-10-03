//! Memory stays within the worst case (programming-model.md, 6.3), measured by
//! a counting allocator: the agent child domain with every slot holding a spawn
//! of exactly its limits, then every run's outbox full of what may wait for
//! it, then every agent ending with as much detail as it may keep, its facts'
//! queue full; and every entry point along the way. The fill reaches the
//! worst case, short only of what no agent can hold at once.

use skein_lib::{Duration, Env, Queue, Set, Time, Token, Wall};
use temper_worker_domain_agent::channel::{Ask, Finish, Reply, Up};
use temper_worker_domain_agent::{
    Bounce, Domain, End, Event, Fault, Invalid, Limits, MAX_OUT, Request, Signal, Spawn, fire, step, worst_case,
};
use temper_world::heap::{self, Meter};

#[global_allocator]
static HEAP: heap::Counting = heap::Counting;

const LIMITS: Limits = Limits {
    agents: 2,
    charter_bytes: 1024,
    snapshot_bytes: 512,
    event_bytes: 256,
    events: 2,
    calls: 2,
    call_bytes: 128,
    answer_bytes: 384,
    fact_bytes: 64,
    outcome_bytes: 256,
    detail_bytes: 128,
    spawn_timeout: Duration::from_secs(1),
    no_progress: Duration::from_secs(10),
    long_span: Duration::from_secs(60),
    wall_time: Duration::from_secs(100),
    grace: Duration::from_secs(5),
    kill_after: Duration::from_secs(2),
    facts: 4,
};

fn bytes(len: u64) -> Box<[u8]> {
    vec![b'x'; usize::try_from(len).expect("a test length fits")].into_boxed_slice()
}

/// What a step asked for, without the payload.
#[derive(PartialEq, Eq, Debug)]
enum Asked {
    Started { agent: Token },
    Called,
    Spawn { owner: Token },
    Send,
    Read,
    Signal(Signal),
    Gone(End),
    Finished,
    Faulted(Fault),
    Bounced(Bounce),
    Other,
}

/// The agent child domain under `limits`, measured: each step's peak is checked
/// against the worst case, less what it handed out in requests, which their
/// receivers count.
struct Measured {
    domain: Domain,
    env: Env<Limits>,
    out: Queue<Request>,
    meter: Meter,
    bound: u64,
    /// The most held between steps, so far.
    peak: u64,
}

impl Measured {
    fn new(limits: Limits) -> Measured {
        let bound = worst_case(&limits).expect("the test limits fit");
        let meter = Meter::new();
        let domain = Domain::new(&limits);
        let out = Queue::with_capacity(MAX_OUT);
        Measured { domain, env: Env { now: Time::ZERO, wall: Wall::EPOCH, limits }, out, meter, bound, peak: 0 }
    }

    fn step(&mut self, event: Event) -> Vec<Asked> {
        self.meter.start();
        step(&mut self.domain, &self.env, event, &mut self.out);
        self.drain()
    }

    /// Fires what is due at `secs`.
    fn fire(&mut self, secs: u64) -> Vec<Asked> {
        self.env.now = Time::ZERO.saturating_add(Duration::from_secs(secs));
        assert!(self.domain.is_due(self.env.now), "an alarm is due");
        self.meter.start();
        fire(&mut self.domain, &self.env, &mut self.out);
        self.drain()
    }

    fn drain(&mut self) -> Vec<Asked> {
        let measured = self.meter.end();
        let mut asked = Vec::new();
        while let Some(request) = self.out.pop() {
            asked.push(match request {
                Request::Started { agent, .. } => Asked::Started { agent },
                Request::Called { .. } | Request::Withdrawn { .. } => Asked::Called,
                Request::Spawn { owner, .. } => Asked::Spawn { owner },
                Request::Send { .. } => Asked::Send,
                Request::Read { .. } => Asked::Read,
                Request::Signal { signal, .. } => Asked::Signal(signal),
                Request::Gone { end, .. } => Asked::Gone(end),
                Request::Finished { .. } => Asked::Finished,
                Request::Faulted { fault, .. } => Asked::Faulted(fault),
                Request::Bounced { bounce, .. } => Asked::Bounced(bounce),
                Request::Waiting { .. } | Request::Told { .. } | Request::Wait { .. } | Request::Reap { .. } => {
                    Asked::Other
                }
            });
        }
        self.meter.check(measured, self.bound, self.env.limits);
        self.peak = self.peak.max(self.meter.held());
        // The iteration ends: the reclaim point.
        self.domain.reclaim();
        asked
    }

    /// Spawns an agent of exactly the limits for `client`: its token.
    fn spawn(&mut self, client: u64) -> Token {
        let limits = self.env.limits;
        let spawn = Spawn {
            workspace: Token::new(client),
            charter: bytes(limits.charter_bytes),
            snapshot: Some(bytes(limits.snapshot_bytes)),
        };
        let asked = self.step(Event::Spawn { client: Token::new(client), spawn });
        let [Asked::Spawn { owner }] = asked[..] else {
            panic!("a spawn of exactly the limits is admitted: {asked:?}");
        };
        owner
    }

    /// Has the agent `owner`'s process spawned: its run is live, its start
    /// message in flight.
    fn start(&mut self, owner: Token) {
        let asked = self.step(Event::Spawned { owner, process: owner });
        assert_eq!(asked, [Asked::Started { agent: owner }, Asked::Other, Asked::Other, Asked::Send, Asked::Read]);
    }

    fn say(&mut self, owner: Token, message: Up) -> Vec<Asked> {
        self.step(Event::Received { owner, message })
    }
}

fn fill(limits: Limits) {
    let mut agent = Measured::new(limits);
    let owners: Vec<Token> = (0..u64::from(limits.agents)).map(|client| agent.spawn(client)).collect();
    let spawning = u64::from(limits.agents) * (limits.charter_bytes + limits.snapshot_bytes);
    assert!(agent.meter.held() >= spawning, "{limits:?}: every slot holds what it may as it spawns");
    let busy = Spawn { workspace: Token::new(99), charter: bytes(1), snapshot: None };
    assert_eq!(agent.step(Event::Spawn { client: Token::new(99), spawn: busy }), [Asked::Gone(End::Busy)]);

    // Every run's outbox full: the start in flight, an answer for every call
    // and every event that may wait, all of their limits.
    for owner in &owners {
        agent.start(*owner);
        for call in 0..u64::from(limits.calls) {
            let ask = Ask::Relay { body: bytes(limits.call_bytes) };
            assert_eq!(agent.say(*owner, Up::Call { call: Token::new(call), ask }), [Asked::Called, Asked::Read]);
        }
        for call in 0..u64::from(limits.calls) {
            let reply = Reply::Relayed { answer: bytes(limits.answer_bytes) };
            assert!(agent.step(Event::Answer { agent: *owner, call: Token::new(call), reply }).is_empty());
        }
        for _ in 0..limits.events {
            assert!(agent.step(Event::Deliver { agent: *owner, event: bytes(limits.event_bytes) }).is_empty());
        }
        let full = agent.step(Event::Deliver { agent: *owner, event: bytes(1) });
        assert_eq!(full, [Asked::Bounced(Bounce::Full)]);
    }
    let waiting = u64::from(limits.calls) * limits.answer_bytes + u64::from(limits.events) * limits.event_bytes;
    let held = agent.meter.held();
    assert!(held >= u64::from(limits.agents) * waiting, "{limits:?}: every outbox holds what may wait ({held})");

    // Each run finishes, and its agent goes with as much detail as it may
    // keep, and more.
    for owner in &owners {
        let finish = Finish::Parked { snapshot: Some(bytes(limits.snapshot_bytes)) };
        assert_eq!(agent.say(*owner, Up::Finish { finish }), [Asked::Finished, Asked::Read]);
        assert!(agent.step(Event::Sent { owner: *owner }).is_empty());
        assert!(agent.step(Event::Exited { owner: *owner }).is_empty());
        let detail = bytes(u64::from(limits.detail_bytes) * 2);
        assert!(agent.step(Event::Reaped { owner: *owner, detail }).is_empty(), "still read");
    }
    for owner in &owners {
        assert_eq!(agent.step(Event::Hangup { owner: *owner }), [Asked::Gone(End::Stopped)]);
    }
    assert_eq!(agent.domain.agents(), 0, "every slot came back");
    assert!(agent.domain.facts_lost() > 0, "{limits:?}: the facts' queue was filled, and more");
    // The bound is reached, not only respected: short of it by no more than
    // what no agent holds at once, the detail of its end (kept once its
    // outbox has gone), and the bookkeeping of its three sets of call names
    // (those in flight, those the client has to answer, those withdrawn),
    // which the worst case counts at its most. Every payload is reached.
    let names = Set::<Token>::worst_case(limits.calls).expect("fits");
    let apart = u64::from(limits.agents) * (u64::from(limits.detail_bytes) + 3 * names);
    let (peak, bound) = (agent.peak, agent.bound);
    assert!(peak + apart >= bound, "{limits:?}: {peak} held at the most, short of the worst case of {bound}");

    let beyond = Spawn { workspace: Token::new(0), charter: bytes(limits.charter_bytes + 1), snapshot: None };
    let refused = agent.step(Event::Spawn { client: Token::new(0), spawn: beyond });
    assert_eq!(refused, [Asked::Gone(End::Invalid(Invalid::Charter))]);
}

/// Every entry point, along every path an agent takes to its end.
fn paths(limits: Limits) {
    let mut agent = Measured::new(limits);
    // Unspawned.
    let owner = agent.spawn(0);
    let detail = bytes(u64::from(limits.detail_bytes) + 1);
    assert_eq!(agent.step(Event::Unspawned { owner, detail }), [Asked::Gone(End::Unspawned)]);

    // Hung: the watchdog fires, the tree is terminated, then killed.
    let owner = agent.spawn(1);
    agent.start(owner);
    assert!(agent.step(Event::Sent { owner }).is_empty());
    let fact = Up::Fact { fact: bytes(limits.fact_bytes) };
    assert_eq!(agent.say(owner, fact), [Asked::Other, Asked::Read]);
    let fired = agent.fire(10);
    assert_eq!(fired, [Asked::Faulted(Fault::NoProgress), Asked::Signal(Signal::Terminate)]);
    assert_eq!(agent.fire(12), [Asked::Signal(Signal::Kill)]);
    for event in [Event::Signalled { owner }, Event::Signalled { owner }, Event::Exited { owner }] {
        assert!(agent.step(event).is_empty());
    }
    assert!(agent.step(Event::Reaped { owner, detail: bytes(3) }).is_empty());
    assert_eq!(agent.step(Event::Hangup { owner }), [Asked::Gone(End::Stopped)]);

    // Busy, cancelled, past its wall time, drained after an exit.
    let owner = agent.spawn(2);
    agent.start(owner);
    for call in 0..u64::from(limits.calls) {
        let ask = Ask::Push { message: bytes(limits.call_bytes) };
        assert_eq!(agent.say(owner, Up::Call { call: Token::new(call), ask }), [Asked::Called, Asked::Read]);
    }
    let ask = Ask::Push { message: bytes(limits.call_bytes) };
    let busy = agent.say(owner, Up::Call { call: Token::new(99), ask });
    assert_eq!(busy, [Asked::Read], "the busy answer waits behind the start, and the run is read on");
    let ask = Ask::Push { message: bytes(limits.call_bytes) };
    let second = agent.say(owner, Up::Call { call: Token::new(98), ask });
    assert!(second.is_empty(), "a second busy answer waits too, and nothing more is read");
    assert_eq!(agent.step(Event::Sent { owner }), [Asked::Send, Asked::Read], "a busy answer goes down");
    assert_eq!(agent.step(Event::Sent { owner }), [Asked::Send], "and the other");
    assert_eq!(
        agent.step(Event::Deliver { agent: owner, event: bytes(limits.event_bytes + 1) }),
        [Asked::Bounced(Bounce::TooLarge)]
    );
    // Spawned at 12s, after the hung agent's kill.
    let wall = 12 + limits.wall_time.as_nanos() / 1_000_000_000;
    assert!(agent.fire(wall).is_empty(), "the cancel waits behind the busy answer");
    assert_eq!(agent.step(Event::Sent { owner }), [Asked::Send], "the cancel goes down");
    let grace = wall + limits.grace.as_nanos() / 1_000_000_000;
    assert_eq!(agent.fire(grace), [Asked::Faulted(Fault::WallTime), Asked::Signal(Signal::Terminate)]);
    let late = Up::Finish { finish: Finish::Ended { outcome: bytes(limits.outcome_bytes) } };
    assert!(!agent.say(owner, late).is_empty(), "dropped, and read on");
    assert!(agent.step(Event::Stop { agent: owner }).is_empty());
    for event in
        [Event::Sent { owner }, Event::Signalled { owner }, Event::Exited { owner }, Event::Malformed { owner }]
    {
        assert!(agent.step(event).is_empty());
    }
    assert_eq!(agent.step(Event::Reaped { owner, detail: bytes(1) }), [Asked::Gone(End::Stopped)]);

    let owner = agent.spawn(3);
    agent.start(owner);
    assert!(agent.step(Event::Stop { agent: owner }).is_empty(), "the cancel waits behind the start");
    assert_eq!(agent.step(Event::Sent { owner }), [Asked::Send]);
    assert!(agent.step(Event::Unsent { owner }).is_empty());
    assert!(agent.step(Event::Exited { owner }).is_empty());
    let finish = Up::Finish { finish: Finish::Ended { outcome: bytes(limits.outcome_bytes) } };
    assert_eq!(agent.say(owner, finish), [Asked::Finished, Asked::Read]);
    assert!(agent.step(Event::Hangup { owner }).is_empty());
    assert_eq!(agent.step(Event::Reaped { owner, detail: bytes(1) }), [Asked::Gone(End::Stopped)]);
    assert_eq!(agent.domain.agents(), 0, "every slot came back");
}

#[test]
fn an_agent_child_domain_with_every_slot_full_stays_within_its_worst_case() {
    fill(LIMITS);
    fill(Limits { agents: 16, events: 8, calls: 8, ..LIMITS });
    fill(Limits { agents: 64, charter_bytes: 65_536, snapshot_bytes: 16_384, ..LIMITS });
    fill(Limits { events: 0, calls: 1, ..LIMITS });
    // What waits to go down dominates what an agent holds: the listening
    // side of the max.
    fill(Limits { charter_bytes: 16, snapshot_bytes: 16, events: 8, event_bytes: 4096, ..LIMITS });
}

#[test]
fn every_entry_point_stays_within_the_worst_case() {
    paths(LIMITS);
    paths(Limits { agents: 8, calls: 1, events: 1, ..LIMITS });
}
