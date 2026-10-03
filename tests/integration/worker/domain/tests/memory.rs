//! Memory stays within the worst case (programming-model.md, 6.3), measured by
//! a counting allocator: the whole worker, every entry point of it, with
//! every slot holding a run of exactly its limits as it prepares, then the
//! channel lost, the runs filling what the worker keeps for the engine
//! meanwhile (the relays, the run's facts, the answers), and the channel
//! back.
//!
//! The harness plays io and the engine by hand: each request is answered at
//! once by what io would say if all went well, but for what a test decides
//! itself (what an agent says, when it exits, what the engine sends).

use std::collections::VecDeque;

use skein_lib::{Duration, Env, Queue, Time, Token, Wall};
use temper_worker_domain::agent::channel::{Ask, Finish, RunFailure, Up};
use temper_worker_domain::checkout::git::{Commit, Done, Op};
use temper_worker_domain::host::{Access, Assignment, Repository, Start, Workspace};
use temper_worker_domain::{Domain, Event, Limits, Phase, Request, fire, max_out, resume, step, worst_case};
use temper_worker_domain_tests::Settings;
use temper_world::heap::{self, Meter};

#[global_allocator]
static HEAP: heap::Counting = heap::Counting;

fn bytes(len: u64) -> Box<[u8]> {
    vec![b'x'; usize::try_from(len).expect("a test length fits")].into_boxed_slice()
}

/// What the worker asked of the engine, without the payload.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Asked {
    Dial,
    Hello { hosting: usize, answered: usize },
    Answer { run: Token, attempt: Token },
    Relay,
    Bounced,
}

/// An io terminal owed, to be stepped in turn.
#[derive(Debug)]
enum Owed {
    Done { owner: Token, done: Done },
    Spawned { owner: Token },
    Sent { owner: Token },
    Signalled { owner: Token },
}

/// The worker under `limits`, measured: each entry point's peak is checked
/// against the worst case, less what it handed out in requests, which their
/// receivers count. What the harness keeps of io is made before the meter,
/// with room enough, so that it is not counted.
struct Measured {
    domain: Domain,
    env: Env<Limits>,
    out: Queue<Request>,
    owed: VecDeque<Owed>,
    /// Agents spawned, in order; those with a read, a wait or a reap in
    /// flight.
    agents: Vec<Token>,
    reading: Vec<Token>,
    waiting: Vec<Token>,
    reaping: Vec<Token>,
    asked: Vec<Asked>,
    commits: u8,
    meter: Meter,
    bound: u64,
}

impl Measured {
    fn new(limits: &Limits) -> Measured {
        let bound = worst_case(limits).expect("the test limits fit");
        let room = 1024;
        let owed = VecDeque::with_capacity(room);
        let (agents, readings, waits, reaps, asked) = (
            Vec::with_capacity(room),
            Vec::with_capacity(room),
            Vec::with_capacity(room),
            Vec::with_capacity(room),
            Vec::with_capacity(room),
        );
        // The shell's queue, not the domain's.
        let out = Queue::with_capacity(max_out(limits));
        let meter = Meter::new();
        let domain = Domain::new(limits, 7);
        let env = Env { now: Time::ZERO, wall: Wall::EPOCH, limits: *limits };
        Measured {
            domain,
            env,
            out,
            owed,
            agents,
            reading: readings,
            waiting: waits,
            reaping: reaps,
            asked,
            commits: 0,
            meter,
            bound,
        }
    }

    fn step(&mut self, event: Event) {
        self.meter.start();
        step(&mut self.domain, &self.env, event, &mut self.out);
        self.drain();
    }

    fn fire(&mut self) {
        self.meter.start();
        fire(&mut self.domain, &self.env, &mut self.out);
        self.drain();
    }

    fn resume(&mut self) {
        self.meter.start();
        resume(&mut self.domain, &self.env, &mut self.out);
        self.drain();
    }

    /// Takes the requests, the payloads dropped: handed out. io's terminals
    /// are owed, but for an agent's reads, wait and reap, which wait for
    /// what the test has it do.
    fn drain(&mut self) {
        let measured = self.meter.end();
        while let Some(request) = self.out.pop() {
            match request {
                Request::Dial => self.asked.push(Asked::Dial),
                Request::Hello { hello } => {
                    let answered = hello.hosting.iter().filter(|hosted| hosted.phase == Phase::Answered).count();
                    self.asked.push(Asked::Hello { hosting: hello.hosting.len(), answered });
                }
                Request::Answer { run, attempt, answer: _ } => self.asked.push(Asked::Answer { run, attempt }),
                Request::Relay { .. } => self.asked.push(Asked::Relay),
                Request::Bounced { .. } => self.asked.push(Asked::Bounced),
                Request::Spawn { owner, .. } => {
                    self.agents.push(owner);
                    self.owed.push_back(Owed::Spawned { owner });
                }
                Request::Send { owner, .. } => self.owed.push_back(Owed::Sent { owner }),
                Request::Read { owner, .. } => self.reading.push(owner),
                Request::Signal { owner, .. } => self.owed.push_back(Owed::Signalled { owner }),
                Request::Wait { owner, .. } => self.waiting.push(owner),
                Request::Reap { owner, .. } => self.reaping.push(owner),
                Request::Io { owner, op, .. } => {
                    self.commits += 1;
                    let commit = Commit::new([self.commits; 32]);
                    let done = match op {
                        Op::Make { .. }
                        | Op::Clone { .. }
                        | Op::Create { .. }
                        | Op::CheckOut { .. }
                        | Op::Push { .. } => Done::Succeeded,
                        Op::Fetch { .. } => Done::Fetched { commit },
                        Op::Commit { .. } => Done::Committed { commit },
                    };
                    self.owed.push_back(Owed::Done { owner, done });
                }
                // Its operation's end is owed already: it lost the race.
                Request::CancelIo { .. } => {}
            }
        }
        self.meter.check(measured, self.bound, self.env.limits);
        // The iteration ends: the reclaim point.
        self.domain.reclaim();
    }

    /// Steps what io owes, and what that leads to, until it owes nothing.
    fn settle(&mut self) {
        while let Some(owed) = self.owed.pop_front() {
            let event = match owed {
                Owed::Done { owner, done } => Event::Done { owner, done },
                Owed::Spawned { owner } => Event::Spawned { owner, process: owner },
                Owed::Sent { owner } => Event::Sent { owner },
                Owed::Signalled { owner } => Event::Signalled { owner },
            };
            self.step(event);
        }
    }

    /// The agent `owner` says `message`, read up its channel.
    fn say(&mut self, owner: Token, message: Up) {
        let read = self.reading.iter().position(|reading| *reading == owner).expect("a read is in flight");
        self.reading.swap_remove(read);
        self.step(Event::Received { owner, message });
        self.settle();
    }

    /// The agent `owner` exits: its channel ends, its process exits, its tree
    /// empties.
    fn exit(&mut self, owner: Token) {
        if let Some(read) = self.reading.iter().position(|reading| *reading == owner) {
            self.reading.swap_remove(read);
            self.step(Event::Hangup { owner });
            self.settle();
        }
        let wait = self.waiting.iter().position(|waiting| *waiting == owner).expect("a wait is in flight");
        self.waiting.swap_remove(wait);
        self.step(Event::Exited { owner });
        self.settle();
        let reap = self.reaping.iter().position(|reaping| *reaping == owner).expect("a reap is in flight");
        self.reaping.swap_remove(reap);
        self.step(Event::Reaped { owner, detail: bytes(u64::from(self.env.limits.agent.detail_bytes) + 1) });
        self.settle();
    }

    fn take_asked(&mut self) -> Vec<Asked> {
        self.asked.drain(..).collect()
    }

    /// Time passes until the worker dials again, and the channel opens.
    fn reconnect(&mut self) {
        self.env.now = self.env.now.saturating_add(self.env.limits.redial_max);
        while self.domain.is_due(self.env.now) {
            self.fire();
            self.settle();
        }
        assert_eq!(self.take_asked(), [Asked::Dial], "it dials again");
        self.step(Event::Connected);
    }

    /// The engine acknowledges the answers among `asked`.
    fn acknowledge(&mut self, asked: &[Asked]) -> usize {
        let mut answers = 0;
        for asked in asked {
            match asked {
                Asked::Answer { run, attempt } => {
                    self.step(Event::Acknowledged { run: *run, attempt: *attempt });
                    answers += 1;
                }
                Asked::Dial | Asked::Hello { .. } | Asked::Relay | Asked::Bounced => {}
            }
        }
        answers
    }
}

/// An assignment for `run` of exactly the limits: names, a charter and a
/// snapshot of their limits, as many writable repositories as a workspace
/// may list, each starting from a branch, and a saved-work branch.
fn assignment(run: u64, limits: &Limits) -> Assignment {
    let host = &limits.host;
    let name = |byte: u8| vec![byte; usize::try_from(host.name_bytes).expect("fits")].into_boxed_slice();
    let mut repositories = Vec::new();
    for index in 0..host.repositories {
        let letter = b'a' + u8::try_from(index).expect("few repositories");
        repositories.push(Repository {
            name: name(letter),
            remote: name(b'r'),
            start: Start::Branch { branch: name(b'b') },
            access: Access::Writable { push: name(b'p') },
            identity: name(b'i'),
        });
    }
    let mut key = name(b'k');
    key[0] = u8::try_from(run).expect("few runs");
    Assignment {
        run: Token::new(run),
        attempt: Token::new(run + 1000),
        workspace: Workspace { key, repositories: repositories.into_boxed_slice() },
        save: Some(name(b's')),
        charter: bytes(host.charter_bytes),
        snapshot: Some(bytes(host.snapshot_bytes)),
    }
}

/// Connects, and admits a run of exactly the limits into every slot, each
/// with as many inbound events as it holds until it is live; then prepares
/// each, and its agent starts. Returns the agents, by run.
fn admit(worker: &mut Measured) -> Vec<Token> {
    let limits = worker.env.limits;
    let host = limits.host;
    worker.fire();
    assert_eq!(worker.take_asked(), [Asked::Dial], "it dials at once");
    worker.step(Event::Connected);
    assert_eq!(worker.take_asked(), [Asked::Hello { hosting: 0, answered: 0 }]);
    for run in 0..u64::from(host.slots) {
        worker.step(Event::Assign { assignment: assignment(run, &limits) });
        for _ in 0..host.held {
            let event = bytes(host.event_bytes);
            worker.step(Event::Inbound { run: Token::new(run), attempt: Token::new(run + 1000), event });
        }
    }
    let starting = host.charter_bytes + host.snapshot_bytes + u64::from(host.held) * host.event_bytes;
    let held = worker.meter.held();
    assert!(held >= u64::from(host.slots) * starting, "{limits:?}: every run holds what it may as it prepares");
    worker.settle();
    assert!(worker.take_asked().is_empty(), "every run admitted");
    assert_eq!(worker.agents.len(), usize::try_from(host.slots).expect("fits"), "every run's agent is live");
    worker.agents.clone()
}

/// Fills every slot with a run of exactly the limits, holding as many inbound
/// events as it may as it prepares; then, with the channel lost, has each run
/// make as many relayed calls as it may and tell more facts than the worker
/// keeps for the engine, then park with a snapshot of exactly the limit, its
/// agent exit and its work be saved, its answer kept; then the channel comes
/// back, and the engine acknowledges every answer.
///
/// The worst case adds up the child domains' own and the link's, and a payload
/// moves from one to the next (a charter from the host to the agent child
/// domain and down to the agent; a snapshot from the agent child domain to the
/// host and on to the link), so no moment holds them all: the fill reaches each
/// in turn, every slot's payloads in the host as the runs prepare, and in the
/// link while the channel is down.
fn fill(limits: &Limits) {
    let mut worker = Measured::new(limits);
    let agents = admit(&mut worker);
    let agent = limits.agent;
    worker.step(Event::Lost);
    for owner in &agents {
        for call in 1..=u64::from(limits.host.run_calls.min(agent.calls)) {
            let ask = Ask::Relay { body: bytes(agent.call_bytes) };
            worker.say(*owner, Up::Call { call: Token::new(call), ask });
        }
        for _ in 0..=limits.told {
            worker.say(*owner, Up::Fact { fact: bytes(agent.fact_bytes) });
        }
    }
    let kept = u64::from(limits.host.slots) * u64::from(limits.host.run_calls.min(agent.calls)) * agent.call_bytes
        + u64::from(limits.told) * agent.fact_bytes;
    assert!(worker.meter.held() >= kept, "{limits:?}: the relays and the run's facts are kept for the engine");
    for owner in &agents {
        worker.say(*owner, Up::Finish { finish: Finish::Parked { snapshot: Some(bytes(agent.snapshot_bytes)) } });
        worker.exit(*owner);
    }
    let answers = u64::from(limits.host.slots) * limits.host.snapshot_bytes;
    assert!(worker.meter.held() >= kept + answers, "{limits:?}: every answer is kept too");
    assert!(worker.take_asked().is_empty(), "nothing goes without a channel");
    assert_eq!(worker.domain.held(), limits.host.slots, "an answer for every slot");

    worker.reconnect();
    let asked = worker.take_asked();
    let slots = usize::try_from(limits.host.slots).expect("fits");
    assert_eq!(asked.first(), Some(&Asked::Hello { hosting: slots, answered: slots }), "{asked:?}");
    assert_eq!(worker.acknowledge(&asked), slots, "every answer follows the hello");
    assert_eq!(worker.domain.held(), 0, "every answer acknowledged");
    assert_eq!(worker.domain.host().hosted(), 0, "every slot came back");
}

/// Every other entry point, each measured: the grace passing with every run
/// live, which cancels them; agents stopped by the watchdog and by a cancel;
/// a prepare abandoned; an assignment refused; and a shutdown out of reach
/// past the grace, which gives the answers up.
fn paths(limits: &Limits) {
    let mut worker = Measured::new(limits);
    let agents = admit(&mut worker);
    worker.step(Event::Lost);
    worker.env.now = Time::ZERO.saturating_add(limits.grace);
    while worker.domain.is_due(worker.env.now) {
        worker.fire();
        worker.settle();
    }
    while worker.domain.is_ready() {
        worker.resume();
        worker.settle();
    }
    for owner in &agents {
        let failed = Finish::Failed { failure: RunFailure::Cancelled };
        worker.say(*owner, Up::Finish { finish: failed });
        worker.exit(*owner);
    }
    assert_eq!(worker.domain.held(), limits.host.slots, "an answer for every run cancelled");
    worker.step(Event::Shutdown);
    worker.settle();
    assert!(worker.domain.is_done(), "out of reach past the grace, a worker shutting down gives its answers up");

    // The watchdog, a cancel, an abandoned prepare and a refusal, on a worker
    // in reach.
    let mut worker = Measured::new(limits);
    let agents = admit(&mut worker);
    worker.env.now = Time::ZERO.saturating_add(limits.agent.no_progress).saturating_add(Duration::from_secs(1));
    while worker.domain.is_due(worker.env.now) {
        worker.fire();
        worker.settle();
    }
    for owner in &agents {
        worker.exit(*owner);
    }
    let asked = worker.take_asked();
    assert_eq!(worker.acknowledge(&asked), usize::try_from(limits.host.slots).expect("fits"), "every run answered");
    let mut beyond = assignment(5, limits);
    beyond.charter = bytes(limits.host.charter_bytes + 1);
    worker.step(Event::Assign { assignment: beyond });
    let [Asked::Answer { .. }] = worker.take_asked()[..] else {
        panic!("refused at once");
    };
    let next = assignment(9, limits);
    let (run, attempt) = (next.run, next.attempt);
    worker.step(Event::Assign { assignment: next });
    worker.step(Event::Cancel { run, attempt });
    worker.settle();
    let [Asked::Answer { .. }] = worker.take_asked()[..] else {
        panic!("answered once its prepare is abandoned");
    };
}

fn limits() -> Limits {
    Settings::calm(0).worker
}

#[test]
fn a_worker_with_every_slot_full_stays_within_its_worst_case() {
    let calm = limits();
    fill(&calm);
    fill(&Limits {
        host: temper_worker_domain::host::Limits { slots: 1, repositories: 1, run_calls: 1, held: 1, ..calm.host },
        agent: temper_worker_domain::agent::Limits { agents: 1, calls: 1, events: 1, ..calm.agent },
        checkout: temper_worker_domain::checkout::Limits { workspaces: 1, repositories: 1, ..calm.checkout },
        told: 1,
        stalled: 1,
        ..calm
    });
    let large = Limits {
        host: temper_worker_domain::host::Limits {
            charter_bytes: 65_536,
            snapshot_bytes: 16_384,
            outcome_bytes: 16_384,
            event_bytes: 4096,
            ..calm.host
        },
        agent: temper_worker_domain::agent::Limits {
            charter_bytes: 65_536,
            snapshot_bytes: 16_384,
            outcome_bytes: 16_384,
            event_bytes: 4096,
            call_bytes: 4096,
            fact_bytes: 1024,
            ..calm.agent
        },
        checkout: temper_worker_domain::checkout::Limits { message_bytes: 4096, ..calm.checkout },
        told: 64,
        ..calm
    };
    fill(&large);
}

#[test]
fn every_entry_point_stays_within_the_worst_case() {
    paths(&limits());
}
