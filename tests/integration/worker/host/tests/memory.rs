//! Memory stays within the worst case (programming-model.md, 6.3), measured by
//! a counting allocator: the host with every slot holding an assignment of
//! exactly its limits, then every run ending with as much as it may hold.

use temper_lib::{Env, Queue, ReplyTo, Time, Token};
use temper_worker_domain_host::{
    Access, AgentFailure, Answer, Ask, Assignment, Bounce, Domain, Event, Finish, Invalid, Landing, Limits,
    Preparation, Reason, Refusal, Repository, Request, Start, Workspace, max_out, resume, step, worst_case,
};
use temper_world::heap::{self, Meter};

#[global_allocator]
static HEAP: heap::Counting = heap::Counting;

const LIMITS: Limits = Limits {
    slots: 2,
    repositories: 2,
    name_bytes: 32,
    charter_bytes: 1024,
    snapshot_bytes: 512,
    outcome_bytes: 768,
    detail_bytes: 128,
    held: 2,
    event_bytes: 256,
    run_calls: 2,
    facts: 16,
};

fn bytes(len: u64) -> Box<[u8]> {
    vec![b'x'; usize::try_from(len).expect("a test length fits")].into_boxed_slice()
}

/// What a step asked for, without the payload.
#[derive(PartialEq, Eq, Debug)]
enum Asked {
    Prepare { owner: Token },
    Start,
    Push { owner: Token },
    Relay { call: Token },
    Save { owner: Token },
    Answer { answer: Answer },
    Bounced { bounce: Bounce },
    Hosting { runs: usize },
    Other,
}

/// The host under `limits`, measured: each step's peak is checked against
/// the worst case, less what it handed out in requests, which their
/// receivers count.
struct Measured {
    domain: Domain,
    env: Env<Limits>,
    out: Queue<Request>,
    meter: Meter,
    bound: u64,
}

impl Measured {
    fn new(limits: Limits) -> Measured {
        let bound = worst_case(&limits).expect("the test limits fit");
        let meter = Meter::new();
        let domain = Domain::new(&limits);
        let out = Queue::with_capacity(max_out(&limits));
        Measured { domain, env: Env { now: Time::ZERO, limits }, out, meter, bound }
    }

    fn step(&mut self, event: Event) -> Vec<Asked> {
        self.meter.start();
        step(&mut self.domain, &self.env, event, &mut self.out);
        self.drain()
    }

    fn resume(&mut self) -> Vec<Asked> {
        self.meter.start();
        resume(&mut self.domain, &self.env, &mut self.out);
        self.drain()
    }

    fn drain(&mut self) -> Vec<Asked> {
        let measured = self.meter.end();
        let mut asked = Vec::new();
        while let Some(request) = self.out.pop() {
            asked.push(match request {
                Request::Prepare { owner, .. } => Asked::Prepare { owner },
                Request::Start { .. } => Asked::Start,
                Request::Push { owner, .. } => Asked::Push { owner },
                Request::Relay { call, .. } => Asked::Relay { call },
                Request::Save { owner, .. } => Asked::Save { owner },
                Request::Answer { answer, .. } => Asked::Answer { answer },
                Request::Bounced { bounce, .. } => Asked::Bounced { bounce },
                Request::Hosting { runs } => Asked::Hosting { runs: runs.len() },
                Request::Abort { .. }
                | Request::Deliver { .. }
                | Request::Reply { .. }
                | Request::Stop { .. }
                | Request::Release { .. } => Asked::Other,
            });
        }
        self.meter.check(measured, self.bound, self.env.limits);
        // The iteration ends: the reclaim point.
        self.domain.reclaim();
        asked
    }
}

/// An assignment for `run` of exactly the limits: a charter, a snapshot and
/// names of their limits, as many repositories as a workspace may list.
fn assignment(run: u64, limits: &Limits) -> Assignment {
    let name = |byte: u8| vec![byte; usize::try_from(limits.name_bytes).expect("fits")].into_boxed_slice();
    let mut repositories = Vec::new();
    for index in 0..limits.repositories {
        let letter = b'a' + u8::try_from(index).expect("few repositories");
        repositories.push(Repository {
            name: name(letter),
            remote: name(b'r'),
            start: Start::Branch { branch: name(b'b') },
            access: Access::Writable { push: name(b'p') },
            identity: name(b'i'),
        });
    }
    Assignment {
        run: Token::new(run),
        attempt: Token::new(run + 1000),
        workspace: Workspace { key: name(b'k'), repositories: repositories.into_boxed_slice() },
        save: Some(name(b's')),
        charter: bytes(limits.charter_bytes),
        snapshot: Some(bytes(limits.snapshot_bytes)),
    }
}

/// Fills every slot with an assignment of exactly the limits and a full hold
/// of inbound events of exactly their limit; starts each run, has it land a
/// change in every repository and make as many calls as it may, the last a
/// push still in flight as it ends with an outcome of exactly the limit; and
/// takes each through its tail. Every step's peak is checked against the
/// worst case.
fn fill(limits: Limits) {
    let mut host = Measured::new(limits);
    let landing = Landing::Landed { commit: [7; 32] };
    let landed = vec![landing; usize::try_from(limits.repositories).expect("fits")].into_boxed_slice();
    let mut owners = Vec::new();
    for run in 0..u64::from(limits.slots) {
        let [Asked::Prepare { owner }] = host
            .step(Event::Assign { reply_to: ReplyTo::new(Token::new(run)), assignment: assignment(run, &limits) })[..]
        else {
            panic!("an assignment of exactly the limits is admitted");
        };
        for _ in 0..limits.held {
            let event = Event::Inbound {
                run: Token::new(run),
                attempt: Token::new(run + 1000),
                event: bytes(limits.event_bytes),
            };
            assert!(host.step(event).is_empty(), "held");
        }
        owners.push(owner);
    }
    let held = host.meter.held();
    let starting = limits.charter_bytes + limits.snapshot_bytes + u64::from(limits.held) * limits.event_bytes;
    assert!(held >= u64::from(limits.slots) * starting, "{limits:?}: every slot holds what it may as it prepares");

    let mut pending = Vec::new();
    for (index, owner) in owners.iter().enumerate() {
        let index = u64::try_from(index).expect("fits");
        let workspace = Token::new(index);
        assert_eq!(host.step(Event::Prepared { owner: *owner, workspace }), [Asked::Start]);
        let agent = Token::new(index);
        assert_eq!(
            host.step(Event::Started { owner: *owner, agent }).len(),
            usize::try_from(limits.held).expect("fits")
        );
        assert!(host.step(Event::Yielded { owner: *owner }).is_empty(), "waiting");
        let [Asked::Push { owner: push }] =
            host.step(Event::Called { owner: *owner, call: Token::new(1), ask: Ask::Push { message: bytes(64) } })[..]
        else {
            panic!("pushed");
        };
        assert_eq!(host.step(Event::Pushed { owner: push, push: landed.clone() }), [Asked::Other], "landed everywhere");
        let mut calls = Vec::new();
        for call in 2..=u64::from(limits.run_calls) {
            let ask = Ask::Relay { body: bytes(64) };
            let [Asked::Relay { call }] = host.step(Event::Called { owner: *owner, call: Token::new(call), ask })[..]
            else {
                panic!("relayed");
            };
            calls.push(call);
        }
        // The first relayed call is answered, and a push takes its place.
        if let Some(call) = calls.first() {
            let relayed = Event::Relayed {
                run: Token::new(index),
                attempt: Token::new(index + 1000),
                call: *call,
                answer: bytes(64),
            };
            assert_eq!(host.step(relayed), [Asked::Other]);
        }
        let push = if limits.run_calls > 1 {
            let ask = Ask::Push { message: bytes(64) };
            let [Asked::Push { owner: push }] =
                host.step(Event::Called { owner: *owner, call: Token::new(99), ask })[..]
            else {
                panic!("pushed again");
            };
            Some(push)
        } else {
            None
        };
        let finish = Finish::Ended { outcome: bytes(limits.outcome_bytes) };
        assert!(!host.step(Event::Finished { owner: *owner, finish }).is_empty(), "stopped");
        assert!(host.step(Event::Faulted { owner: *owner, fault: AgentFailure::WallTime }).is_empty(), "decided");
        let detail = bytes(u64::from(limits.detail_bytes) + 1);
        let gone = host.step(Event::Gone { owner: *owner, detail });
        if let Some(push) = push {
            assert!(gone.is_empty(), "the push in flight is waited for");
            pending.push(push);
        }
    }
    if limits.run_calls > 1 {
        let held = host.meter.held();
        assert!(held >= u64::from(limits.slots) * limits.outcome_bytes, "{limits:?}: every run holds its outcome");
    }
    for push in pending {
        owners_push(&mut host, push, &landed);
    }
    assert_eq!(host.domain.hosted(), 0, "every slot came back");

    // Beyond the limits: refused, and nothing held.
    let mut beyond = assignment(0, &limits);
    beyond.charter = bytes(limits.charter_bytes + 1);
    let refused = host.step(Event::Assign { reply_to: ReplyTo::new(Token::new(0)), assignment: beyond });
    assert_eq!(refused, [Asked::Answer { answer: Answer::Refused(Refusal::Invalid(Invalid::Charter)) }]);
    let [Asked::Prepare { owner: _ }] =
        host.step(Event::Assign { reply_to: ReplyTo::new(Token::new(0)), assignment: assignment(0, &limits) })[..]
    else {
        panic!("admitted");
    };
    let large = Event::Inbound { run: Token::new(0), attempt: Token::new(1000), event: bytes(limits.event_bytes + 1) };
    assert_eq!(host.step(large), [Asked::Bounced { bounce: Bounce::TooLarge }]);
}

/// A push still in flight as its run ended settles, and the run answers.
fn owners_push(host: &mut Measured, push: Token, landed: &[Landing]) {
    let settled = host.step(Event::Pushed { owner: push, push: landed.into() });
    let [Asked::Other, Asked::Other, Asked::Answer { answer: Answer::Ended { .. } }] = &settled[..] else {
        panic!("told how it went, released and answered: {settled:?}");
    };
}

/// Every other entry point, each measured: a report, cancels of every run and
/// of one, a prepare that fails, a save, and a fault.
fn paths(limits: Limits) {
    let mut host = Measured::new(limits);
    let mut owners = Vec::new();
    for run in 0..u64::from(limits.slots) {
        let [Asked::Prepare { owner }] = host
            .step(Event::Assign { reply_to: ReplyTo::new(Token::new(run)), assignment: assignment(run, &limits) })[..]
        else {
            panic!("admitted");
        };
        owners.push(owner);
    }
    let reported = host.step(Event::Report);
    assert_eq!(reported, [Asked::Hosting { runs: usize::try_from(limits.slots).expect("fits") }]);
    let first = owners[0];
    let unprepared = Event::Unprepared {
        owner: first,
        failure: Preparation::Refused { repository: 0 },
        detail: bytes(u64::from(limits.detail_bytes) + 1),
    };
    assert_eq!(host.step(unprepared).len(), 1, "answered");
    for owner in owners.iter().skip(1) {
        assert_eq!(host.step(Event::Prepared { owner: *owner, workspace: *owner }), [Asked::Start]);
        assert!(host.step(Event::Started { owner: *owner, agent: *owner }).is_empty());
    }
    assert!(host.step(Event::CancelAll { reason: Reason::Contact }).is_empty());
    while host.domain.is_ready() {
        host.resume();
    }
    for (index, owner) in owners.iter().enumerate().skip(1) {
        let index = u64::try_from(index).expect("fits");
        let cancel = Event::Cancel { run: Token::new(index), attempt: Token::new(index + 1000) };
        assert!(host.step(cancel).is_empty(), "decided");
        let [Asked::Save { owner: saving }] = host.step(Event::Gone { owner: *owner, detail: bytes(10) })[..] else {
            panic!("saved");
        };
        let save = vec![Landing::Unchanged; usize::try_from(limits.repositories).expect("fits")].into_boxed_slice();
        let saved = host.step(Event::Saved { owner: saving, save });
        assert_eq!(saved.len(), 2, "released and answered");
    }
    // A fault, from a run that had started.
    let [Asked::Prepare { owner }] =
        host.step(Event::Assign { reply_to: ReplyTo::new(Token::new(0)), assignment: assignment(0, &limits) })[..]
    else {
        panic!("admitted");
    };
    assert_eq!(host.step(Event::Prepared { owner, workspace: owner }), [Asked::Start]);
    assert!(host.step(Event::Started { owner, agent: owner }).is_empty());
    assert_eq!(host.step(Event::Faulted { owner, fault: AgentFailure::NoProgress }).len(), 1, "stopped");
    assert_eq!(host.step(Event::Gone { owner, detail: bytes(10) }).len(), 1, "saved");
}

#[test]
fn a_host_with_every_slot_full_stays_within_its_worst_case() {
    fill(LIMITS);
    fill(Limits { slots: 16, repositories: 8, held: 8, run_calls: 4, ..LIMITS });
    fill(Limits { slots: 200, charter_bytes: 65_536, snapshot_bytes: 16_384, event_bytes: 4096, ..LIMITS });
    fill(Limits { run_calls: 1, held: 0, ..LIMITS });
    // The outcome dominates what a run holds: the ending's side of the max.
    fill(Limits { charter_bytes: 16, snapshot_bytes: 16, held: 0, outcome_bytes: 4096, run_calls: 4, ..LIMITS });
}

#[test]
fn every_entry_point_stays_within_the_worst_case() {
    paths(LIMITS);
    paths(Limits { slots: 8, repositories: 4, ..LIMITS });
}
