//! Feed the model events, inspect the requests that come out.

use alloc::boxed::Box;

use temper_lib::{Env, List, Queue, ReplyTo, Time, Token};

use crate::{
    Access, AgentFailure, Answer, Ask, Assignment, Bounce, Event, Fact, Failure, Finish, Hosting, Invalid, Landing,
    Limits, Model, Phase, Preparation, Push, Reason, Refusal, Reply, Repository, Request, RunFailure, Start, Work,
    Workspace, max_out, resume, step, worst_case,
};

const LIMITS: Limits = Limits {
    slots: 2,
    repositories: 2,
    name_bytes: 16,
    charter_bytes: 64,
    snapshot_bytes: 32,
    outcome_bytes: 32,
    detail_bytes: 8,
    held: 2,
    event_bytes: 16,
    run_calls: 2,
    facts: 64,
};

/// The model, its environment, and room for one step's output.
struct Harness {
    model: Model,
    env: Env<Limits>,
    out: Queue<Request>,
}

/// A run as a test drives it: the engine's names for it, and the host's
/// tokens and those it was given.
#[derive(Clone, Copy, Debug)]
struct Names {
    run: Token,
    attempt: Token,
    owner: Token,
    workspace: Token,
    agent: Token,
}

impl Harness {
    fn new(limits: Limits) -> Harness {
        let out = Queue::with_capacity(max_out(&limits));
        Harness { model: Model::new(&limits), env: Env { now: Time::ZERO, limits }, out }
    }

    /// Steps `event`, returning what it emitted, oldest first.
    fn step(&mut self, event: Event) -> Box<[Request]> {
        step(&mut self.model, &self.env, event, &mut self.out);
        self.drain()
    }

    fn resume(&mut self) -> Box<[Request]> {
        assert!(self.model.is_ready(), "a run is ready");
        resume(&mut self.model, &self.env, &mut self.out);
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

    fn assign(&mut self, assignment: Assignment) -> Box<[Request]> {
        let reply_to = ReplyTo::new(assignment.run);
        self.step(Event::Assign { reply_to, assignment })
    }

    /// Assigns the test assignment for `run`, which is admitted: the run, with
    /// its workspace being prepared.
    fn admit(&mut self, run: u64) -> Names {
        let emitted = self.assign(assignment(run));
        let [Request::Prepare { owner, workspace }] = &*emitted else {
            panic!("expected a prepare, got {emitted:?}");
        };
        assert_eq!(*workspace, self::workspace());
        Names {
            run: Token::new(run),
            attempt: token(run, 1000),
            owner: *owner,
            workspace: token(run, 100),
            agent: token(run, 200),
        }
    }

    /// Admits `run`, and has its workspace prepared: its agent is starting.
    fn starting(&mut self, run: u64) -> Names {
        let hosted = self.admit(run);
        let emitted = self.step(Event::Prepared { owner: hosted.owner, workspace: hosted.workspace });
        let start = Request::Start {
            owner: hosted.owner,
            workspace: hosted.workspace,
            charter: bytes(b"charter"),
            snapshot: None,
        };
        assert_eq!(&*emitted, [start]);
        hosted
    }

    /// Admits `run`, and has its agent started: it is live.
    fn live(&mut self, run: u64) -> Names {
        let hosted = self.starting(run);
        assert!(self.step(Event::Started { owner: hosted.owner, agent: hosted.agent }).is_empty(), "nothing held");
        hosted
    }

    fn inbound(&mut self, hosted: Names, event: &[u8]) -> Box<[Request]> {
        self.step(Event::Inbound { run: hosted.run, attempt: hosted.attempt, event: bytes(event) })
    }

    fn cancel(&mut self, hosted: Names) -> Box<[Request]> {
        self.step(Event::Cancel { run: hosted.run, attempt: hosted.attempt })
    }

    fn call(&mut self, hosted: Names, call: u64, ask: Ask) -> Box<[Request]> {
        self.step(Event::Called { owner: hosted.owner, call: Token::new(call), ask })
    }

    /// The live run makes the push call `call`: the host's token for it.
    fn push(&mut self, hosted: Names, call: u64) -> Token {
        let emitted = self.call(hosted, call, Ask::Push { message: bytes(b"change") });
        let [Request::Push { owner, workspace, message }] = &*emitted else {
            panic!("expected a push, got {emitted:?}");
        };
        assert_eq!((*workspace, &**message), (hosted.workspace, &b"change"[..]));
        *owner
    }

    /// The live run makes the relayed call `call`: the host's token for it.
    fn relay(&mut self, hosted: Names, call: u64) -> Token {
        let emitted = self.call(hosted, call, Ask::Relay { body: bytes(b"read") });
        let [Request::Relay { run, attempt, call, body }] = &*emitted else {
            panic!("expected a relay, got {emitted:?}");
        };
        assert_eq!((*run, *attempt, &**body), (hosted.run, hosted.attempt, &b"read"[..]));
        *call
    }

    fn finish(&mut self, hosted: Names, finish: Finish) -> Box<[Request]> {
        self.step(Event::Finished { owner: hosted.owner, finish })
    }

    fn gone(&mut self, hosted: Names, detail: &[u8]) -> Box<[Request]> {
        self.step(Event::Gone { owner: hosted.owner, detail: bytes(detail) })
    }

    fn report(&mut self) -> Box<[Hosting]> {
        let emitted = self.step(Event::Report);
        let [Request::Hosting { runs }] = &*emitted else {
            panic!("expected a report, got {emitted:?}");
        };
        runs.clone()
    }

    fn facts(&mut self) -> Box<[Fact]> {
        let mut facts = List::with_capacity(self.env.limits.facts);
        for _ in 0..self.env.limits.facts {
            let Some(fact) = self.model.pop_fact() else { break };
            facts.push(fact).expect("room for the facts");
        }
        facts.into_boxed()
    }
}

/// The test's name for something of the run `run`, `offset` past it.
fn token(run: u64, offset: u64) -> Token {
    Token::new(run.checked_add(offset).expect("a test name fits"))
}

fn bytes(text: &[u8]) -> Box<[u8]> {
    Box::from(text)
}

fn workspace() -> Workspace {
    Workspace {
        key: bytes(b"issue-7"),
        repositories: Box::new([
            Repository {
                name: bytes(b"app"),
                remote: bytes(b"org/app"),
                start: Start::Base { branch: bytes(b"main") },
                access: Access::Writable { push: bytes(b"fix-7"), identity: bytes(b"bot") },
            },
            Repository {
                name: bytes(b"lib"),
                remote: bytes(b"org/lib"),
                start: Start::Commit { commit: bytes(b"abc123") },
                access: Access::ReadOnly,
            },
        ]),
    }
}

/// The test assignment for `run`: two repositories, work saved to `saved`,
/// and no snapshot.
fn assignment(run: u64) -> Assignment {
    Assignment {
        run: Token::new(run),
        attempt: token(run, 1000),
        workspace: workspace(),
        save: Some(bytes(b"saved")),
        charter: bytes(b"charter"),
        snapshot: None,
    }
}

fn answer(hosted: Names, answer: Answer) -> Request {
    Request::Answer { to: ReplyTo::new(hosted.run), run: hosted.run, attempt: hosted.attempt, answer }
}

fn nothing() -> Work {
    Work { landed: Box::new([]), saved: None }
}

fn failed(failure: Failure, detail: &[u8], work: Work) -> Answer {
    Answer::Failed { failure, detail: bytes(detail), work }
}

fn reply(hosted: Names, call: u64, reply: Reply) -> Request {
    Request::Reply { agent: hosted.agent, call: Token::new(call), reply }
}

fn save(hosted: Names) -> Request {
    Request::Save { owner: hosted.owner, workspace: hosted.workspace, branch: bytes(b"saved") }
}

fn release(hosted: Names) -> Request {
    Request::Release { workspace: hosted.workspace }
}

fn stop(hosted: Names) -> Request {
    Request::Stop { agent: hosted.agent }
}

fn bounced(hosted: Names, bounce: Bounce) -> Request {
    Request::Bounced { run: hosted.run, attempt: hosted.attempt, bounce }
}

fn deliver(hosted: Names, event: &[u8]) -> Request {
    Request::Deliver { agent: hosted.agent, event: bytes(event) }
}

/// Ends the stopping run's tail with its agent gone and nothing to save, or
/// asserts what it does instead.
fn saving(h: &mut Harness, hosted: Names) {
    assert_eq!(&*h.gone(hosted, b""), [save(hosted)], "unfinished work is saved");
}

// Admission.

#[test]
fn an_assignment_within_the_limits_is_admitted_and_its_workspace_prepared() {
    let mut h = Harness::new(LIMITS);
    h.admit(1);
    assert_eq!(h.model.hosted(), 1);
    assert_eq!(&*h.facts(), [Fact::Admitted { run: Token::new(1), attempt: Token::new(1001) }]);
}

#[test]
fn an_assignment_beyond_the_limits_is_refused_as_invalid() {
    let cases: [(Assignment, Invalid); 8] = [
        (Assignment { charter: Box::from([0_u8; 65]), ..assignment(1) }, Invalid::Charter),
        (Assignment { snapshot: Some(Box::from([0_u8; 33])), ..assignment(1) }, Invalid::Snapshot),
        (Assignment { save: Some(bytes(b"")), ..assignment(1) }, Invalid::Name),
        (
            Assignment { workspace: Workspace { key: Box::from([0_u8; 17]), ..workspace() }, ..assignment(1) },
            Invalid::Name,
        ),
        (
            Assignment {
                workspace: Workspace {
                    key: bytes(b"k"),
                    repositories: Box::new([repository(b"a"), repository(b"b"), repository(b"c")]),
                },
                ..assignment(1)
            },
            Invalid::Repositories,
        ),
        (
            Assignment {
                workspace: Workspace { key: bytes(b"k"), repositories: Box::new([repository(b"a"), repository(b"a")]) },
                ..assignment(1)
            },
            Invalid::Duplicate,
        ),
        (
            Assignment {
                workspace: Workspace {
                    key: bytes(b"k"),
                    repositories: Box::new([Repository {
                        access: Access::Writable { push: bytes(b"p"), identity: bytes(b"") },
                        ..repository(b"a")
                    }]),
                },
                ..assignment(1)
            },
            Invalid::Name,
        ),
        (
            Assignment {
                workspace: Workspace {
                    key: bytes(b"k"),
                    repositories: Box::new([Repository {
                        start: Start::Saved { branch: Box::from([0_u8; 17]) },
                        ..repository(b"a")
                    }]),
                },
                ..assignment(1)
            },
            Invalid::Name,
        ),
    ];
    for (assignment, invalid) in cases {
        let mut h = Harness::new(LIMITS);
        let hosted = Names { run: assignment.run, attempt: assignment.attempt, ..run_of(1) };
        let emitted = h.assign(assignment);
        assert_eq!(&*emitted, [answer(hosted, Answer::Refused(Refusal::Invalid(invalid)))]);
        assert_eq!(h.model.hosted(), 0, "a refused assignment is never admitted");
        assert!(h.facts().is_empty(), "a refusal tells nothing");
    }
}

fn repository(name: &[u8]) -> Repository {
    Repository {
        name: bytes(name),
        remote: bytes(b"org/repo"),
        start: Start::Branch { branch: bytes(b"b") },
        access: Access::ReadOnly,
    }
}

fn run_of(run: u64) -> Names {
    Names {
        run: Token::new(run),
        attempt: token(run, 1000),
        owner: Token::new(0),
        workspace: token(run, 100),
        agent: token(run, 200),
    }
}

#[test]
fn an_assignment_with_every_slot_taken_is_refused_as_busy_until_one_comes_back() {
    let mut h = Harness::new(LIMITS);
    let first = h.admit(1);
    h.admit(2);
    let emitted = h.assign(assignment(3));
    assert_eq!(&*emitted, [answer(run_of(3), Answer::Refused(Refusal::Busy))]);
    // The first fails to prepare and answers: its slot comes back at the
    // reclaim point, not before.
    let unprepared = Event::Unprepared { owner: first.owner, failure: Preparation::Transient, detail: bytes(b"") };
    assert_eq!(h.step(unprepared).len(), 1);
    assert_eq!(&*h.assign(assignment(3)), [answer(run_of(3), Answer::Refused(Refusal::Busy))]);
    h.model.reclaim();
    h.admit(3);
}

#[test]
fn an_assignment_for_a_run_hosted_already_under_another_attempt_is_refused_as_busy() {
    let mut h = Harness::new(LIMITS);
    h.admit(1);
    let again = Assignment { attempt: Token::new(5000), ..assignment(1) };
    let emitted = h.assign(again);
    let hosted = Names { attempt: Token::new(5000), ..run_of(1) };
    assert_eq!(&*emitted, [answer(hosted, Answer::Refused(Refusal::Busy))]);
}

#[test]
fn the_attempt_hosted_assigned_again_is_dropped() {
    let mut h = Harness::new(LIMITS);
    let hosted = h.admit(1);
    assert!(h.assign(assignment(1)).is_empty(), "its one answer is the hosted run's");
    let invalid = Assignment { charter: Box::from([0_u8; 65]), ..assignment(1) };
    assert!(h.assign(invalid).is_empty(), "whatever it holds");
    assert_eq!(h.model.hosted(), 1);
    let unprepared = Event::Unprepared { owner: hosted.owner, failure: Preparation::Transient, detail: bytes(b"") };
    assert_eq!(h.step(unprepared).len(), 1, "answered once");
}

#[test]
fn a_charter_and_snapshot_of_exactly_the_limits_are_admitted_and_passed_on() {
    let mut h = Harness::new(LIMITS);
    let assignment = Assignment {
        charter: Box::from([7_u8; 64]),
        snapshot: Some(Box::from([8_u8; 32])),
        workspace: Workspace { key: Box::from([1_u8; 16]), ..workspace() },
        ..assignment(1)
    };
    let emitted = h.assign(assignment);
    let [Request::Prepare { owner, .. }] = &*emitted else { panic!("admitted: {emitted:?}") };
    let emitted = h.step(Event::Prepared { owner: *owner, workspace: Token::new(9) });
    let start = Request::Start {
        owner: *owner,
        workspace: Token::new(9),
        charter: Box::from([7_u8; 64]),
        snapshot: Some(Box::from([8_u8; 32])),
    };
    assert_eq!(&*emitted, [start]);
}

// Preparing and Cancelling.

#[test]
fn a_workspace_that_cannot_be_prepared_fails_the_run_with_nothing_to_release() {
    let mut h = Harness::new(LIMITS);
    let hosted = h.admit(1);
    let unprepared =
        Event::Unprepared { owner: hosted.owner, failure: Preparation::Permanent, detail: bytes(b"no such branch") };
    let emitted = h.step(unprepared);
    let failure = Failure::Unprepared(Preparation::Permanent);
    assert_eq!(&*emitted, [answer(hosted, failed(failure, b"h branch", nothing()))], "the detail's tail");
    assert_eq!(h.model.hosted(), 1, "retired, and reclaimed at the reclaim point");
    h.model.reclaim();
    assert_eq!(h.model.hosted(), 0);
}

#[test]
fn a_cancel_as_the_workspace_is_prepared_waits_for_the_prepare_then_releases_it() {
    let mut h = Harness::new(LIMITS);
    let hosted = h.admit(1);
    assert!(h.cancel(hosted).is_empty(), "the prepare in flight is waited for");
    assert!(h.cancel(hosted).is_empty(), "a second cancel changes nothing");
    assert_eq!(&*h.inbound(hosted, b"hi"), [bounced(hosted, Bounce::Ending)]);
    assert_eq!(h.report()[0].phase, Phase::Ending);
    let emitted = h.step(Event::Prepared { owner: hosted.owner, workspace: hosted.workspace });
    let cancelled = failed(Failure::Cancelled(Reason::Engine), b"", nothing());
    assert_eq!(&*emitted, [release(hosted), answer(hosted, cancelled)], "nothing ran: nothing to save");
}

#[test]
fn a_cancel_as_the_workspace_fails_to_prepare_answers_cancelled() {
    let mut h = Harness::new(LIMITS);
    let hosted = h.admit(1);
    assert!(h.cancel(hosted).is_empty());
    let unprepared = Event::Unprepared { owner: hosted.owner, failure: Preparation::Transient, detail: bytes(b"x") };
    let cancelled = failed(Failure::Cancelled(Reason::Engine), b"", nothing());
    assert_eq!(&*h.step(unprepared), [answer(hosted, cancelled)], "the prepare's failure is not the answer's");
}

#[test]
fn inbound_events_before_the_run_is_live_are_held_in_order_and_delivered_as_it_starts() {
    let mut h = Harness::new(LIMITS);
    let hosted = h.admit(1);
    assert!(h.inbound(hosted, b"one").is_empty(), "held");
    let emitted = h.step(Event::Prepared { owner: hosted.owner, workspace: hosted.workspace });
    assert_eq!(emitted.len(), 1, "started");
    assert!(h.inbound(hosted, b"two").is_empty(), "held");
    assert_eq!(&*h.inbound(hosted, b"three"), [bounced(hosted, Bounce::Full)], "past the hold");
    let emitted = h.step(Event::Started { owner: hosted.owner, agent: hosted.agent });
    assert_eq!(&*emitted, [deliver(hosted, b"one"), deliver(hosted, b"two")]);
    assert_eq!(&*h.inbound(hosted, b"four"), [deliver(hosted, b"four")], "live: delivered as it arrives");
}

#[test]
fn an_inbound_event_beyond_the_limits_is_bounced() {
    let mut h = Harness::new(LIMITS);
    let hosted = h.live(1);
    assert_eq!(&*h.inbound(hosted, &[0_u8; 17]), [bounced(hosted, Bounce::TooLarge)]);
    assert_eq!(&*h.inbound(hosted, &[0_u8; 16]), [deliver(hosted, &[0_u8; 16])]);
}

// Starting and Unwanted.

#[test]
fn an_agent_that_cannot_be_started_fails_the_run_and_releases_its_workspace() {
    let mut h = Harness::new(LIMITS);
    let hosted = h.starting(1);
    assert!(h.inbound(hosted, b"held").is_empty());
    let emitted = h.gone(hosted, b"spawn failed: no such file");
    let failure = Failure::Agent(AgentFailure::Unstarted);
    assert_eq!(&*emitted, [release(hosted), answer(hosted, failed(failure, b"uch file", nothing()))]);
}

#[test]
fn a_cancel_as_the_agent_starts_stops_it_once_it_has_started() {
    let mut h = Harness::new(LIMITS);
    let hosted = h.starting(1);
    assert!(h.inbound(hosted, b"held").is_empty());
    assert!(h.cancel(hosted).is_empty(), "the start in flight is waited for");
    assert!(h.cancel(hosted).is_empty());
    assert_eq!(&*h.inbound(hosted, b"late"), [bounced(hosted, Bounce::Ending)]);
    let emitted = h.step(Event::Started { owner: hosted.owner, agent: hosted.agent });
    assert_eq!(&*emitted, [stop(hosted)], "what was held is dropped");
    saving(&mut h, hosted);
    let saved = Event::Saved { owner: hosted.owner, save: Box::new([Landing::Landed, Landing::Unchanged]) };
    let work = Work { landed: Box::new([]), saved: Some(Box::new([Landing::Landed, Landing::Unchanged])) };
    let cancelled = failed(Failure::Cancelled(Reason::Engine), b"", work);
    assert_eq!(&*h.step(saved), [release(hosted), answer(hosted, cancelled)]);
}

#[test]
fn a_cancel_as_the_agent_fails_to_start_answers_cancelled() {
    let mut h = Harness::new(LIMITS);
    let hosted = h.starting(1);
    assert!(h.cancel(hosted).is_empty());
    let cancelled = failed(Failure::Cancelled(Reason::Engine), b"", nothing());
    assert_eq!(&*h.gone(hosted, b""), [release(hosted), answer(hosted, cancelled)]);
}

// Live: Active and Waiting.

#[test]
fn a_run_that_ends_is_stopped_saved_released_and_answered_with_its_outcome() {
    let mut h = Harness::new(LIMITS);
    let hosted = h.live(1);
    let emitted = h.finish(hosted, Finish::Ended { outcome: bytes(b"verdict") });
    assert_eq!(&*emitted, [stop(hosted)]);
    assert_eq!(h.report()[0].phase, Phase::Ending);
    saving(&mut h, hosted);
    let saved = Event::Saved { owner: hosted.owner, save: Box::new([Landing::Unchanged, Landing::Unchanged]) };
    let work = Work { landed: Box::new([]), saved: Some(Box::new([Landing::Unchanged, Landing::Unchanged])) };
    let ended = Answer::Ended { outcome: bytes(b"verdict"), work };
    assert_eq!(&*h.step(saved), [release(hosted), answer(hosted, ended)]);
    let run = (hosted.run, hosted.attempt);
    let facts = [
        Fact::Admitted { run: run.0, attempt: run.1 },
        Fact::Prepared { run: run.0, attempt: run.1 },
        Fact::Started { run: run.0, attempt: run.1 },
        Fact::Ended { run: run.0, attempt: run.1 },
    ];
    assert_eq!(&*h.facts(), facts);
}

#[test]
fn a_run_that_parks_answers_with_its_snapshot() {
    let mut h = Harness::new(LIMITS);
    let hosted = h.live(1);
    let emitted = h.finish(hosted, Finish::Parked { snapshot: Some(bytes(b"state")) });
    assert_eq!(&*emitted, [stop(hosted)]);
    saving(&mut h, hosted);
    let saved = Event::Saved { owner: hosted.owner, save: Box::new([Landing::Failed, Landing::Unchanged]) };
    let work = Work { landed: Box::new([]), saved: Some(Box::new([Landing::Failed, Landing::Unchanged])) };
    let parked = Answer::Parked { snapshot: Some(bytes(b"state")), work };
    assert_eq!(&*h.step(saved), [release(hosted), answer(hosted, parked)]);
    assert_eq!(h.facts().last(), Some(&Fact::Parked { run: hosted.run, attempt: hosted.attempt }));
}

#[test]
fn a_run_that_fails_answers_with_its_failure_and_the_tail_of_its_agents_output() {
    let mut h = Harness::new(LIMITS);
    let hosted = h.live(1);
    let emitted = h.finish(hosted, Finish::Failed { failure: RunFailure::Budget });
    assert_eq!(&*emitted, [stop(hosted)]);
    assert_eq!(&*h.gone(hosted, b"budget exhausted"), [save(hosted)]);
    let saved = Event::Saved { owner: hosted.owner, save: Box::new([Landing::Landed, Landing::Moved]) };
    let work = Work { landed: Box::new([]), saved: Some(Box::new([Landing::Landed, Landing::Moved])) };
    let budget = failed(Failure::Run(RunFailure::Budget), b"xhausted", work);
    assert_eq!(&*h.step(saved), [release(hosted), answer(hosted, budget)]);
}

#[test]
fn a_run_that_says_more_than_the_limits_allow_has_broken_the_rules() {
    for finish in
        [Finish::Ended { outcome: Box::from([0_u8; 33]) }, Finish::Parked { snapshot: Some(Box::from([0_u8; 33])) }]
    {
        let mut h = Harness::new(Limits { held: 0, ..LIMITS });
        let hosted = h.live(1);
        assert_eq!(&*h.finish(hosted, finish), [stop(hosted)]);
        saving(&mut h, hosted);
        let saved = Event::Saved { owner: hosted.owner, save: Box::new([Landing::Unchanged; 2]) };
        let rules = failed(
            Failure::Agent(AgentFailure::Rules),
            b"",
            Work { landed: Box::new([]), saved: Some(Box::new([Landing::Unchanged; 2])) },
        );
        assert_eq!(&*h.step(saved), [release(hosted), answer(hosted, rules)]);
    }
}

#[test]
fn an_agent_that_exits_without_a_word_fails_the_run_and_is_not_stopped() {
    let mut h = Harness::new(LIMITS);
    let hosted = h.live(1);
    let relayed = h.relay(hosted, 7);
    let emitted = h.gone(hosted, b"segfault");
    assert_eq!(&*emitted, [reply(hosted, 7, Reply::Unavailable), save(hosted)], "its call is answered as it leaves");
    let answer_late = Event::Relayed { run: hosted.run, attempt: hosted.attempt, call: relayed, answer: bytes(b"a") };
    assert!(h.step(answer_late).is_empty(), "the engine's late answer is dropped");
    let saved = Event::Saved { owner: hosted.owner, save: Box::new([Landing::Unchanged, Landing::Unchanged]) };
    let work = Work { landed: Box::new([]), saved: Some(Box::new([Landing::Unchanged, Landing::Unchanged])) };
    let exited = failed(Failure::Agent(AgentFailure::Exited), b"segfault", work);
    assert_eq!(&*h.step(saved), [release(hosted), answer(hosted, exited)]);
}

#[test]
fn a_fault_of_the_agent_stops_it_and_fails_the_run() {
    for fault in [AgentFailure::Rules, AgentFailure::NoProgress, AgentFailure::WallTime] {
        let mut h = Harness::new(LIMITS);
        let hosted = h.live(1);
        assert_eq!(&*h.step(Event::Faulted { owner: hosted.owner, fault }), [stop(hosted)]);
        assert!(h.step(Event::Faulted { owner: hosted.owner, fault: AgentFailure::WallTime }).is_empty());
        assert!(h.step(Event::Yielded { owner: hosted.owner }).is_empty());
        assert_eq!(&*h.gone(hosted, b"killed"), [save(hosted)], "it said nothing: the fault is the answer");
        let saved = Event::Saved { owner: hosted.owner, save: Box::new([Landing::Unchanged, Landing::Unchanged]) };
        let work = Work { landed: Box::new([]), saved: Some(Box::new([Landing::Unchanged, Landing::Unchanged])) };
        assert_eq!(&*h.step(saved), [release(hosted), answer(hosted, failed(Failure::Agent(fault), b"killed", work))]);
    }
}

#[test]
fn a_run_that_yields_waits_and_an_inbound_event_wakes_it() {
    let mut h = Harness::new(LIMITS);
    let hosted = h.live(1);
    assert_eq!(h.report()[0].phase, Phase::Active);
    assert!(h.step(Event::Yielded { owner: hosted.owner }).is_empty());
    assert!(h.step(Event::Yielded { owner: hosted.owner }).is_empty());
    assert_eq!(h.report()[0].phase, Phase::Waiting);
    assert_eq!(&*h.inbound(hosted, b"next"), [deliver(hosted, b"next")]);
    assert_eq!(h.report()[0].phase, Phase::Active);
    assert!(h.step(Event::Yielded { owner: hosted.owner }).is_empty());
    h.relay(hosted, 7);
    assert_eq!(h.report()[0].phase, Phase::Active, "a run that calls is at work");
    assert!(h.step(Event::Yielded { owner: hosted.owner }).is_empty());
    assert_eq!(&*h.cancel(hosted), [reply(hosted, 7, Reply::Unavailable), stop(hosted)], "waiting, as active");
}

// Host calls.

#[test]
fn a_relayed_call_goes_to_the_engine_and_its_answer_back_to_the_run() {
    let mut h = Harness::new(LIMITS);
    let hosted = h.live(1);
    let call = h.relay(hosted, 7);
    assert_eq!(h.model.calls(), 1);
    let stale = Event::Relayed { run: hosted.run, attempt: Token::new(9), call, answer: bytes(b"old") };
    assert!(h.step(stale).is_empty(), "a stale attempt never acts");
    let unknown = Event::Relayed { run: Token::new(9), attempt: hosted.attempt, call, answer: bytes(b"old") };
    assert!(h.step(unknown).is_empty(), "nor a run not hosted");
    let relayed = Event::Relayed { run: hosted.run, attempt: hosted.attempt, call, answer: bytes(b"page") };
    assert_eq!(&*h.step(relayed), [reply(hosted, 7, Reply::Relayed { answer: bytes(b"page") })]);
    let again = Event::Relayed { run: hosted.run, attempt: hosted.attempt, call, answer: bytes(b"page") };
    assert!(h.step(again).is_empty(), "a call is answered once");
    h.model.reclaim();
    assert_eq!(h.model.calls(), 0);
}

#[test]
fn an_answer_for_a_call_of_another_run_is_dropped() {
    let mut h = Harness::new(LIMITS);
    let first = h.live(1);
    let second = h.live(2);
    let call = h.relay(first, 7);
    let wrong = Event::Relayed { run: second.run, attempt: second.attempt, call, answer: bytes(b"x") };
    assert!(h.step(wrong).is_empty());
    let push = h.push(first, 8);
    let wrong = Event::Relayed { run: first.run, attempt: first.attempt, call: push, answer: bytes(b"x") };
    assert!(h.step(wrong).is_empty(), "a push is not the engine's to answer");
}

#[test]
fn a_push_is_served_through_the_workspace_and_the_run_told_how_it_went() {
    let cases = [
        ([Landing::Landed, Landing::Unchanged], Push::Done),
        ([Landing::Landed, Landing::Moved], Push::Moved),
        ([Landing::Failed, Landing::Moved], Push::Moved),
        ([Landing::Failed, Landing::Landed], Push::Failed),
        ([Landing::Unchanged, Landing::Unchanged], Push::Nothing),
    ];
    for (push, told) in cases {
        let mut h = Harness::new(LIMITS);
        let hosted = h.live(1);
        let owner = h.push(hosted, 7);
        let emitted = h.step(Event::Pushed { owner, push: Box::new(push) });
        assert_eq!(&*emitted, [reply(hosted, 7, Reply::Pushed(told))]);
    }
}

#[test]
fn a_run_that_landed_a_change_has_nothing_to_save_and_says_what_landed() {
    let mut h = Harness::new(LIMITS);
    let hosted = h.live(1);
    let owner = h.push(hosted, 7);
    let push = Event::Pushed { owner, push: Box::new([Landing::Unchanged, Landing::Landed]) };
    assert_eq!(&*h.step(push), [reply(hosted, 7, Reply::Pushed(Push::Done))]);
    assert_eq!(&*h.finish(hosted, Finish::Ended { outcome: bytes(b"pr") }), [stop(hosted)]);
    let work = Work { landed: Box::new([1]), saved: None };
    let ended = Answer::Ended { outcome: bytes(b"pr"), work };
    assert_eq!(&*h.gone(hosted, b""), [release(hosted), answer(hosted, ended)], "it ended with a landed change");
}

#[test]
fn a_run_without_a_save_directive_is_released_once_stopped() {
    let mut h = Harness::new(LIMITS);
    let emitted = h.assign(Assignment { save: None, ..assignment(1) });
    let [Request::Prepare { owner, .. }] = &*emitted else { panic!("admitted: {emitted:?}") };
    let hosted = Names { owner: *owner, ..run_of(1) };
    assert_eq!(h.step(Event::Prepared { owner: hosted.owner, workspace: hosted.workspace }).len(), 1);
    assert!(h.step(Event::Started { owner: hosted.owner, agent: hosted.agent }).is_empty());
    assert_eq!(&*h.cancel(hosted), [stop(hosted)]);
    let cancelled = failed(Failure::Cancelled(Reason::Engine), b"bye", nothing());
    assert_eq!(&*h.gone(hosted, b"bye"), [release(hosted), answer(hosted, cancelled)]);
}

#[test]
fn calls_beyond_the_runs_limit_and_a_second_push_are_busy() {
    let mut h = Harness::new(LIMITS);
    let hosted = h.live(1);
    let push = h.push(hosted, 7);
    assert_eq!(&*h.call(hosted, 8, Ask::Push { message: bytes(b"m") }), [reply(hosted, 8, Reply::Busy)]);
    h.relay(hosted, 9);
    assert_eq!(&*h.call(hosted, 10, Ask::Relay { body: bytes(b"b") }), [reply(hosted, 10, Reply::Busy)]);
    let pushed = Event::Pushed { owner: push, push: Box::new([Landing::Failed, Landing::Unchanged]) };
    assert_eq!(&*h.step(pushed), [reply(hosted, 7, Reply::Pushed(Push::Failed))]);
    // The push's call keeps its slot until the reclaim point: the slab is
    // full for now, though the run has room.
    let tight = Limits { slots: 1, ..LIMITS };
    let mut h = Harness::new(tight);
    let hosted = h.live(1);
    let first = h.relay(hosted, 7);
    h.relay(hosted, 8);
    let relayed = Event::Relayed { run: hosted.run, attempt: hosted.attempt, call: first, answer: bytes(b"a") };
    assert_eq!(h.step(relayed).len(), 1);
    assert_eq!(&*h.call(hosted, 9, Ask::Relay { body: bytes(b"b") }), [reply(hosted, 9, Reply::Busy)]);
    h.model.reclaim();
    h.relay(hosted, 10);
}

#[test]
fn a_cancelled_run_answers_its_relayed_calls_as_unavailable_and_waits_for_its_push() {
    let mut h = Harness::new(LIMITS);
    let hosted = h.live(1);
    let push = h.push(hosted, 7);
    let relayed = h.relay(hosted, 8);
    let emitted = h.cancel(hosted);
    assert_eq!(&*emitted, [reply(hosted, 8, Reply::Unavailable), stop(hosted)]);
    assert!(h.cancel(hosted).is_empty(), "decided already");
    assert_eq!(&*h.call(hosted, 9, Ask::Relay { body: bytes(b"b") }), [reply(hosted, 9, Reply::Unavailable)]);
    let late = Event::Relayed { run: hosted.run, attempt: hosted.attempt, call: relayed, answer: bytes(b"a") };
    assert!(h.step(late).is_empty());
    assert!(h.gone(hosted, b"").is_empty(), "the push in flight still touches the workspace");
    let pushed = Event::Pushed { owner: push, push: Box::new([Landing::Landed, Landing::Unchanged]) };
    let emitted = h.step(pushed);
    assert_eq!(&*emitted, [reply(hosted, 7, Reply::Pushed(Push::Done)), save(hosted)], "told how it went; saved");
    let saved = Event::Saved { owner: hosted.owner, save: Box::new([Landing::Unchanged, Landing::Unchanged]) };
    let work = Work { landed: Box::new([0]), saved: Some(Box::new([Landing::Unchanged, Landing::Unchanged])) };
    let cancelled = failed(Failure::Cancelled(Reason::Engine), b"", work);
    assert_eq!(&*h.step(saved), [release(hosted), answer(hosted, cancelled)], "it did not end with its change");
}

#[test]
fn a_push_that_settles_before_the_agent_goes_leaves_the_tail_to_the_agent() {
    let mut h = Harness::new(LIMITS);
    let hosted = h.live(1);
    let push = h.push(hosted, 7);
    assert_eq!(&*h.step(Event::Faulted { owner: hosted.owner, fault: AgentFailure::NoProgress }), [stop(hosted)]);
    let pushed = Event::Pushed { owner: push, push: Box::new([Landing::Moved, Landing::Unchanged]) };
    let told = reply(hosted, 7, Reply::Pushed(Push::Moved));
    assert_eq!(&*h.step(pushed), [told], "told how it went, and the agent is still there");
    assert_eq!(&*h.gone(hosted, b""), [save(hosted)], "nothing landed");
}

#[test]
fn inbound_events_and_calls_to_a_stopping_or_saving_run_are_bounced_and_unavailable() {
    let mut h = Harness::new(LIMITS);
    let hosted = h.live(1);
    assert_eq!(&*h.finish(hosted, Finish::Parked { snapshot: None }), [stop(hosted)]);
    assert_eq!(&*h.inbound(hosted, b"x"), [bounced(hosted, Bounce::Ending)]);
    saving(&mut h, hosted);
    assert_eq!(&*h.inbound(hosted, b"x"), [bounced(hosted, Bounce::Ending)]);
    assert!(h.cancel(hosted).is_empty());
    assert_eq!(h.report()[0].phase, Phase::Ending);
}

// Fencing.

#[test]
fn what_the_engine_sends_for_another_attempt_or_run_is_dropped() {
    let mut h = Harness::new(LIMITS);
    let hosted = h.live(1);
    let stale = Names { attempt: Token::new(42), ..hosted };
    assert!(h.inbound(stale, b"x").is_empty());
    assert!(h.cancel(stale).is_empty());
    let unknown = Names { run: Token::new(42), ..hosted };
    assert!(h.inbound(unknown, b"x").is_empty());
    assert!(h.cancel(unknown).is_empty());
    assert_eq!(h.report()[0].phase, Phase::Active, "nothing changed");
}

// Cancelling every run, and the report.

#[test]
fn cancel_all_cancels_every_run_one_at_a_time_for_its_reason() {
    let mut h = Harness::new(LIMITS);
    let first = h.admit(1);
    let second = h.live(2);
    assert!(!h.model.is_ready());
    assert!(h.step(Event::CancelAll { reason: Reason::Contact }).is_empty());
    assert!(h.step(Event::CancelAll { reason: Reason::Shutdown }).is_empty(), "the first reason stands");
    assert!(h.model.is_ready());
    assert!(h.resume().is_empty(), "the first is preparing: its prepare is waited for");
    assert_eq!(&*h.resume(), [stop(second)]);
    assert!(!h.model.is_ready());
    let emitted = h.step(Event::Prepared { owner: first.owner, workspace: first.workspace });
    let contact = failed(Failure::Cancelled(Reason::Contact), b"", nothing());
    assert_eq!(&*emitted, [release(first), answer(first, contact)]);
    assert_eq!(&*h.gone(second, b""), [save(second)], "work is saved first");
}

#[test]
fn a_run_that_answers_before_its_turn_on_the_ready_list_leaves_it() {
    let mut h = Harness::new(LIMITS);
    let first = h.admit(1);
    let second = h.admit(2);
    assert!(h.step(Event::CancelAll { reason: Reason::Shutdown }).is_empty());
    let unprepared = Event::Unprepared { owner: first.owner, failure: Preparation::Transient, detail: bytes(b"") };
    assert_eq!(h.step(unprepared).len(), 1);
    assert!(h.resume().is_empty(), "the second is cancelled");
    assert!(!h.model.is_ready(), "the first left the list as it answered");
    let emitted = h.step(Event::Prepared { owner: second.owner, workspace: second.workspace });
    let shutdown = failed(Failure::Cancelled(Reason::Shutdown), b"", nothing());
    assert_eq!(&*emitted, [release(second), answer(second, shutdown)]);
}

#[test]
fn the_report_lists_every_hosted_run_by_name_with_its_phase() {
    let mut h = Harness::new(Limits { slots: 4, ..LIMITS });
    assert!(h.report().is_empty());
    let waiting = h.live(5);
    assert!(h.step(Event::Yielded { owner: waiting.owner }).is_empty());
    h.admit(3);
    h.starting(9);
    let ending = h.live(7);
    assert_eq!(h.cancel(ending).len(), 1);
    let phases: [(u64, Phase); 4] =
        [(3, Phase::Preparing), (5, Phase::Waiting), (7, Phase::Ending), (9, Phase::Starting)];
    let mut expected = List::with_capacity(4);
    for (run, phase) in phases {
        let hosting = Hosting { run: Token::new(run), attempt: token(run, 1000), phase };
        expected.push(hosting).expect("room");
    }
    assert_eq!(h.report(), expected.into_boxed());
}

// Facts and memory.

#[test]
fn facts_that_do_not_fit_are_dropped_and_counted_and_change_nothing() {
    let mut h = Harness::new(Limits { facts: 1, ..LIMITS });
    let hosted = h.live(1);
    assert_eq!(h.model.facts_lost(), 2);
    assert_eq!(&*h.cancel(hosted), [stop(hosted)]);
    assert_eq!(&*h.facts(), [Fact::Admitted { run: hosted.run, attempt: hosted.attempt }]);
}

#[test]
fn a_failed_run_tells_its_failure() {
    let mut h = Harness::new(LIMITS);
    let hosted = h.admit(1);
    let unprepared = Event::Unprepared { owner: hosted.owner, failure: Preparation::Transient, detail: bytes(b"") };
    assert_eq!(h.step(unprepared).len(), 1);
    let failure = Failure::Unprepared(Preparation::Transient);
    assert_eq!(h.facts().last(), Some(&Fact::Failed { run: hosted.run, attempt: hosted.attempt, failure }));
}

#[test]
fn every_slot_comes_back_once_every_run_has_answered() {
    let mut h = Harness::new(LIMITS);
    let first = h.live(1);
    let second = h.live(2);
    assert_eq!(&*h.finish(first, Finish::Failed { failure: RunFailure::Stale }), [stop(first)]);
    assert_eq!(&*h.finish(second, Finish::Failed { failure: RunFailure::Model }), [stop(second)]);
    saving(&mut h, first);
    saving(&mut h, second);
    for hosted in [first, second] {
        assert_eq!(h.step(Event::Saved { owner: hosted.owner, save: Box::new([Landing::Unchanged; 2]) }).len(), 2);
    }
    h.model.reclaim();
    assert_eq!((h.model.hosted(), h.model.calls()), (0, 0));
    assert!(h.report().is_empty());
}

#[test]
fn max_out_covers_the_hold_and_the_calls() {
    assert_eq!(max_out(&LIMITS), 4);
    assert_eq!(max_out(&Limits { held: 10, ..LIMITS }), 12);
    assert_eq!(max_out(&Limits { run_calls: 7, ..LIMITS }), 9);
}

#[test]
fn the_worst_case_is_bounded_or_refused() {
    let bound = worst_case(&LIMITS).expect("the test limits fit");
    assert!(bound > LIMITS.charter_bytes.saturating_add(LIMITS.snapshot_bytes), "it counts what a run holds");
    let more = worst_case(&Limits { slots: 4, ..LIMITS }).expect("fits");
    assert!(more > bound, "a slot more is more");
    assert_eq!(worst_case(&Limits { charter_bytes: u64::MAX, ..LIMITS }), None);
    assert_eq!(worst_case(&Limits { slots: u32::MAX, run_calls: u32::MAX, ..LIMITS }), None);
}

// Saving, and the run's own ending.

#[test]
fn a_run_that_landed_a_change_mid_run_and_parks_still_saves() {
    let mut h = Harness::new(LIMITS);
    let hosted = h.live(1);
    let owner = h.push(hosted, 7);
    let push = Event::Pushed { owner, push: Box::new([Landing::Landed, Landing::Unchanged]) };
    assert_eq!(h.step(push).len(), 1);
    assert_eq!(&*h.finish(hosted, Finish::Parked { snapshot: None }), [stop(hosted)]);
    assert_eq!(&*h.gone(hosted, b""), [save(hosted)], "it did not end with its change");
    let saved = Event::Saved { owner: hosted.owner, save: Box::new([Landing::Unchanged, Landing::Unchanged]) };
    let work = Work { landed: Box::new([0]), saved: Some(Box::new([Landing::Unchanged, Landing::Unchanged])) };
    let parked = Answer::Parked { snapshot: None, work };
    assert_eq!(&*h.step(saved), [release(hosted), answer(hosted, parked)]);
}

#[test]
fn a_cancelled_run_that_lands_its_change_as_it_winds_down_ends_with_it() {
    let mut h = Harness::new(LIMITS);
    let hosted = h.live(1);
    let owner = h.push(hosted, 7);
    assert_eq!(&*h.cancel(hosted), [stop(hosted)]);
    let push = Event::Pushed { owner, push: Box::new([Landing::Landed, Landing::Unchanged]) };
    assert_eq!(&*h.step(push), [reply(hosted, 7, Reply::Pushed(Push::Done))]);
    assert!(h.finish(hosted, Finish::Ended { outcome: bytes(b"pr") }).is_empty(), "its own ending wins");
    assert!(h.finish(hosted, Finish::Parked { snapshot: None }).is_empty(), "said once");
    assert!(h.cancel(hosted).is_empty());
    let ended = Answer::Ended { outcome: bytes(b"pr"), work: Work { landed: Box::new([0]), saved: None } };
    assert_eq!(&*h.gone(hosted, b"bye"), [release(hosted), answer(hosted, ended)], "nothing left to save");
}

#[test]
fn a_stopped_run_that_says_how_it_finishes_is_answered_as_it_says() {
    let cases: [(bool, Finish, Failure); 4] = [
        (true, Finish::Failed { failure: RunFailure::Cancelled }, Failure::Cancelled(Reason::Engine)),
        (false, Finish::Failed { failure: RunFailure::Cancelled }, Failure::Agent(AgentFailure::NoProgress)),
        (true, Finish::Failed { failure: RunFailure::Budget }, Failure::Run(RunFailure::Budget)),
        (false, Finish::Ended { outcome: Box::from([0_u8; 33]) }, Failure::Agent(AgentFailure::Rules)),
    ];
    for (cancelled, finish, failure) in cases {
        let mut h = Harness::new(LIMITS);
        let hosted = h.live(1);
        let stopped = if cancelled {
            h.cancel(hosted)
        } else {
            h.step(Event::Faulted { owner: hosted.owner, fault: AgentFailure::NoProgress })
        };
        assert_eq!(&*stopped, [stop(hosted)]);
        assert!(h.finish(hosted, finish).is_empty());
        assert_eq!(&*h.gone(hosted, b"bye"), [save(hosted)]);
        let saved = Event::Saved { owner: hosted.owner, save: Box::new([Landing::Unchanged, Landing::Unchanged]) };
        let work = Work { landed: Box::new([]), saved: Some(Box::new([Landing::Unchanged, Landing::Unchanged])) };
        assert_eq!(&*h.step(saved), [release(hosted), answer(hosted, failed(failure, b"bye", work))]);
    }
    // A cancel the run reports while live is its own.
    let mut h = Harness::new(LIMITS);
    let hosted = h.live(1);
    assert_eq!(&*h.finish(hosted, Finish::Failed { failure: RunFailure::Cancelled }), [stop(hosted)]);
    assert!(h.cancel(hosted).is_empty(), "decided already");
    assert_eq!(&*h.gone(hosted, b""), [save(hosted)]);
    let saved = Event::Saved { owner: hosted.owner, save: Box::new([Landing::Unchanged, Landing::Unchanged]) };
    let work = Work { landed: Box::new([]), saved: Some(Box::new([Landing::Unchanged, Landing::Unchanged])) };
    let own = failed(Failure::Run(RunFailure::Cancelled), b"", work);
    assert_eq!(&*h.step(saved), [release(hosted), answer(hosted, own)]);
}

#[test]
fn a_run_cancelled_as_it_started_may_still_park() {
    let mut h = Harness::new(LIMITS);
    let hosted = h.starting(1);
    assert!(h.cancel(hosted).is_empty());
    assert_eq!(&*h.step(Event::Started { owner: hosted.owner, agent: hosted.agent }), [stop(hosted)]);
    assert!(h.finish(hosted, Finish::Parked { snapshot: Some(bytes(b"s")) }).is_empty());
    assert_eq!(&*h.gone(hosted, b""), [save(hosted)]);
    let saved = Event::Saved { owner: hosted.owner, save: Box::new([Landing::Unchanged, Landing::Unchanged]) };
    let work = Work { landed: Box::new([]), saved: Some(Box::new([Landing::Unchanged, Landing::Unchanged])) };
    assert_eq!(
        &*h.step(saved),
        [release(hosted), answer(hosted, Answer::Parked { snapshot: Some(bytes(b"s")), work })]
    );
}

// Shutdown, and the names of repositories.

#[test]
fn a_worker_shutting_down_admits_no_more_runs() {
    let mut h = Harness::new(LIMITS);
    assert!(h.step(Event::CancelAll { reason: Reason::Contact }).is_empty());
    h.admit(1);
    assert!(h.step(Event::CancelAll { reason: Reason::Shutdown }).is_empty());
    assert_eq!(&*h.assign(assignment(2)), [answer(run_of(2), Answer::Refused(Refusal::Busy))]);
    assert_eq!(h.model.hosted(), 1);
}

#[test]
fn a_repository_name_that_is_not_one_path_component_is_refused() {
    let names: [&[u8]; 7] = [b".", b"..", b".git", b".GiT", b"a/b", b"a\0b", b"/"];
    for name in names {
        let mut h = Harness::new(LIMITS);
        let repositories = Box::new([repository(name)]);
        let assignment = Assignment { workspace: Workspace { key: bytes(b"k"), repositories }, ..assignment(1) };
        let refused = answer(run_of(1), Answer::Refused(Refusal::Invalid(Invalid::Name)));
        assert_eq!(&*h.assign(assignment), [refused], "{name:?}");
    }
    for remote in [&b""[..], &[b'r'; 17][..]] {
        let mut h = Harness::new(LIMITS);
        let repositories = Box::new([Repository { remote: Box::from(remote), ..repository(b"a") }]);
        let assignment = Assignment { workspace: Workspace { key: bytes(b"k"), repositories }, ..assignment(1) };
        let refused = answer(run_of(1), Answer::Refused(Refusal::Invalid(Invalid::Name)));
        assert_eq!(&*h.assign(assignment), [refused]);
    }
    for name in [&b".github"[..], &b"git"[..], &b"..."[..]] {
        let mut h = Harness::new(LIMITS);
        let repositories = Box::new([repository(name)]);
        let assignment = Assignment { workspace: Workspace { key: bytes(b"k"), repositories }, ..assignment(1) };
        assert_eq!(h.assign(assignment).len(), 1);
        assert_eq!(h.model.hosted(), 1, "{name:?} is a name");
    }
}
