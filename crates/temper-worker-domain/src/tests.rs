//! Feed the domain events, inspect the requests that come out. What each
//! child domain does alone is its own tests' business: these follow the paths
//! that cross them, and the engine link.

use alloc::boxed::Box;

use skein_lib::{Duration, Env, List, Queue, Time, Token, Wall};
use temper_worker_domain_agent::channel::{self, Down, Up};
use temper_worker_domain_checkout::git::{self, Commit, Op};

use crate::{
    Domain, Event, Fact, Hello, Hosted, Limits, Phase, Request, Told, agent, checkout, fire, host, max_out, resume,
    step, wire, worst_case,
};

const LIMITS: Limits = Limits {
    host: host::Limits {
        accounts: 4,
        slots: 2,
        charter_bytes: 64,
        snapshot_bytes: 32,
        transcript_bytes: 0,
        turn_bytes: 0,
        outcome_bytes: 32,
        detail_bytes: 8,
        held: 2,
        event_bytes: 16,
        run_calls: 2,
        facts: 64,
        told: 2,
        fact_bytes: 16,
        turns: 0,
        turn_queue_bytes: 0,
    },
    checkout: checkout::Limits {
        workspaces: 2,
        repositories: 2,
        name_bytes: 16,
        message_bytes: 64,
        conflicts: 0,
        path_bytes: 0,
        remote_timeout: Duration::from_secs(60),
        local_timeout: Duration::from_secs(10),
        facts: 64,
    },
    agent: agent::Limits {
        accounts: 4,
        repositories: 8,
        name_bytes: 256,
        agents: 2,
        charter_bytes: 64,
        snapshot_bytes: 32,
        transcript_bytes: 0,
        turn_bytes: 0,
        conflicts: 0,
        path_bytes: 0,
        event_bytes: 16,
        events: 4,
        calls: 2,
        call_bytes: 32,
        answer_bytes: 64,
        fact_bytes: 16,
        outcome_bytes: 32,
        detail_bytes: 8,
        spawn_timeout: Duration::from_secs(10),
        no_progress: Duration::from_secs(600),
        long_span: Duration::from_secs(3600),
        wall_time: Duration::from_secs(7200),
        grace: Duration::from_secs(10),
        kill_after: Duration::from_secs(5),
        facts: 64,
    },
    grace: Duration::from_secs(30),
    redial: Duration::from_secs(1),
    redial_max: Duration::from_secs(8),
    stalled: 4,
    turn_backoff: Duration::from_secs(1),
};

/// What io's commits commit, when there is a change.
const COMMITTED: [u8; 32] = [2; 32];

/// The most a test lets io and the run go back and forth.
const ROUNDS: u32 = 64;

/// The domain, its environment, and room for one step's output.
struct Harness {
    domain: Domain,
    env: Env<Limits>,
    out: Queue<Request>,
}

/// A run as a test drives it: the engine's names for it, and the tokens of
/// its agent and its agent's process.
#[derive(Clone, Copy, Debug)]
struct Names {
    run: Token,
    attempt: Token,
    agent: Token,
    process: Token,
}

impl Harness {
    fn new(limits: &Limits) -> Harness {
        assert!(worst_case(limits).is_some(), "the test's limits are honoured");
        let out = Queue::with_capacity(max_out(limits));
        Harness {
            domain: Domain::new(limits, 7),
            env: Env { now: Time::ZERO, wall: Wall::EPOCH, limits: *limits },
            out,
        }
    }

    /// Steps `event`, returning what it emitted, oldest first.
    fn step(&mut self, event: Event) -> Box<[Request]> {
        step(&mut self.domain, &self.env, event, &mut self.out);
        self.drain()
    }

    fn fire(&mut self) -> Box<[Request]> {
        assert!(self.domain.is_due(self.env.now), "an alarm is due");
        fire(&mut self.domain, &self.env, &mut self.out);
        self.drain()
    }

    fn resume(&mut self) -> Box<[Request]> {
        assert!(self.domain.is_ready(), "a run is ready");
        resume(&mut self.domain, &self.env, &mut self.out);
        self.drain()
    }

    fn drain(&mut self) -> Box<[Request]> {
        let most = max_out(&self.env.limits);
        let mut requests = List::with_capacity(most);
        for _ in 0..most {
            let Some(request) = self.out.pop() else { break };
            requests.push(request).expect("room for max_out");
        }
        assert!(self.out.is_empty(), "a step emits at most max_out");
        requests.into_boxed()
    }

    /// Moves the clock to `secs` from the start.
    fn at(&mut self, secs: u64) {
        self.env.now = Time::ZERO.saturating_add(Duration::from_secs(secs));
    }

    /// Dials the engine, which answers: the channel is open. What it emitted
    /// on opening, the hello first.
    fn connect(&mut self) -> Box<[Request]> {
        assert_eq!(&*self.fire(), [Request::Dial]);
        let emitted = self.step(Event::Connected);
        assert!(matches_hello(emitted.first()), "the hello goes first: {emitted:?}");
        emitted
    }

    /// The engine assigns run `run`: its workspace is prepared, by io that
    /// succeeds, and its agent asked for. The spawn's owner.
    fn assign(&mut self, run: u64) -> Token {
        let emitted = self.step(Event::Assign { assignment: assignment(run) });
        let emitted = self.git(emitted, true);
        let [Request::Spawn { owner, workspace: _, deadline }] = &*emitted else {
            panic!("expected a spawn, got {emitted:?}");
        };
        assert_eq!(*deadline, self.env.now.saturating_add(LIMITS.agent.spawn_timeout));
        *owner
    }

    /// The engine assigns run `run`, and its agent starts: the run is live.
    fn live(&mut self, run: u64) -> Names {
        let agent = self.assign(run);
        let process = Token::new(run.saturating_add(500));
        let emitted = self.step(Event::Spawned { owner: agent, process });
        let r = Names { run: Token::new(run), attempt: attempt(run), agent, process };
        let start = Down::Start {
            repositories: Box::new([channel::Repository { name: bytes(b"app"), writable: true }]),
            grants: Box::new([]),
            charter: bytes(b"charter"),
            snapshot: None,
        };
        let expected = [
            Request::Wait { owner: agent, process },
            Request::Reap { owner: agent, process },
            Request::Send { owner: agent, process, message: start },
            read(r),
        ];
        assert_eq!(&*emitted, expected);
        assert!(self.step(Event::Sent { owner: agent }).is_empty(), "nothing waits to go down");
        r
    }

    fn say(&mut self, r: Names, message: Up) -> Box<[Request]> {
        self.step(Event::Received { owner: r.agent, message })
    }

    /// The run's agent exits and its tree empties: what that leads to.
    fn goes(&mut self, r: Names) -> Box<[Request]> {
        assert!(self.step(Event::Exited { owner: r.agent }).is_empty(), "the channel is still read");
        assert!(self.step(Event::Hangup { owner: r.agent }).is_empty(), "the tree is not empty yet");
        self.step(Event::Reaped { owner: r.agent, detail: bytes(b"bye") })
    }

    /// Answers the git and file operations among `requests` as io would, each
    /// succeeding (a commit committing a change if `changed`), and those that
    /// follow from them, until none is left: what else came out, in order.
    fn git(&mut self, requests: Box<[Request]>, changed: bool) -> Box<[Request]> {
        let mut pending = Queue::with_capacity(ROUNDS);
        let mut others = List::with_capacity(ROUNDS);
        sort(requests, changed, &mut pending, &mut others);
        for _ in 0..ROUNDS {
            let Some((owner, done)) = pending.pop() else {
                return others.into_boxed();
            };
            let emitted = self.step(Event::Done { owner, done });
            sort(emitted, changed, &mut pending, &mut others);
        }
        panic!("io settles within {ROUNDS} rounds");
    }

    /// The run's push call `call`, served by io that pushes it: what came out.
    fn push(&mut self, r: Names, call: u64) -> Box<[Request]> {
        let ask = channel::Ask::Push { message: bytes(b"change") };
        let emitted = self.say(r, Up::Call { call: Token::new(call), ask });
        self.git(emitted, true)
    }

    fn facts(&mut self) -> Box<[Fact]> {
        let mut facts = List::with_capacity(256);
        for _ in 0..256_u32 {
            let Some(fact) = self.domain.pop_fact() else { break };
            facts.push(fact).expect("room for the facts");
        }
        facts.into_boxed()
    }
}

/// Splits `requests` into git and file operations, answered as io would, and
/// the rest.
fn sort(requests: Box<[Request]>, changed: bool, pending: &mut Queue<(Token, git::Done)>, others: &mut List<Request>) {
    for request in requests {
        match request {
            Request::Io { owner, op, deadline: _ } => pending.push((owner, done(&op, changed))),
            other @ (Request::HelloV2 { .. }
            | Request::Turn { .. }
            | Request::AnswerV2 { .. }
            | Request::RelayV2 { .. }
            | Request::RelayTyped { .. }
            | Request::Dial
            | Request::Hello { .. }
            | Request::Answer { .. }
            | Request::Relay { .. }
            | Request::Bounced { .. }
            | Request::Rejected { .. }
            | Request::Exhausted { .. }
            | Request::Spawn { .. }
            | Request::Send { .. }
            | Request::Read { .. }
            | Request::Signal { .. }
            | Request::Wait { .. }
            | Request::Reap { .. }
            | Request::CancelRelay { .. }
            | Request::CancelIo { .. }) => others.push(other).expect("room for what a test makes"),
        }
    }
}

/// What io says of `op`, done as asked.
fn done(op: &Op, changed: bool) -> git::Done {
    match op {
        Op::Make { .. } | Op::Clone { .. } | Op::Create { .. } | Op::CheckOut { .. } | Op::Push { .. } => {
            git::Done::Succeeded
        }
        Op::Fetch { .. } => git::Done::Fetched { commit: Commit::new([1; 32]) },
        Op::Commit { .. } if changed => git::Done::Committed { commit: Commit::new(COMMITTED) },
        Op::Commit { .. } => git::Done::Unchanged,
        Op::Merge { .. } => unreachable!("legacy assignments do not request merges"),
    }
}

fn matches_hello(request: Option<&Request>) -> bool {
    match request {
        Some(Request::Hello { .. }) => true,
        Some(_) | None => false,
    }
}

fn bytes(text: &[u8]) -> Box<[u8]> {
    Box::from(text)
}

fn attempt(run: u64) -> Token {
    Token::new(run.saturating_add(1000))
}

/// The test assignment for `run`: one writable repository, its unfinished
/// work saved, no snapshot.
fn assignment(run: u64) -> wire::Assignment {
    let repository = wire::Repository {
        tag: 0,
        name: bytes(b"app"),
        remote: bytes(b"org/app"),
        start: wire::Start::Base { branch: bytes(b"main") },
        access: wire::Access::Writable { push: bytes(b"fix") },
        identity: 0,
    };
    wire::Assignment {
        grants: Box::new([]),
        run: Token::new(run),
        attempt: attempt(run),
        workspace: wire::Workspace { key: key(run), repositories: Box::new([repository]) },
        save: Some(bytes(b"saved")),
        charter: bytes(b"charter"),
        snapshot: None,
    }
}

/// The workstream of run `run`: one of its own.
fn key(run: u64) -> Box<[u8]> {
    Box::new(run.to_be_bytes())
}

fn inbound(r: Names, event: &[u8]) -> Event {
    Event::Inbound { name: Token::new(1), run: r.run, attempt: r.attempt, event: bytes(event) }
}

fn read(r: Names) -> Request {
    Request::Read { owner: r.agent, process: r.process }
}

fn send(r: Names, message: Down) -> Request {
    Request::Send { owner: r.agent, process: r.process, message }
}

fn answer(r: Names, answer: wire::Answer) -> Request {
    Request::Answer { run: r.run, attempt: r.attempt, answer }
}

fn work(landed: &[wire::Landed], saved: Option<Box<[wire::Landing]>>) -> wire::Work {
    wire::Work { landed: Box::from(landed), saved }
}

fn cancelled(reason: wire::Reason, work: wire::Work) -> wire::Answer {
    wire::Answer::Failed { failure: wire::Failure::Cancelled(reason), detail: bytes(b""), work }
}

// A run, across the three child domains.

#[test]
fn a_run_goes_from_its_assignment_to_its_answer_through_all_three_child_domains() {
    let mut h = Harness::new(&LIMITS);
    h.connect();
    let r = h.live(1);
    let emitted = h.push(r, 7);
    let told = channel::Reply::Pushed(channel::Push::Done);
    assert_eq!(&*emitted, [read(r), send(r, Down::Answer { call: Token::new(7), reply: told })], "landed");
    assert!(h.step(Event::Sent { owner: r.agent }).is_empty());
    let finish = channel::Finish::Ended { outcome: bytes(b"done") };
    let emitted = h.say(r, Up::Finish { finish });
    assert_eq!(&*emitted, [read(r)], "the stop changes nothing: it exits next");
    let emitted = h.goes(r);
    let ended = wire::Answer::Ended {
        outcome: bytes(b"done"),
        work: work(&[wire::Landed { tag: 0, commit: COMMITTED }], None),
    };
    assert_eq!(&*emitted, [answer(r, ended)], "it ended with its change landed: nothing to save; released");
    h.domain.reclaim();
    assert_eq!(h.domain.workspaces(), 0, "the workspace is back in the cache");
    assert_eq!(h.domain.host().hosted(), 0);
    assert_eq!(h.domain.agent().agents(), 0);
    assert_eq!(h.domain.checkout().idle(), 1);
    let (mut host, mut checkout, mut agent) = (0_u32, 0_u32, 0_u32);
    for fact in h.facts() {
        match fact {
            Fact::Host { .. } => host += 1,
            Fact::Checkout { .. } => checkout += 1,
            Fact::Agent { .. } => agent += 1,
            Fact::Connected | Fact::Lost | Fact::Grace => {}
        }
    }
    assert!(host > 0 && checkout > 0 && agent > 0, "the sub-models' facts are gathered");
    assert_eq!(h.domain.facts_lost(), 0);
}

#[test]
fn a_run_without_items_starts_without_preparing_or_saving_a_workspace() {
    let mut h = Harness::new(&LIMITS);
    h.connect();
    let mut assignment = assignment(1);
    assignment.workspace.repositories = Box::new([]);
    assignment.workspace.key = Box::new([]);
    let emitted = h.step(Event::Assign { assignment });
    let [Request::Spawn { owner, workspace: None, .. }] = &*emitted else {
        panic!("an itemless run spawns directly: {emitted:?}");
    };
    let r = Names { run: Token::new(1), attempt: attempt(1), agent: *owner, process: Token::new(501) };
    let emitted = h.step(Event::Spawned { owner: r.agent, process: r.process });
    assert_eq!(
        &*emitted,
        [
            Request::Wait { owner: r.agent, process: r.process },
            Request::Reap { owner: r.agent, process: r.process },
            send(
                r,
                Down::Start {
                    repositories: Box::new([]),
                    grants: Box::new([]),
                    charter: bytes(b"charter"),
                    snapshot: None,
                },
            ),
            read(r),
        ]
    );
    assert!(h.step(Event::Sent { owner: r.agent }).is_empty());
    assert_eq!(&*h.say(r, Up::Finish { finish: channel::Finish::Ended { outcome: bytes(b"done") } }), [read(r)]);
    assert_eq!(&*h.goes(r), [answer(r, wire::Answer::Ended { outcome: bytes(b"done"), work: work(&[], None) })]);
    assert_eq!(h.domain.checkout().workspaces(), 0);
}

#[test]
fn another_attempt_for_a_hosted_run_is_refused_before_staging_its_items() {
    let mut h = Harness::new(&LIMITS);
    h.connect();
    let r = h.live(1);
    let mut assigned = assignment(1);
    assigned.attempt = Token::new(2001);
    assert_eq!(
        &*h.step(Event::Assign { assignment: assigned }),
        [Request::Answer { run: r.run, attempt: Token::new(2001), answer: wire::Answer::Refused(wire::Refusal::Busy) }]
    );
    assert_eq!(h.domain.workspaces(), 1);
}

#[test]
fn a_run_cancelled_as_its_workspace_is_prepared_aborts_the_prepare() {
    let mut h = Harness::new(&LIMITS);
    h.connect();
    let emitted = h.step(Event::Assign { assignment: assignment(1) });
    let [Request::Io { owner, op: Op::Make { .. }, deadline: _ }] = &*emitted else {
        panic!("expected the workspace made, got {emitted:?}");
    };
    let owner = *owner;
    let emitted = h.step(Event::Cancel { run: Token::new(1), attempt: attempt(1) });
    assert_eq!(&*emitted, [Request::CancelIo { owner }], "the prepare is abandoned, not waited out");
    let done = git::Done::Failed { fault: git::Fault::Cancelled };
    let emitted = h.step(Event::Done { owner, done });
    let r = Names { run: Token::new(1), attempt: attempt(1), agent: owner, process: owner };
    assert_eq!(&*emitted, [answer(r, cancelled(wire::Reason::Engine, work(&[], None)))]);
    h.domain.reclaim();
    assert_eq!(h.domain.workspaces(), 0, "the hold went with the prepare");
    assert_eq!(h.domain.checkout().holds(), 0);
}

#[test]
fn a_run_cancelled_as_its_agent_starts_is_stopped_once_started_and_its_work_saved() {
    let mut h = Harness::new(&LIMITS);
    h.connect();
    let agent = h.assign(1);
    assert!(h.step(Event::Cancel { run: Token::new(1), attempt: attempt(1) }).is_empty(), "the spawn is waited for");
    let process = Token::new(501);
    let r = Names { run: Token::new(1), attempt: attempt(1), agent, process };
    let emitted = h.step(Event::Spawned { owner: agent, process });
    let start = Down::Start {
        repositories: Box::new([channel::Repository { name: bytes(b"app"), writable: true }]),
        grants: Box::new([]),
        charter: bytes(b"charter"),
        snapshot: None,
    };
    let expected =
        [Request::Wait { owner: agent, process }, Request::Reap { owner: agent, process }, send(r, start), read(r)];
    assert_eq!(&*emitted, expected);
    assert_eq!(&*h.step(Event::Sent { owner: agent }), [send(r, Down::Cancel)], "stopped once started");
    assert!(h.step(Event::Sent { owner: agent }).is_empty());
    let finish = channel::Finish::Failed { failure: channel::RunFailure::Cancelled };
    assert_eq!(&*h.say(r, Up::Finish { finish }), [read(r)]);
    let emitted = h.goes(r);
    let emitted = h.git(emitted, false);
    let saved = work(&[], Some(Box::new([wire::Landing::Unchanged])));
    let answered = wire::Answer::Failed {
        failure: wire::Failure::Cancelled(wire::Reason::Engine),
        detail: bytes(b"bye"),
        work: saved,
    };
    assert_eq!(&*emitted, [answer(r, answered)], "saved, released and answered as the worker's cancel");
}

#[test]
fn a_live_run_cancelled_winds_down_and_its_own_ending_is_the_answer() {
    let mut h = Harness::new(&LIMITS);
    h.connect();
    let r = h.live(1);
    let emitted = h.step(Event::Cancel { run: r.run, attempt: r.attempt });
    assert_eq!(&*emitted, [send(r, Down::Cancel)]);
    assert!(h.step(Event::Sent { owner: r.agent }).is_empty());
    let finish = channel::Finish::Parked { snapshot: Some(bytes(b"snap")) };
    assert_eq!(&*h.say(r, Up::Finish { finish }), [read(r)]);
    let emitted = h.goes(r);
    let emitted = h.git(emitted, true);
    let parked = wire::Answer::Parked {
        snapshot: Some(bytes(b"snap")),
        work: work(&[], Some(Box::new([wire::Landing::Landed { commit: COMMITTED }]))),
    };
    assert_eq!(&*emitted, [answer(r, parked)], "its unfinished work saved");
}

#[test]
fn an_agent_that_exits_without_a_word_is_faulted_and_gone_in_one_step() {
    let mut h = Harness::new(&LIMITS);
    h.connect();
    let r = h.live(1);
    assert!(h.step(Event::Exited { owner: r.agent }).is_empty(), "what it wrote is still read");
    assert!(h.step(Event::Reaped { owner: r.agent, detail: bytes(b"bye") }).is_empty());
    // The agent child domain tells the host the agent failed, then that it has
    // gone: the host stops it (a stale name by then), saves its work, then
    // answers.
    let emitted = h.step(Event::Hangup { owner: r.agent });
    let [Request::Io { op: Op::Commit { .. }, .. }] = &*emitted else {
        panic!("expected the save's commit, got {emitted:?}");
    };
    let emitted = h.git(emitted, false);
    let failed = wire::Answer::Failed {
        failure: wire::Failure::Agent(wire::AgentFailure::Exited),
        detail: bytes(b"bye"),
        work: work(&[], Some(Box::new([wire::Landing::Unchanged]))),
    };
    assert_eq!(&*emitted, [answer(r, failed)]);
}

#[test]
fn a_workspace_that_cannot_be_prepared_is_released_and_the_run_fails() {
    let mut h = Harness::new(&LIMITS);
    h.connect();
    let emitted = h.step(Event::Assign { assignment: assignment(1) });
    let [Request::Io { owner, .. }] = &*emitted else {
        panic!("expected the workspace made, got {emitted:?}");
    };
    let owner = *owner;
    let emitted = h.step(Event::Done { owner, done: git::Done::Succeeded });
    let [Request::Io { op: Op::Clone { .. }, .. }] = &*emitted else {
        panic!("expected a clone, got {emitted:?}");
    };
    let missing = git::Fault::Missing { missing: git::Missing::Repository };
    let emitted = h.step(Event::Done { owner, done: git::Done::Failed { fault: missing } });
    let r = Names { run: Token::new(1), attempt: attempt(1), agent: owner, process: owner };
    let missing = wire::Preparation::Missing { repository: 0, missing: wire::Missing::Repository };
    let failure = wire::Failure::Unprepared(missing);
    let failed = wire::Answer::Failed { failure, detail: bytes(b""), work: work(&[], None) };
    assert_eq!(&*emitted, [answer(r, failed)]);
    h.domain.reclaim();
    assert_eq!(h.domain.workspaces(), 0, "released by the top level");
    assert_eq!(h.domain.checkout().holds(), 0);
}

#[test]
fn a_workstream_held_by_another_run_fails_the_prepare_for_now() {
    let mut h = Harness::new(&LIMITS);
    h.connect();
    h.live(1);
    let mut second = assignment(2);
    second.workspace.key = key(1);
    let emitted = h.step(Event::Assign { assignment: second });
    let r = Names { run: Token::new(2), attempt: attempt(2), agent: Token::new(0), process: Token::new(0) };
    let failure = wire::Failure::Unprepared(wire::Preparation::Transient);
    let failed = wire::Answer::Failed { failure, detail: bytes(b""), work: work(&[], None) };
    assert_eq!(&*emitted, [answer(r, failed)], "a retry may find it free");
    h.domain.reclaim();
    assert_eq!(h.domain.workspaces(), 1);
}

// Host calls.

#[test]
fn a_relay_goes_to_the_engine_and_its_answer_back_to_the_run() {
    let mut h = Harness::new(&LIMITS);
    h.connect();
    let r = h.live(1);
    let ask = channel::Ask::Relay { body: bytes(b"read") };
    let emitted = h.say(r, Up::Call { call: Token::new(7), ask });
    let [first, Request::Relay { run, attempt, call, body }] = &*emitted else {
        panic!("expected a relay, got {emitted:?}");
    };
    assert_eq!((*first == read(r), *run, *attempt, &**body), (true, r.run, r.attempt, &b"read"[..]));
    let relayed = Event::Relayed { run: r.run, attempt: r.attempt, call: *call, answer: bytes(b"page") };
    let reply = channel::Reply::Relayed { answer: bytes(b"page") };
    assert_eq!(&*h.step(relayed), [send(r, Down::Answer { call: Token::new(7), reply })]);
}

#[test]
fn a_withdrawn_relay_is_answered_at_once_and_the_engines_late_answer_dropped() {
    let mut h = Harness::new(&LIMITS);
    h.connect();
    let r = h.live(1);
    let ask = channel::Ask::Relay { body: bytes(b"read") };
    let emitted = h.say(r, Up::Call { call: Token::new(7), ask });
    let [_, Request::Relay { call, .. }] = &*emitted else {
        panic!("expected a relay, got {emitted:?}");
    };
    let call = *call;
    let emitted = h.say(r, Up::Withdraw { call: Token::new(7) });
    let withdrawn = Down::Answer { call: Token::new(7), reply: channel::Reply::Withdrawn };
    assert_eq!(&*emitted, [read(r), send(r, withdrawn), Request::CancelRelay { call }]);
    h.domain.reclaim();
    assert_eq!(h.domain.host().calls(), 1, "the wire delivery is still settling");
    assert!(h.step(Event::RelayCancelled { call }).is_empty());
    h.domain.reclaim();
    assert_eq!(h.domain.host().calls(), 0);
    let late = Event::Relayed { run: r.run, attempt: r.attempt, call, answer: bytes(b"page") };
    assert!(h.step(late).is_empty(), "the engine's answer to a withdrawn call is dropped");
}

#[test]
fn a_withdrawn_push_goes_on_and_is_answered_with_how_it_went() {
    let mut h = Harness::new(&LIMITS);
    h.connect();
    let r = h.live(1);
    let ask = channel::Ask::Push { message: bytes(b"change") };
    let emitted = h.say(r, Up::Call { call: Token::new(7), ask });
    let [first, commit @ Request::Io { op: Op::Commit { .. }, .. }] = &*emitted else {
        panic!("expected a commit, got {emitted:?}");
    };
    assert_eq!(*first, read(r));
    let Request::Io { owner, op, .. } = commit else { unreachable!("matched above") };
    let (owner, done) = (*owner, done(op, true));
    assert_eq!(&*h.say(r, Up::Withdraw { call: Token::new(7) }), [read(r)], "a push goes on");
    let emitted = h.step(Event::Done { owner, done });
    let emitted = h.git(emitted, true);
    let told = channel::Reply::Pushed(channel::Push::Done);
    assert_eq!(&*emitted, [send(r, Down::Answer { call: Token::new(7), reply: told })]);
}

#[test]
fn a_push_the_forge_refuses_fails_and_the_run_is_told() {
    let mut h = Harness::new(&LIMITS);
    h.connect();
    let r = h.live(1);
    let ask = channel::Ask::Push { message: bytes(b"change") };
    let emitted = h.say(r, Up::Call { call: Token::new(7), ask });
    let [_, Request::Io { owner, op: Op::Commit { .. }, .. }] = &*emitted else {
        panic!("expected a commit, got {emitted:?}");
    };
    let owner = *owner;
    let emitted = h.step(Event::Done { owner, done: git::Done::Committed { commit: Commit::new(COMMITTED) } });
    let [Request::Io { op: Op::Push { .. }, .. }] = &*emitted else {
        panic!("expected a push, got {emitted:?}");
    };
    let diagnostic = git::PushDiagnostic::new(b"remote: branch protection rejected this push", 19);
    let refused = git::Done::FailedWithOutput { fault: git::Fault::Refused, diagnostic };
    let told = channel::Reply::Pushed(channel::Push::Failed {
        failure: channel::PushFailure {
            repository: Some(0),
            reason: channel::PushReason::Refused,
            diagnostic: channel::PushDiagnostic::new(diagnostic.output(), diagnostic.cut()),
        },
    });
    let emitted = h.step(Event::Done { owner, done: refused });
    assert_eq!(&*emitted, [send(r, Down::Answer { call: Token::new(7), reply: told })], "a protected branch, say");
}

#[test]
fn an_inbound_event_the_agent_cannot_take_is_bounced_to_the_engine() {
    let limits = Limits {
        host: host::Limits { held: 1, ..LIMITS.host },
        agent: agent::Limits { events: 2, ..LIMITS.agent },
        ..LIMITS
    };
    let mut h = Harness::new(&limits);
    h.connect();
    let r = h.live(1);
    assert_eq!(&*h.step(inbound(r, b"one")), [send(r, Down::Event { name: Token::new(1), event: bytes(b"one") })]);
    assert!(h.step(inbound(r, b"two")).is_empty(), "waits behind the first");
    let bounced = Request::Bounced { name: Token::new(1), run: r.run, attempt: r.attempt, bounce: wire::Bounce::Full };
    assert_eq!(&*h.step(inbound(r, b"three")), [bounced], "the engine keeps it");
}

#[test]
fn the_runs_facts_go_to_the_engine_under_its_names_best_effort() {
    let mut h = Harness::new(&LIMITS);
    h.connect();
    let r = h.live(1);
    assert_eq!(&*h.say(r, Up::Fact { fact: bytes(b"one") }), [read(r)]);
    let told = Told { run: r.run, attempt: r.attempt, fact: bytes(b"one") };
    assert_eq!(h.domain.pop_told(), Some(told));
    assert_eq!(h.domain.pop_told(), None);
    assert!(h.step(Event::Lost).is_empty());
    for fact in [b"two", b"tri", b"for"] {
        h.say(r, Up::Fact { fact: bytes(fact) });
    }
    assert_eq!(h.domain.pop_told(), None, "nothing goes while the channel is down");
    assert_eq!(h.domain.told_lost(), 1, "what does not fit is dropped and counted");
    h.at(10);
    h.connect();
    let told = Told { run: r.run, attempt: r.attempt, fact: bytes(b"two") };
    assert_eq!(h.domain.pop_told(), Some(told), "kept until the channel is back");
}

// The engine link.

#[test]
fn the_link_dials_at_once_and_again_after_a_jittered_backoff() {
    let mut h = Harness::new(&LIMITS);
    assert_eq!(h.domain.next_deadline(), Some(Time::ZERO), "it dials at once");
    assert_eq!(&*h.fire(), [Request::Dial]);
    assert_eq!(h.domain.next_deadline(), None, "one dial in flight");
    let mut from = 0;
    for doubled in [1_u64, 2, 4, 8, 8] {
        assert!(h.step(Event::Lost).is_empty());
        let at = h.domain.next_deadline().expect("it dials again").as_nanos();
        let base = Duration::from_secs(doubled).as_nanos();
        let now = h.env.now.as_nanos();
        assert!(now + base / 2 <= at && at <= now + base, "between half the backoff and all of it: {doubled}s");
        from = at;
        h.env.now = Time::from_nanos(at);
        assert_eq!(&*h.fire(), [Request::Dial]);
    }
    assert!(from > 0);
    let emitted = h.step(Event::Connected);
    let hello = Hello { slots: 2, workstreams: Box::new([]), hosting: Box::new([]) };
    assert_eq!(&*emitted, [Request::Hello { hello }], "hello, first");
    assert!(h.domain.is_connected());
    let proved = Event::Acknowledged { run: Token::new(9), attempt: Token::new(9) };
    assert!(h.step(proved).is_empty(), "the engine speaks: the channel has proved itself");
    assert!(h.step(Event::Lost).is_empty());
    let at = h.domain.next_deadline().expect("the grace, and a dial").as_nanos();
    let now = h.env.now.as_nanos();
    assert!(at <= now + Duration::from_secs(1).as_nanos(), "the backoff starts over");
    assert_eq!(&*h.facts(), [Fact::Connected, Fact::Lost]);
}

#[test]
fn an_answer_made_while_the_channel_is_down_follows_the_next_hello() {
    let mut h = Harness::new(&LIMITS);
    h.connect();
    let first = h.live(1);
    let second = h.live(2);
    assert!(h.step(Event::Lost).is_empty());
    let finish = channel::Finish::Failed { failure: channel::RunFailure::Model };
    h.say(first, Up::Finish { finish });
    let emitted = h.goes(first);
    assert!(h.git(emitted, false).is_empty(), "the answer is held");
    assert_eq!(h.domain.held(), 1);
    h.at(5);
    let emitted = h.connect();
    let hosting = [
        Hosted { run: second.run, attempt: second.attempt, phase: Phase::Active },
        Hosted { run: first.run, attempt: first.attempt, phase: Phase::Answered },
    ];
    let hello = Hello { slots: 2, workstreams: Box::new([key(1), key(2)]), hosting: Box::new(hosting) };
    let failure = wire::Failure::Run(wire::RunFailure::Model);
    let saved = work(&[], Some(Box::new([wire::Landing::Unchanged])));
    let failed = wire::Answer::Failed { failure, detail: bytes(b"bye"), work: saved };
    assert_eq!(&*emitted, [Request::Hello { hello }, answer(first, failed)], "the answer follows the hello");
    assert_eq!(h.domain.held(), 1, "until the engine acknowledges it");
}

#[test]
fn past_the_grace_every_run_is_cancelled_and_the_answer_held_until_the_engine_is_back() {
    let mut h = Harness::new(&LIMITS);
    h.connect();
    let r = h.live(1);
    assert!(h.step(Event::Lost).is_empty());
    h.at(30);
    assert_eq!(&*h.fire(), [Request::Dial], "the dial falls due first");
    assert!(h.step(Event::Lost).is_empty());
    assert!(h.fire().is_empty(), "the grace");
    assert_eq!(&*h.resume(), [send(r, Down::Cancel)], "the run is cancelled for contact");
    assert!(h.step(Event::Sent { owner: r.agent }).is_empty());
    let emitted = h.goes(r);
    assert_eq!(h.domain.held(), 0, "its work is being saved first");
    assert!(h.git(emitted, false).is_empty(), "the answer is held");
    assert_eq!(h.domain.held(), 1);
    let facts = h.facts();
    assert!(facts.contains(&Fact::Grace), "{facts:?}");
    h.at(40);
    let emitted = h.connect();
    let hosting = [Hosted { run: r.run, attempt: r.attempt, phase: Phase::Answered }];
    let hello = Hello { slots: 2, workstreams: Box::new([key(1)]), hosting: Box::new(hosting) };
    let saved = work(&[], Some(Box::new([wire::Landing::Unchanged])));
    let contact = wire::Answer::Failed {
        failure: wire::Failure::Cancelled(wire::Reason::Contact),
        detail: bytes(b"bye"),
        work: saved,
    };
    assert_eq!(&*emitted, [Request::Hello { hello }, answer(r, contact)]);
}

#[test]
fn relays_made_while_the_channel_is_down_follow_the_hello_while_their_calls_wait() {
    let mut h = Harness::new(&LIMITS);
    h.connect();
    let r = h.live(1);
    assert!(h.step(Event::Lost).is_empty());
    for call in 1..=5 {
        let ask = channel::Ask::Relay { body: bytes(b"read") };
        assert_eq!(&*h.say(r, Up::Call { call: Token::new(call), ask }), [read(r)], "kept");
        if call <= 3 {
            withdraw(&mut h, r, call);
            assert_eq!(h.domain.stalled(), 0, "a queued relay cancels without a wire request");
            assert_eq!(h.domain.host().calls(), 0, "its local terminal releases the binding");
        }
    }
    assert_eq!(h.domain.stalled(), 2, "only the two still waiting retain their bytes");
    withdraw(&mut h, r, 5);
    assert_eq!(h.domain.stalled(), 1, "cancellation removes queued bytes immediately");
    h.at(5);
    let emitted = h.connect();
    let [Request::Hello { .. }, Request::Relay { .. }] = &*emitted else {
        panic!("expected the one relay still waiting after the hello, got {emitted:?}");
    };
    assert_eq!(h.domain.stalled(), 0);
}

/// The run withdraws its relayed call `call`, which is answered at once.
fn withdraw(h: &mut Harness, r: Names, call: u64) {
    let emitted = h.say(r, Up::Withdraw { call: Token::new(call) });
    let withdrawn = Down::Answer { call: Token::new(call), reply: channel::Reply::Withdrawn };
    assert_eq!(&*emitted, [read(r), send(r, withdrawn)]);
    assert!(h.step(Event::Sent { owner: r.agent }).is_empty());
    // The iteration ends: the call's slot is free again.
    h.domain.reclaim();
}

#[test]
fn an_answer_is_kept_and_sent_after_every_hello_until_the_engine_acknowledges_it() {
    let mut h = Harness::new(&LIMITS);
    h.connect();
    let r = h.live(1);
    let finish = channel::Finish::Ended { outcome: bytes(b"done") };
    h.say(r, Up::Finish { finish });
    let emitted = h.goes(r);
    let emitted = h.git(emitted, false);
    let saved = work(&[], Some(Box::new([wire::Landing::Unchanged])));
    let ended = wire::Answer::Ended { outcome: bytes(b"done"), work: saved };
    let [Request::Answer { .. }] = &*emitted else {
        panic!("expected the answer, got {emitted:?}");
    };
    assert_eq!(h.domain.held(), 1, "kept until the engine has it");
    assert!(h.step(Event::Lost).is_empty(), "the answer may be lost with the channel");
    h.at(5);
    let emitted = h.connect();
    let hosting = [Hosted { run: r.run, attempt: r.attempt, phase: Phase::Answered }];
    let hello = Hello { slots: 2, workstreams: Box::new([key(1)]), hosting: Box::new(hosting) };
    assert_eq!(&*emitted, [Request::Hello { hello }, answer(r, ended)], "sent again after the hello");
    assert!(h.step(Event::Assign { assignment: assignment(1) }).is_empty(), "the attempt answered is dropped");
    assert!(h.step(Event::Acknowledged { run: r.run, attempt: r.attempt }).is_empty());
    assert!(h.step(Event::Acknowledged { run: r.run, attempt: r.attempt }).is_empty(), "again: nothing");
    assert_eq!(h.domain.held(), 0);
    assert!(h.step(Event::Lost).is_empty());
    h.at(10);
    let emitted = h.connect();
    let hello = Hello { slots: 2, workstreams: Box::new([key(1)]), hosting: Box::new([]) };
    assert_eq!(&*emitted, [Request::Hello { hello }], "forgotten");
}

#[test]
fn an_answer_the_engine_has_yet_to_acknowledge_keeps_its_slot() {
    let mut h = Harness::new(&LIMITS);
    h.connect();
    let first = h.live(1);
    h.live(2);
    h.step(Event::Cancel { run: first.run, attempt: first.attempt });
    h.step(Event::Sent { owner: first.agent });
    let emitted = h.goes(first);
    assert_eq!(h.git(emitted, false).len(), 1, "answered");
    h.domain.reclaim();
    assert_eq!(h.domain.host().hosted(), 1, "its host slot is back");
    let third = Names { run: Token::new(3), attempt: attempt(3), ..first };
    let busy = wire::Answer::Refused(wire::Refusal::Busy);
    assert_eq!(&*h.step(Event::Assign { assignment: assignment(3) }), [answer(third, busy)], "not yet free");
    assert_eq!(h.domain.held(), 1, "a refusal is not kept");
    assert!(h.step(Event::Acknowledged { run: first.run, attempt: first.attempt }).is_empty());
    h.assign(3);
}

#[test]
fn a_channel_that_opens_and_drops_unproved_keeps_the_backoff_growing() {
    let mut h = Harness::new(&LIMITS);
    h.connect();
    assert!(h.step(Event::Lost).is_empty());
    let first = h.domain.next_deadline().expect("a dial").as_nanos();
    assert!(first <= Duration::from_secs(1).as_nanos());
    h.env.now = Time::from_nanos(first);
    h.connect();
    assert!(h.step(Event::Lost).is_empty(), "dropped before the engine said a word");
    let second = h.domain.next_deadline().expect("a dial").as_nanos() - first;
    assert!(second >= Duration::from_secs(1).as_nanos(), "the backoff doubled: {second}");
    h.env.now = Time::from_nanos(first + second);
    h.connect();
    assert!(h.step(Event::Acknowledged { run: Token::new(9), attempt: Token::new(9) }).is_empty(), "proved");
    assert!(h.step(Event::Lost).is_empty());
    let third = h.domain.next_deadline().expect("a dial").as_nanos() - first - second;
    assert!(third <= Duration::from_secs(1).as_nanos(), "the backoff starts over: {third}");
}

// Shutting down.

#[test]
fn a_worker_shutting_down_cancels_every_run_and_is_done_once_every_answer_has_gone() {
    let mut h = Harness::new(&LIMITS);
    h.connect();
    let r = h.live(1);
    assert!(h.step(Event::Shutdown).is_empty(), "runs are cancelled one at a time");
    assert!(!h.domain.is_done());
    assert_eq!(&*h.resume(), [send(r, Down::Cancel)]);
    let late = Names { run: Token::new(2), attempt: attempt(2), ..r };
    let busy = wire::Answer::Refused(wire::Refusal::Busy);
    assert_eq!(&*h.step(Event::Assign { assignment: assignment(2) }), [answer(late, busy)], "no more runs");
    assert!(h.step(Event::Sent { owner: r.agent }).is_empty());
    let emitted = h.goes(r);
    let emitted = h.git(emitted, false);
    let [Request::Answer { run, .. }] = &*emitted else {
        panic!("expected the answer, got {emitted:?}");
    };
    assert_eq!(*run, r.run);
    h.domain.reclaim();
    assert!(!h.domain.is_done(), "until the engine has the answer");
    assert!(h.step(Event::Acknowledged { run: r.run, attempt: r.attempt }).is_empty());
    assert!(h.domain.is_done());
    assert_eq!(h.domain.abandoned(), 0, "every answer delivered");
}

#[test]
fn a_worker_shutting_down_offers_no_slots() {
    let mut h = Harness::new(&LIMITS);
    h.connect();
    let r = h.live(1);
    assert!(h.step(Event::Shutdown).is_empty());
    assert!(h.step(Event::Lost).is_empty());
    h.at(5);
    let emitted = h.connect();
    let hosting = [Hosted { run: r.run, attempt: r.attempt, phase: Phase::Active }];
    let hello = Hello { slots: 0, workstreams: Box::new([key(1)]), hosting: Box::new(hosting) };
    assert_eq!(&*emitted, [Request::Hello { hello }], "it takes no more work");
}

#[test]
fn a_worker_shutting_down_out_of_reach_past_the_grace_gives_up_its_answers() {
    let mut h = Harness::new(&LIMITS);
    h.connect();
    let r = h.live(1);
    assert!(h.step(Event::Lost).is_empty());
    assert!(h.step(Event::Shutdown).is_empty());
    assert_eq!(&*h.resume(), [send(r, Down::Cancel)]);
    assert!(h.step(Event::Sent { owner: r.agent }).is_empty());
    let emitted = h.goes(r);
    assert!(h.git(emitted, false).is_empty(), "held within the grace");
    h.domain.reclaim();
    assert_eq!(h.domain.held(), 1);
    assert!(!h.domain.is_done(), "the engine may still come back");
    h.at(30);
    assert_eq!(&*h.fire(), [Request::Dial]);
    assert!(h.step(Event::Lost).is_empty());
    assert!(h.fire().is_empty(), "the grace");
    assert_eq!((h.domain.held(), h.domain.abandoned()), (0, 1), "given up, and counted");
    assert!(h.domain.is_done());
}

/// Two runs live, the channel lost, and the worker told to shut down: both
/// are cancelled, and the first goes, its answer kept.
fn shutting_down_out_of_reach(h: &mut Harness) -> (Names, Names) {
    h.connect();
    let first = h.live(1);
    let second = h.live(2);
    assert!(h.step(Event::Lost).is_empty());
    assert!(h.step(Event::Shutdown).is_empty());
    for r in [first, second] {
        assert_eq!(&*h.resume(), [send(r, Down::Cancel)]);
        assert!(h.step(Event::Sent { owner: r.agent }).is_empty());
    }
    let emitted = h.goes(first);
    assert!(h.git(emitted, false).is_empty(), "kept without a channel");
    h.domain.reclaim();
    (first, second)
}

/// The clock moves to `secs`, and every alarm due fires: every dial fails,
/// and the agents' trees are signalled.
fn out_of_reach_until(h: &mut Harness, secs: u64) {
    h.at(secs);
    for _ in 0..ROUNDS {
        if !h.domain.is_due(h.env.now) {
            return;
        }
        for request in h.fire() {
            let emitted = match request {
                Request::Dial => h.step(Event::Lost),
                Request::Signal { owner, .. } => h.step(Event::Signalled { owner }),
                other @ (Request::HelloV2 { .. }
                | Request::Turn { .. }
                | Request::AnswerV2 { .. }
                | Request::RelayV2 { .. }
                | Request::RelayTyped { .. }
                | Request::Hello { .. }
                | Request::Answer { .. }
                | Request::Relay { .. }
                | Request::Bounced { .. }
                | Request::Rejected { .. }
                | Request::Exhausted { .. }
                | Request::Spawn { .. }
                | Request::Send { .. }
                | Request::Read { .. }
                | Request::Wait { .. }
                | Request::Reap { .. }
                | Request::Io { .. }
                | Request::CancelRelay { .. }
                | Request::CancelIo { .. }) => panic!("only dials and signals: {other:?}"),
            };
            assert!(emitted.is_empty(), "nothing follows: {emitted:?}");
        }
    }
    panic!("the alarms settle within {ROUNDS} rounds");
}

#[test]
fn a_worker_shutting_down_keeps_its_answers_while_a_run_is_left_and_delivers_them_once_back() {
    let mut h = Harness::new(&LIMITS);
    let (first, second) = shutting_down_out_of_reach(&mut h);
    out_of_reach_until(&mut h, 31);
    assert_eq!((h.domain.held(), h.domain.abandoned()), (1, 0), "past the grace, a run is left: the answer is kept");
    // The channel opens again before the last run has answered.
    out_of_reach_until(&mut h, 40);
    let dial = h.domain.next_deadline().expect("a dial");
    h.env.now = dial;
    let emitted = h.connect();
    let hosting = [
        Hosted { run: second.run, attempt: second.attempt, phase: Phase::Ending },
        Hosted { run: first.run, attempt: first.attempt, phase: Phase::Answered },
    ];
    let [Request::Hello { hello }, Request::Answer { run, .. }] = &*emitted else {
        panic!("expected the hello and the answer kept, got {emitted:?}");
    };
    assert_eq!(&*hello.hosting, hosting);
    assert_eq!(*run, first.run, "the answer kept goes after the hello");
    let emitted = h.goes(second);
    let [Request::Answer { run, .. }] = &*h.git(emitted, false) else {
        panic!("the last run answers on the channel");
    };
    assert_eq!(*run, second.run);
    h.domain.reclaim();
    for r in [first, second] {
        assert!(h.step(Event::Acknowledged { run: r.run, attempt: r.attempt }).is_empty());
    }
    assert!(h.domain.is_done());
    assert_eq!(h.domain.abandoned(), 0, "every answer delivered");
}

#[test]
fn a_worker_shutting_down_out_of_reach_gives_up_its_answers_once_no_run_is_left() {
    let mut h = Harness::new(&LIMITS);
    let (_, second) = shutting_down_out_of_reach(&mut h);
    out_of_reach_until(&mut h, 31);
    assert_eq!(h.domain.held(), 1, "kept while a run is left");
    let emitted = h.goes(second);
    assert!(h.git(emitted, false).is_empty(), "nothing goes without a channel");
    assert_eq!((h.domain.held(), h.domain.abandoned()), (0, 2), "both given up as the last run answers");
    h.domain.reclaim();
    assert!(h.domain.is_done());
}

// Limits.

#[test]
fn the_worst_case_is_bounded_or_refused() {
    assert!(worst_case(&LIMITS).is_some());
    let disagree = [
        Limits { checkout: checkout::Limits { repositories: 9, ..LIMITS.checkout }, ..LIMITS },
        Limits { checkout: checkout::Limits { name_bytes: 257, ..LIMITS.checkout }, ..LIMITS },
        Limits {
            host: host::Limits { slots: 3, ..LIMITS.host },
            agent: agent::Limits { agents: 3, ..LIMITS.agent },
            ..LIMITS
        },
        Limits {
            host: host::Limits { slots: 3, ..LIMITS.host },
            checkout: checkout::Limits { workspaces: 3, ..LIMITS.checkout },
            ..LIMITS
        },
        Limits { host: host::Limits { charter_bytes: 65, ..LIMITS.host }, ..LIMITS },
        Limits { host: host::Limits { snapshot_bytes: 31, ..LIMITS.host }, ..LIMITS },
        Limits { host: host::Limits { outcome_bytes: 33, ..LIMITS.host }, ..LIMITS },
        Limits { host: host::Limits { event_bytes: 17, ..LIMITS.host }, ..LIMITS },
        Limits { host: host::Limits { held: 5, ..LIMITS.host }, ..LIMITS },
        Limits { stalled: 3, ..LIMITS },
        Limits { agent: agent::Limits { call_bytes: 65, ..LIMITS.agent }, ..LIMITS },
        Limits {
            checkout: checkout::Limits { message_bytes: 8, ..LIMITS.checkout },
            agent: agent::Limits { call_bytes: 8, ..LIMITS.agent },
            ..LIMITS
        },
        Limits { redial: Duration::ZERO, ..LIMITS },
        Limits { redial: Duration::from_secs(9), ..LIMITS },
        Limits { checkout: checkout::Limits { workspaces: 0, ..LIMITS.checkout }, ..LIMITS },
        Limits {
            host: host::Limits { told: u32::MAX, fact_bytes: u64::MAX, ..LIMITS.host },
            agent: agent::Limits { fact_bytes: u64::MAX, ..LIMITS.agent },
            ..LIMITS
        },
    ];
    for limits in disagree {
        assert_eq!(worst_case(&limits), None, "{limits:?}");
    }
    let wider = Limits {
        host: host::Limits { slots: 2, charter_bytes: 32, event_bytes: 8, ..LIMITS.host },
        checkout: checkout::Limits { workspaces: 4, repositories: 4, name_bytes: 32, ..LIMITS.checkout },
        agent: agent::Limits { agents: 4, ..LIMITS.agent },
        ..LIMITS
    };
    assert!(worst_case(&wider).is_some(), "capabilities may take more than the host gives them");
}

#[test]
fn max_out_follows_the_longest_chain_of_hand_offs() {
    // Cancelling two relays emits two replies and two cancels, with room
    // for the stop and answer. Immediate child terminals extend the chain.
    assert_eq!(host::max_out(&LIMITS.host), 8);
    assert_eq!(max_out(&LIMITS), 18 * 8 + 50 * checkout::MAX_OUT + 49 * agent::MAX_OUT + 2 + 4 + 2 * (2 + 4));
}

#[test]
fn wire_assignment_retries_create_no_second_child_call() {
    let mut h = Harness::new(&LIMITS);
    h.connect();
    let r = h.live(1);
    assert!(h.step(Event::Assign { assignment: assignment(1) }).is_empty());
    assert_eq!(h.domain.host().hosted(), 1);
    assert_eq!(h.domain.host().unanswered(), 1);
    let ask = channel::Ask::Relay { body: bytes(b"read") };
    let emitted = h.say(r, Up::Call { call: Token::new(7), ask });
    let [_, Request::Relay { call, .. }] = &*emitted else {
        panic!("expected relay");
    };
    let call = *call;
    for (run, attempt) in [(Token::new(999), r.attempt), (r.run, Token::new(999))] {
        assert!(h.step(Event::Relayed { run, attempt, call, answer: bytes(b"bad") }).is_empty());
    }
    assert!(h.domain.host().is_relayed_for(r.run, r.attempt, call));
    let answered = Event::Relayed { run: r.run, attempt: r.attempt, call, answer: bytes(b"ok") };
    assert_eq!(h.step(answered).len(), 1, "the valid terminal still arrives");
}

#[test]
fn initial_git_grant_names_stay_on_the_worker() {
    let mut h = Harness::new(&LIMITS);
    h.connect();
    let git = wire::Grant { account: 0, generation: 0, valid: Duration::from_secs(60) };
    let llm = wire::Grant { account: 7, generation: 1, valid: Duration::from_secs(60) };
    let assigned = wire::Assignment { grants: Box::new([git, llm]), ..assignment(1) };
    let emitted = h.step(Event::Assign { assignment: assigned });
    let emitted = h.git(emitted, true);
    let [Request::Spawn { owner, .. }] = &*emitted else {
        panic!("spawn");
    };
    let owner = *owner;
    let started = h.step(Event::Spawned { owner, process: Token::new(200) });
    let mut found = false;
    for request in &started {
        if let Request::Send { message: Down::Start { grants, repositories, .. }, .. } = request {
            assert_eq!(
                &**grants,
                [channel::Grant { account: llm.account, generation: llm.generation, valid: llm.valid }]
            );
            assert_eq!(&**repositories, [channel::Repository { name: bytes(b"app"), writable: true }]);
            found = true;
        }
    }
    assert!(found, "the agent receives its start metadata");
    let sent = h.step(Event::Sent { owner });
    assert!(sent.is_empty());
    let rejected = h.step(Event::Received { owner, message: Up::Rejected { account: 7, generation: 1 } });
    assert_eq!(
        &*rejected,
        [
            Request::Rejected { run: Token::new(1), attempt: attempt(1), account: 7, generation: 1 },
            Request::Read { owner, process: Token::new(200) }
        ]
    );
}

fn next_limits() -> Limits {
    let mut limits = LIMITS;
    limits.host.transcript_bytes = 64;
    limits.host.turn_bytes = 16;
    limits.agent.transcript_bytes = 64;
    limits.agent.turn_bytes = 16;
    limits.host.turns = 2;
    limits.host.turn_queue_bytes = 32;
    limits
}

fn next_live(h: &mut Harness, run: u64) -> Names {
    assert_eq!(&*h.fire(), [Request::Dial]);
    let hello = h.step(Event::ConnectedV2);
    let [Request::HelloV2 { hello, graces, push_deadline }] = &*hello else { panic!("v2 hello first") };
    assert_eq!(hello.slots, h.env.limits.host.slots);
    assert_eq!(*push_deadline, Duration::from_secs(260));
    assert_eq!(*graces, Duration::from_secs(550));
    let assignment =
        wire::AssignmentV2 { assignment: assignment(run), transcript: Some(bytes(b"turns; committed-call-tail")) };
    let emitted = h.step(Event::AssignV2 { assignment });
    let emitted = h.git(emitted, true);
    let [Request::Spawn { owner, .. }] = &*emitted else { panic!("v2 spawn: {emitted:?}") };
    let r = Names {
        run: Token::new(run),
        attempt: attempt(run),
        agent: *owner,
        process: Token::new(run.saturating_add(500)),
    };
    let emitted = h.step(Event::Spawned { owner: r.agent, process: r.process });
    let expected = Down::StartV2 {
        charter: bytes(b"charter"),
        transcript: Some(bytes(b"turns; committed-call-tail")),
        repositories: Box::new([channel::RepositoryV2 {
            name: bytes(b"app"),
            writable: true,
            conflicts: Box::new([]),
        }]),
        grants: Box::new([]),
    };
    assert_eq!(
        &*emitted,
        [
            Request::Wait { owner: r.agent, process: r.process },
            Request::Reap { owner: r.agent, process: r.process },
            send(r, expected),
            read(r)
        ]
    );
    assert!(h.step(Event::Sent { owner: r.agent }).is_empty());
    r
}

fn next_turn(h: &mut Harness, r: Names, turn: u32, spent: u64) -> Box<[Request]> {
    h.say(r, Up::Turn { turn: channel::Turn { turn, spent, read: None, body: bytes(b"opaque-turn") } })
}

#[test]
fn v2_turn_capacity_busy_retry_and_exact_commit_ack_resume_the_reader() {
    let mut limits = next_limits();
    limits.host.turns = 1;
    limits.host.turn_queue_bytes = 16;
    let mut h = Harness::new(&limits);
    let r = next_live(&mut h, 31);
    let emitted = next_turn(&mut h, r, 1, 17);
    let [Request::Turn { run, attempt, turn }] = &*emitted else { panic!("full window pauses read: {emitted:?}") };
    assert_eq!((*run, *attempt, turn.turn, turn.spent, &*turn.body), (r.run, r.attempt, 1, 17, &b"opaque-turn"[..]));
    assert_eq!(h.domain.retained_turns(), 1);
    assert!(h.step(Event::AcknowledgeTurn { run: r.run, attempt: Token::new(900), turn: 1 }).is_empty());
    assert!(h.step(Event::AcknowledgeTurn { run: r.run, attempt: r.attempt, turn: 2 }).is_empty());
    assert!(h.step(Event::TurnBusy { run: r.run, attempt: r.attempt, turn: 1 }).is_empty());
    h.at(1);
    assert_eq!(&*h.fire(), &*emitted, "busy retries the original body and identity");
    assert_eq!(h.domain.retained_turns(), 1, "busy is not commitment");
    assert_eq!(&*h.step(Event::AcknowledgeTurn { run: r.run, attempt: r.attempt, turn: 1 }), [read(r)]);
    assert_eq!(h.domain.retained_turns(), 0);
    assert!(
        h.step(Event::AcknowledgeTurn { run: r.run, attempt: r.attempt, turn: 1 }).is_empty(),
        "duplicate ACK is harmless"
    );
    assert_eq!(next_turn(&mut h, r, 2, 19).len(), 1);
}

#[test]
fn v2_rehello_replays_turns_before_answers_and_crossed_acks_keep_the_slot() {
    let mut h = Harness::new(&next_limits());
    let r = next_live(&mut h, 32);
    let emitted = next_turn(&mut h, r, 1, 13);
    assert_eq!(emitted.len(), 2, "another read fits");
    let emitted = h.say(r, Up::FinishV2 { turns: 1, spent: 21, finish: channel::FinishV2::Parked });
    assert_eq!(&*emitted, [read(r)]);
    let emitted = h.goes(r);
    let emitted = h.git(emitted, false);
    let [Request::AnswerV2 { run, attempt, answer }] = &*emitted else { panic!("v2 answer: {emitted:?}") };
    assert_eq!((*run, *attempt, answer.turns, answer.spent), (r.run, r.attempt, 1, 21));
    assert_eq!(answer.ending, wire::EndingV2::Parked { work: work(&[], Some(Box::new([wire::Landing::Unchanged]))) });
    h.domain.reclaim();
    assert_eq!(h.domain.workspaces(), 0);
    assert_eq!(h.domain.agent().agents(), 0);
    assert_eq!(h.domain.held(), 1);
    assert!(h.step(Event::Lost).is_empty());
    h.at(1);
    assert_eq!(&*h.fire(), [Request::Dial]);
    let replay = h.step(Event::ConnectedV2);
    let [Request::HelloV2 { hello, .. }, Request::Turn { turn, .. }, Request::AnswerV2 { answer: replayed, .. }] =
        &*replay
    else {
        panic!("hello/turn/answer order: {replay:?}")
    };
    assert_eq!(&*hello.hosting, [Hosted { run: r.run, attempt: r.attempt, phase: Phase::Answered }]);
    assert_eq!((turn.turn, &*turn.body), (1, &b"opaque-turn"[..]));
    assert_eq!(replayed, answer);
    assert!(h.step(Event::Acknowledged { run: r.run, attempt: r.attempt }).is_empty());
    assert_eq!(h.domain.held(), 1, "answer ACK alone cannot free retained turns");
    assert!(
        h.step(Event::AssignV2 { assignment: wire::AssignmentV2 { assignment: assignment(32), transcript: None } })
            .is_empty(),
        "retry does not restart an answered attempt"
    );
    assert!(
        h.step(Event::AcknowledgeTurn { run: r.run, attempt: r.attempt, turn: 1 }).is_empty(),
        "the agent already went"
    );
    assert_eq!(h.domain.held(), 0);
}

#[test]
fn v2_deadlines_and_cross_child_limits_are_checked_before_startup() {
    let limits = next_limits();
    assert_eq!(crate::push_deadline(&limits), Some(Duration::from_secs(260)));
    assert_eq!(crate::declared_graces(&limits), Some(Duration::from_secs(550)));
    let mut bad = limits;
    bad.checkout.remote_timeout = Duration::from_nanos(u64::MAX);
    assert!(worst_case(&bad).is_none(), "deadline arithmetic cannot wrap");
    let mut bad = limits;
    bad.host.turn_queue_bytes = 15;
    assert!(worst_case(&bad).is_none(), "a credit needs space for a maximum turn");
    let mut bad = limits;
    bad.checkout.conflicts = 1;
    assert!(worst_case(&bad).is_none(), "checkout's returned paths must fit the host and agent");
}

#[test]
fn v2_relay_answers_match_the_stable_name_and_the_local_delivery() {
    let mut h = Harness::new(&next_limits());
    let r = next_live(&mut h, 33);
    let emitted = h.say(r, Up::Call { call: Token::new(51), ask: channel::Ask::Relay { body: bytes(b"opaque-call") } });
    let [reading, Request::RelayV2 { run, attempt, call, delivery, body }] = &*emitted else {
        panic!("relay: {emitted:?}")
    };
    assert_eq!(reading, &read(r));
    assert_eq!((*run, *attempt, *call, body.as_ref()), (r.run, r.attempt, Token::new(51), &b"opaque-call"[..]));
    let delivery = *delivery;
    for event in [
        Event::RelayedV2 {
            run: r.run,
            attempt: Token::new(99),
            call: Token::new(51),
            delivery,
            answer: bytes(b"stale"),
        },
        Event::RelayedV2 {
            run: r.run,
            attempt: r.attempt,
            call: Token::new(52),
            delivery,
            answer: bytes(b"wrong-name"),
        },
        Event::RelayedV2 {
            run: r.run,
            attempt: r.attempt,
            call: Token::new(51),
            delivery: Token::new(999),
            answer: bytes(b"wrong-delivery"),
        },
    ] {
        assert!(h.step(event).is_empty());
    }
    let emitted = h.step(Event::RelayedV2 {
        run: r.run,
        attempt: r.attempt,
        call: Token::new(51),
        delivery,
        answer: bytes(b"kept"),
    });
    assert_eq!(
        &*emitted,
        [send(r, Down::Answer { call: Token::new(51), reply: channel::Reply::Relayed { answer: bytes(b"kept") } })]
    );
}

#[test]
fn direct_v2_assignment_gets_a_supported_refusal_when_the_runtime_mode_is_disabled() {
    let mut h = Harness::new(&LIMITS);
    assert_eq!(&*h.fire(), [Request::Dial]);
    h.step(Event::ConnectedV2);
    let next = wire::AssignmentV2 { assignment: assignment(1), transcript: None };
    let emitted = h.step(Event::AssignV2 { assignment: next });
    let answer = wire::AnswerV2 {
        turns: 0,
        spent: 0,
        ending: wire::EndingV2::Refused(wire::Refusal::Invalid(wire::Invalid::Version)),
    };
    assert_eq!(&*emitted, [Request::AnswerV2 { run: Token::new(1), attempt: attempt(1), answer }]);
    assert_eq!(h.domain.held(), 0);
    assert_eq!(h.domain.host().hosted(), 0);
    let mut h = Harness::new(&next_limits());
    h.connect();
    let next = wire::AssignmentV2 { assignment: assignment(2), transcript: None };
    let emitted = h.step(Event::AssignV2 { assignment: next });
    assert_eq!(
        &*emitted,
        [Request::Answer {
            run: Token::new(2),
            attempt: attempt(2),
            answer: wire::Answer::Refused(wire::Refusal::Invalid(wire::Invalid::Charter))
        }]
    );
    assert_eq!(h.domain.held(), 0);
    assert_eq!(h.domain.host().hosted(), 0);
}
