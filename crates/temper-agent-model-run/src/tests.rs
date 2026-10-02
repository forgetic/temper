//! Feed the model events, inspect the requests that come out.

use alloc::boxed::Box;

use temper_lib::{Duration, Env, List, Queue, ReplyTo, Time, Token};

use crate::charter::{Checkout, Endpoint, Grants, Llm, Outlet, Repository, Tools};
use crate::outcome::{
    Change, ChangeSpec, Child, Children, Declared, Field, OutcomeSpec, Problem, Problems, Verdict, VerdictRule,
};
use crate::prepare::{Found, Guide};
use crate::{
    Answer, Ask, Budget, Charter, End, Event, Exhausted, Exit, Failure, Fault, Invalid, Limits, MAX_OUT, Model,
    Opening, Place, Policy, Push, Ran, Read, Refusal, Request, Returned, Spend, Stop, fire, step, worst_case,
};

const BUDGET: Budget = Budget {
    turns: 10,
    input: 10_000,
    output: 2_000,
    cache_read: 1_000,
    cache_write: 1_000,
    time: Duration::from_secs(600),
};

const LIMITS: Limits = Limits {
    runs: 2,
    conversations: 2,
    run_bytes: 4096,
    repositories: 2,
    outlets: 2,
    verdicts: 2,
    calls: 2,
    budget: Budget {
        turns: 100,
        input: 1_000_000,
        output: 100_000,
        cache_read: 1_000_000,
        cache_write: 1_000_000,
        time: Duration::from_secs(3600),
    },
    max_tokens: 4096,
    nudges: 2,
    guide_bytes: 64,
    io_timeout: Duration::from_secs(10),
    outcome_bytes: 1024,
    check_timeout: Duration::from_secs(300),
    check_tail: 4096,
};

/// The model, its environment, and room for one step's output.
struct Harness {
    model: Model,
    env: Env<Limits>,
    out: Queue<Request>,
}

impl Harness {
    fn new(limits: Limits) -> Harness {
        Harness { model: Model::new(&limits), env: Env { now: Time::ZERO, limits }, out: Queue::with_capacity(MAX_OUT) }
    }

    /// Steps `event`, returning what it emitted, oldest first.
    fn step(&mut self, event: Event) -> Box<[Request]> {
        step(&mut self.model, &self.env, event, &mut self.out);
        self.drain()
    }

    fn fire(&mut self) -> Box<[Request]> {
        assert!(self.model.is_due(self.env.now), "an alarm is due");
        fire(&mut self.model, &self.env, &mut self.out);
        self.drain()
    }

    fn drain(&mut self) -> Box<[Request]> {
        let mut requests = List::with_capacity(MAX_OUT);
        for _ in 0..MAX_OUT {
            let Some(request) = self.out.pop() else { break };
            requests.push(request).expect("room for MAX_OUT");
        }
        assert!(self.out.is_empty(), "a step emits at most MAX_OUT");
        requests.into_boxed()
    }

    /// Starts a run of `charter` for call `call`: what it emitted.
    fn start(&mut self, call: u64, charter: Charter) -> Box<[Request]> {
        let reply_to = ReplyTo::new(Token::new(call));
        self.step(Event::Start { reply_to, worker: Token::new(call), charter })
    }

    /// Starts a run of the test charter for call `call`, which is admitted:
    /// the run's token, once it has looked for its one repository's guide.
    fn prepare(&mut self, call: u64) -> Token {
        let emitted = self.start(call, charter());
        let [Request::Admitted { worker, run }, Request::Read { owner, .. }] = &*emitted else {
            panic!("expected an admitted run, got {emitted:?}");
        };
        assert_eq!((*worker, owner), (Token::new(call), run));
        *run
    }

    /// Starts a run of the test charter for call `call`, which is admitted and
    /// finds no guide: the run's token and its main conversation's.
    fn admit(&mut self, call: u64) -> (Token, Token) {
        let run = self.prepare(call);
        let emitted = self.step(Event::Read { owner: run, read: Read::Missing });
        let [Request::Open { conversation, opening: _ }] = &*emitted else {
            panic!("expected main to open, got {emitted:?}");
        };
        (run, *conversation)
    }

    /// Admits a run for call `call` and starts its main conversation as
    /// `peer`: the run's token and main's.
    fn running(&mut self, call: u64, peer: u64) -> (Token, Token) {
        let (run, conversation) = self.admit(call);
        assert!(self.step(Event::Started { conversation, peer: Token::new(peer) }).is_empty(), "starting is quiet");
        (run, conversation)
    }

    fn after(&mut self, span: Duration) {
        self.env.now = self.env.now.checked_add(span).expect("the test stays in range");
    }
}

pub(crate) fn bytes(text: &[u8]) -> Box<[u8]> {
    Box::from(text)
}

pub(crate) fn rule(name: &[u8], min: u32, max: u32) -> VerdictRule {
    VerdictRule {
        name: bytes(name),
        children: Children { min, max },
        kinds: Box::new([bytes(b"blocking"), bytes(b"nit")]),
        fields: Box::new([bytes(b"path"), bytes(b"body")]),
    }
}

pub(crate) fn charter() -> Charter {
    Charter {
        brief: bytes(b"Review the change."),
        checkout: Checkout {
            repositories: Box::new([Repository { name: bytes(b"temper"), root: Token::new(900), writable: false }]),
        },
        grants: Grants {
            tools: Tools { inspect: true, modify: false, shell: true },
            forge: true,
            agents: false,
            outlets: Box::new([Outlet { name: bytes(b"comment") }]),
        },
        outcome: OutcomeSpec { change: None, verdicts: Box::new([rule(b"approve", 0, 0), rule(b"request", 1, 8)]) },
        budget: BUDGET,
        llm: Llm { endpoint: Endpoint(1), model: bytes(b"model-a"), max_tokens: 1024 },
    }
}

fn spend(input: u64) -> Spend {
    Spend { turns: 1, input, output: 10, cache_read: 0, cache_write: 0 }
}

/// What a run answered, and to which call.
fn answered(emitted: Box<[Request]>) -> (u64, Answer) {
    let Ok(one) = Box::<[Request; 1]>::try_from(emitted) else {
        panic!("expected one request");
    };
    let [Request::Answer { to, answer }] = *one else {
        panic!("expected an answer");
    };
    (to.into_token().raw(), answer)
}

fn failed(failure: Failure, spent: Spend) -> Answer {
    Answer::Failed { failure, spent }
}

#[test]
fn an_admitted_run_reads_its_checkout_then_opens_main_with_the_whole_budget() {
    let mut h = Harness::new(LIMITS);
    let emitted = h.start(7, charter());
    let [Request::Admitted { worker, run }, Request::Read { owner, at, max, deadline }] = &*emitted else {
        panic!("expected an admitted run, got {emitted:?}");
    };
    assert_eq!((*worker, owner), (Token::new(7), run));
    assert_eq!(at, &Place { root: Token::new(900), path: bytes(b"AGENTS.md") });
    assert_eq!((*max, *deadline), (LIMITS.guide_bytes, Time::ZERO.saturating_add(LIMITS.io_timeout)));
    assert_eq!((h.model.runs(), h.model.conversations()), (1, 1), "main has its slot from the start");

    let read = Read::Bytes { bytes: bytes(b"Run the tests."), whole: true };
    let emitted = h.step(Event::Read { owner: *run, read });
    let [Request::Open { conversation: _, opening }] = &*emitted else {
        panic!("expected main to open, got {emitted:?}");
    };
    let mut found = Found::with_capacity(1);
    found.guides.push(Guide { repository: 0, text: bytes(b"Run the tests."), whole: true }).expect("room");
    let expected = Opening {
        llm: charter().llm,
        system: super::prompt::system(&charter(), &found),
        prompt: bytes(super::prompt::BEGIN),
        tools: charter().grants.tools,
        checkout: charter().checkout,
        budget: BUDGET,
        finish: true,
    };
    assert_eq!(opening, &expected);
    assert_eq!(h.model.next_deadline(), Some(Time::ZERO.saturating_add(BUDGET.time)));
}

#[test]
fn a_run_looks_for_checks_in_writable_repositories_when_a_change_must_pass_them() {
    let mut h = Harness::new(LIMITS);
    let checkout = Checkout {
        repositories: Box::new([
            Repository { name: bytes(b"temper"), root: Token::new(900), writable: true },
            Repository { name: bytes(b"docs"), root: Token::new(901), writable: false },
        ]),
    };
    let outcome = OutcomeSpec { change: Some(ChangeSpec { checks: true }), verdicts: Box::new([]) };
    let emitted = h.start(1, Charter { checkout, outcome, ..charter() });
    let [Request::Admitted { run, .. }, Request::Read { at, .. }] = &*emitted else {
        panic!("expected a read, got {emitted:?}");
    };
    let run = *run;
    assert_eq!(at.root, Token::new(900));
    let emitted = h.step(Event::Read { owner: run, read: Read::Failed });
    let [Request::Probe { owner, at, deadline }] = &*emitted else {
        panic!("expected a probe, got {emitted:?}");
    };
    assert_eq!((owner, at), (&run, &Place { root: Token::new(900), path: bytes(b".temper/pre-pr") }));
    assert_eq!(*deadline, Time::ZERO.saturating_add(LIMITS.io_timeout));
    let emitted = h.step(Event::Probed { owner: run, executable: true });
    let [Request::Read { at, .. }] = &*emitted else {
        panic!("expected a read, got {emitted:?}");
    };
    assert_eq!(at.root, Token::new(901), "a read-only repository has no checks to look for");
    let emitted = h.step(Event::Read { owner: run, read: Read::Missing });
    let [Request::Open { opening, .. }] = &*emitted else {
        panic!("expected main to open, got {emitted:?}");
    };
    let system = &opening.system;
    let marked = b"- `temper`, which you may change, with checks (`.temper/pre-pr`)\n";
    assert!(temper_lib::bytes::find(system, marked).is_some(), "the checkout says which has checks");
}

#[test]
fn a_run_with_nothing_to_look_for_opens_main_at_once() {
    let mut h = Harness::new(LIMITS);
    let emitted = h.start(1, Charter { checkout: Checkout { repositories: Box::new([]) }, ..charter() });
    let [Request::Admitted { .. }, Request::Open { .. }] = &*emitted else {
        panic!("expected an admitted run and main, got {emitted:?}");
    };
}

#[test]
fn starts_beyond_the_run_or_conversation_slots_are_refused_as_busy() {
    let mut h = Harness::new(Limits { runs: 1, ..LIMITS });
    let _: (Token, Token) = h.admit(1);
    assert_eq!(answered(h.start(2, charter())), (2, Answer::Refused(Refusal::Busy)));

    let mut h = Harness::new(Limits { conversations: 1, ..LIMITS });
    let _: (Token, Token) = h.admit(1);
    assert_eq!(answered(h.start(2, charter())), (2, Answer::Refused(Refusal::Busy)));
    assert_eq!(h.model.runs(), 1);

    // A charter that can never fit is invalid, room or not.
    let never = Charter { budget: Budget { turns: 0, ..BUDGET }, ..charter() };
    assert_eq!(answered(h.start(3, never)), (3, Answer::Refused(Refusal::Invalid(Invalid::Budget))));
}

#[test]
fn charters_beyond_the_limits_are_refused_as_invalid() {
    let three = Box::new([repository(b"a"), repository(b"b"), repository(b"c")]);
    let twins = Box::new([repository(b"a"), repository(b"a")]);
    let outlets = Box::new([Outlet { name: bytes(b"reply") }, Outlet { name: bytes(b"reply") }]);
    let cases = [
        (Charter { budget: Budget { turns: 101, ..BUDGET }, ..charter() }, Invalid::Budget),
        (Charter { budget: Budget { time: Duration::from_secs(3601), ..BUDGET }, ..charter() }, Invalid::Budget),
        (Charter { budget: Budget { turns: 0, ..BUDGET }, ..charter() }, Invalid::Budget),
        (Charter { budget: Budget { output: 0, ..BUDGET }, ..charter() }, Invalid::Budget),
        (Charter { llm: Llm { max_tokens: 0, ..charter().llm }, ..charter() }, Invalid::Llm),
        (Charter { llm: Llm { max_tokens: 4097, ..charter().llm }, ..charter() }, Invalid::Llm),
        (Charter { brief: Box::from([b'x'; 4096].as_slice()), ..charter() }, Invalid::TooLarge),
        (Charter { checkout: Checkout { repositories: three }, ..charter() }, Invalid::Checkout),
        (Charter { checkout: Checkout { repositories: twins }, ..charter() }, Invalid::Checkout),
        (Charter { grants: Grants { outlets, ..charter().grants }, ..charter() }, Invalid::Grants),
        (Charter { outcome: spec(Box::new([])), ..charter() }, Invalid::Outcome),
        (Charter { outcome: spec(Box::new([rule(b"a", 0, 0), rule(b"a", 1, 1)])), ..charter() }, Invalid::Outcome),
        (
            Charter { outcome: spec(Box::new([rule(b"a", 0, 0), rule(b"b", 0, 0), rule(b"c", 0, 0)])), ..charter() },
            Invalid::Outcome,
        ),
        (Charter { outcome: spec(Box::new([rule(b"a", 3, 2)])), ..charter() }, Invalid::Outcome),
        // Children allowed, with no kind for them to be.
        (
            Charter { outcome: spec(Box::new([VerdictRule { kinds: Box::new([]), ..rule(b"a", 0, 1) }])), ..charter() },
            Invalid::Outcome,
        ),
    ];
    for (call, (charter, invalid)) in (0_u64..).zip(cases) {
        let mut h = Harness::new(LIMITS);
        assert_eq!(answered(h.start(call, charter)), (call, Answer::Refused(Refusal::Invalid(invalid))));
        assert_eq!(h.model.runs(), 0);
    }
    // A change alone is an outcome.
    let mut h = Harness::new(LIMITS);
    drop(h.start(
        1,
        Charter {
            outcome: OutcomeSpec { change: Some(ChangeSpec { checks: true }), verdicts: Box::new([]) },
            ..charter()
        },
    ));
    assert_eq!(h.model.runs(), 1);
}

fn repository(name: &[u8]) -> Repository {
    Repository { name: bytes(name), root: Token::new(1), writable: true }
}

fn spec(verdicts: Box<[VerdictRule]>) -> OutcomeSpec {
    OutcomeSpec { change: None, verdicts }
}

#[test]
fn a_run_whose_main_conversation_fails_answers_with_its_fault_and_what_it_spent() {
    let mut h = Harness::new(LIMITS);
    let (_, conversation) = h.running(1, 100);
    assert!(h.step(Event::Used { conversation, spend: spend(100) }).is_empty(), "within the budget");
    let end = End::Fault(Fault::Provider);
    let emitted = h.step(Event::Ended { conversation, end, spend: spend(100) });
    assert_eq!(answered(emitted), (1, failed(Failure::Model(Fault::Provider), spend(100))));
    assert_eq!(h.model.next_deadline(), None);
    h.model.reclaim();
    assert_eq!((h.model.runs(), h.model.conversations()), (0, 0));
}

#[test]
fn a_run_whose_main_conversation_runs_out_of_budget_fails_for_budget() {
    let mut h = Harness::new(LIMITS);
    let (_, conversation) = h.running(1, 100);
    let end = End::Budget(Exhausted::Turns);
    let emitted = h.step(Event::Ended { conversation, end, spend: Spend::ZERO });
    assert_eq!(answered(emitted), (1, failed(Failure::Budget(Exhausted::Turns), Spend::ZERO)));
}

#[test]
fn a_main_conversation_refused_at_its_entrance_refuses_the_run() {
    let mut h = Harness::new(LIMITS);
    let (_, conversation) = h.admit(1);
    let emitted = h.step(Event::Ended { conversation, end: End::Busy, spend: Spend::ZERO });
    assert_eq!(answered(emitted), (1, Answer::Refused(Refusal::Busy)));
    let (_, conversation) = h.admit(2);
    let emitted = h.step(Event::Ended { conversation, end: End::Invalid, spend: Spend::ZERO });
    assert_eq!(answered(emitted), (2, Answer::Refused(Refusal::Invalid(Invalid::Conversation))));
}

#[test]
fn an_llm_that_stops_without_finishing_is_nudged_until_its_nudges_run_out() {
    let mut h = Harness::new(LIMITS);
    let (_, conversation) = h.running(1, 100);
    let peer = Token::new(100);
    for nudge in 1..=LIMITS.nudges {
        assert!(h.step(Event::Used { conversation, spend: spend(5) }).is_empty(), "within the budget");
        let text = super::prompt::nudge(Stop::EndTurn, nudge, LIMITS.nudges);
        assert_eq!(&*h.step(end_turn(conversation)), &[Request::Say { peer, text }]);
    }
    assert!(h.step(Event::Used { conversation, spend: spend(5) }).is_empty(), "within the budget");
    assert_eq!(&*h.step(end_turn(conversation)), &[Request::Close { peer }]);
    assert_eq!(h.model.next_deadline(), None);
    // A turn that won the race with the close is spent all the same.
    assert!(h.step(Event::Used { conversation, spend: spend(5) }).is_empty(), "winding down");
    let total = Spend { turns: 4, input: 20, output: 40, cache_read: 0, cache_write: 0 };
    let emitted = h.step(Event::Ended { conversation, end: End::Closed, spend: total });
    let unfinished = Failure::Policy(Policy::Unfinished { nudges: LIMITS.nudges, rejected: 0 });
    assert_eq!(answered(emitted), (1, failed(unfinished, total)));
}

fn end_turn(conversation: Token) -> Event {
    Event::Yielded { conversation, stop: Stop::EndTurn, text: bytes(b"done, I think") }
}

#[test]
fn an_llm_whose_last_stop_shows_a_fault_fails_the_run_with_it() {
    let faults =
        [(Stop::MaxTokens, Fault::Truncated), (Stop::Refusal, Fault::Refused), (Stop::NoCalls, Fault::Malformed)];
    for (stop, fault) in faults {
        let mut h = Harness::new(Limits { nudges: 0, ..LIMITS });
        let (_, conversation) = h.running(1, 100);
        drop(h.step(Event::Yielded { conversation, stop, text: bytes(b"") }));
        let emitted = h.step(Event::Ended { conversation, end: End::Closed, spend: Spend::ZERO });
        assert_eq!(answered(emitted), (1, failed(Failure::Model(fault), Spend::ZERO)));
    }
}

#[test]
fn an_llm_with_no_turn_left_is_not_nudged() {
    let mut h = Harness::new(LIMITS);
    let (_, conversation) = h.running(1, 100);
    let all = Spend { turns: BUDGET.turns, ..spend(5) };
    assert!(h.step(Event::Used { conversation, spend: all }).is_empty(), "at the budget");
    let yielded = Event::Yielded { conversation, stop: Stop::EndTurn, text: bytes(b"") };
    assert_eq!(&*h.step(yielded), &[Request::Close { peer: Token::new(100) }]);
    let emitted = h.step(Event::Ended { conversation, end: End::Closed, spend: all });
    assert_eq!(answered(emitted), (1, failed(Failure::Budget(Exhausted::Turns), all)));
}

#[test]
fn spending_past_the_budget_lets_main_finish_its_turn_then_closes_it() {
    let mut h = Harness::new(LIMITS);
    let (_, conversation) = h.running(1, 100);
    assert!(h.step(Event::Used { conversation, spend: spend(BUDGET.input) }).is_empty(), "at the budget");
    assert!(h.step(Event::Used { conversation, spend: spend(1) }).is_empty(), "past it: main keeps its turn");
    assert_eq!(&*h.step(Event::Used { conversation, spend: spend(1) }), &[Request::Close { peer: Token::new(100) }]);
    let total = Spend { turns: 3, input: BUDGET.input + 2, output: 30, cache_read: 0, cache_write: 0 };
    let emitted = h.step(Event::Ended { conversation, end: End::Closed, spend: total });
    assert_eq!(answered(emitted), (1, failed(Failure::Budget(Exhausted::Input), total)));

    // Or it yields, and is closed then.
    let (_, conversation) = h.running(2, 101);
    drop(h.step(Event::Used { conversation, spend: spend(BUDGET.input + 1) }));
    assert_eq!(&*h.step(end_turn(conversation)), &[Request::Close { peer: Token::new(101) }], "no nudge past it");
}

#[test]
fn what_an_end_counts_beyond_the_turns_used_is_charged() {
    let mut h = Harness::new(LIMITS);
    let (_, conversation) = h.running(1, 100);
    drop(h.step(Event::Used { conversation, spend: spend(10) }));
    let end = End::Fault(Fault::ContextFull);
    let emitted = h.step(Event::Ended { conversation, end, spend: spend(30) });
    assert_eq!(answered(emitted), (1, failed(Failure::Model(Fault::ContextFull), spend(30))));
}

#[test]
fn the_deadline_closes_main_and_fails_the_run_for_time() {
    let mut h = Harness::new(LIMITS);
    let (_, conversation) = h.running(1, 100);
    h.after(BUDGET.time);
    assert_eq!(&*h.fire(), &[Request::Close { peer: Token::new(100) }]);
    let emitted = h.step(Event::Ended { conversation, end: End::Budget(Exhausted::Time), spend: Spend::ZERO });
    assert_eq!(answered(emitted), (1, failed(Failure::Budget(Exhausted::Time), Spend::ZERO)));
}

#[test]
fn the_worst_case_is_bounded_or_refused() {
    let bytes = worst_case(&LIMITS).expect("the test limits fit");
    assert!(bytes > 2 * LIMITS.run_bytes, "every run may hold its bytes");
    assert_eq!(worst_case(&Limits { runs: u32::MAX, run_bytes: u64::MAX, ..LIMITS }), None);
}

// Finishing.

/// What a run's main conversation asks when it finishes with `outcome`, as
/// its call `call`.
fn finish(conversation: Token, call: u64, outcome: Declared) -> Event {
    Event::Delegated { conversation, call: Token::new(call), ask: Ask::Finish { outcome } }
}

fn returned(call: u64, result: Returned) -> Request {
    Request::Return { call: Token::new(call), result }
}

fn verdict(name: &[u8], children: Box<[Child]>) -> Declared {
    Declared::Verdict(Verdict { name: bytes(name), body: bytes(b"Looks good."), children })
}

fn comment() -> Child {
    let fields = Box::new([
        Field { name: bytes(b"path"), value: bytes(b"a.rs") },
        Field { name: bytes(b"body"), value: bytes(b"Nit.") },
    ]);
    Child { kind: bytes(b"nit"), fields }
}

fn change() -> Change {
    Change { title: bytes(b"Fix the parser"), body: bytes(b"It accepts tabs now.") }
}

/// The test charter, finishing with a change whose checks must pass, in two
/// writable repositories.
fn coding() -> Charter {
    let checkout = Checkout {
        repositories: Box::new([
            Repository { name: bytes(b"temper"), root: Token::new(900), writable: true },
            Repository { name: bytes(b"docs"), root: Token::new(901), writable: true },
        ]),
    };
    Charter {
        checkout,
        outcome: OutcomeSpec { change: Some(ChangeSpec { checks: true }), verdicts: Box::new([]) },
        ..charter()
    }
}

impl Harness {
    /// Starts a run of `coding()` for call `call` whose repositories both have
    /// checks, and its main conversation as `peer`: the run's token and main's.
    fn coding(&mut self, call: u64, peer: u64) -> (Token, Token) {
        let emitted = self.start(call, coding());
        let [Request::Admitted { run, .. }, Request::Read { .. }] = &*emitted else {
            panic!("expected a read, got {emitted:?}");
        };
        let run = *run;
        drop(self.step(Event::Read { owner: run, read: Read::Missing }));
        drop(self.step(Event::Probed { owner: run, executable: true }));
        drop(self.step(Event::Read { owner: run, read: Read::Missing }));
        let emitted = self.step(Event::Probed { owner: run, executable: true });
        let [Request::Open { conversation, .. }] = &*emitted else {
            panic!("expected main to open, got {emitted:?}");
        };
        let conversation = *conversation;
        drop(self.step(Event::Started { conversation, peer: Token::new(peer) }));
        (run, conversation)
    }

    /// Has main finish with `change()` as call `call`: the owner of the
    /// landing, whose first check is in flight.
    fn land(&mut self, conversation: Token, call: u64) -> Token {
        let emitted = self.step(finish(conversation, call, Declared::Change(change())));
        let [Request::Check { owner, program, deadline, tail }, Request::Checking { worker: _, deadline: until }] =
            &*emitted
        else {
            panic!("expected the first check, got {emitted:?}");
        };
        assert_eq!(program, &Place { root: Token::new(900), path: bytes(b".temper/pre-pr") });
        assert_eq!((*tail, deadline), (LIMITS.check_tail, until));
        assert_eq!(*deadline, self.env.now.saturating_add(LIMITS.check_timeout));
        *owner
    }
}

fn ran(code: u8, output: &[u8]) -> Ran {
    Ran { exit: Exit::Code { code }, output: bytes(output), cut: 0 }
}

#[test]
fn an_outcome_that_does_not_fit_the_spec_is_rejected_and_the_run_goes_on() {
    let mut h = Harness::new(LIMITS);
    let (_, conversation) = h.running(1, 100);
    let emitted = h.step(finish(conversation, 7, Declared::Change(change())));
    let problems = Problems { listed: Box::new([Problem::ChangeNotAllowed]), more: 0 };
    assert_eq!(&*emitted, &[returned(7, Returned::Rejected { problems })]);
    let huge = verdict(b"approve", Box::new([]));
    let Declared::Verdict(mut huge) = huge else { unreachable!("a verdict") };
    huge.body = Box::from([b'x'; 2000].as_slice());
    let emitted = h.step(finish(conversation, 8, Declared::Verdict(huge)));
    let problems = Problems { listed: Box::new([Problem::TooLarge { max: LIMITS.outcome_bytes }]), more: 0 };
    assert_eq!(&*emitted, &[returned(8, Returned::Rejected { problems })]);
    // Nudged out, the run fails as unfinished, counting what it rejected.
    for _ in 0..LIMITS.nudges {
        drop(h.step(end_turn(conversation)));
    }
    assert_eq!(&*h.step(end_turn(conversation)), &[Request::Close { peer: Token::new(100) }]);
    let emitted = h.step(Event::Ended { conversation, end: End::Closed, spend: Spend::ZERO });
    let unfinished = Failure::Policy(Policy::Unfinished { nudges: LIMITS.nudges, rejected: 2 });
    assert_eq!(answered(emitted), (1, failed(unfinished, Spend::ZERO)));
}

#[test]
fn a_verdict_that_fits_is_accepted_and_the_run_finishes_with_it() {
    let mut h = Harness::new(LIMITS);
    let (_, conversation) = h.running(1, 100);
    let outcome = verdict(b"request", Box::new([comment()]));
    let emitted = h.step(finish(conversation, 7, outcome));
    assert_eq!(&*emitted, &[returned(7, Returned::Accepted), Request::Close { peer: Token::new(100) }]);
    let emitted = h.step(Event::Ended { conversation, end: End::Closed, spend: spend(5) });
    let accepted = Answer::Accepted { outcome: verdict(b"request", Box::new([comment()])), spent: spend(5) };
    assert_eq!(answered(emitted), (1, accepted));
}

#[test]
fn a_change_runs_each_repositorys_checks_then_is_pushed_and_accepted() {
    let mut h = Harness::new(LIMITS);
    let (_, conversation) = h.coding(1, 100);
    let owner = h.land(conversation, 7);
    let emitted = h.step(Event::Checked { owner, ran: ran(0, b"ok") });
    let [Request::Check { owner: second, program, .. }, Request::Checking { .. }] = &*emitted else {
        panic!("expected the second check, got {emitted:?}");
    };
    assert_eq!((second, program.root), (&owner, Token::new(901)));
    let emitted = h.step(Event::Checked { owner, ran: ran(0, b"ok") });
    assert_eq!(&*emitted, &[Request::Push { worker: Token::new(1), owner, change: change() }]);
    assert_eq!(h.model.calls(), 1);
    let emitted = h.step(Event::Pushed { owner, push: Push::Done });
    assert_eq!(&*emitted, &[returned(7, Returned::Accepted), Request::Close { peer: Token::new(100) }]);
    let emitted = h.step(Event::Ended { conversation, end: End::Closed, spend: Spend::ZERO });
    let accepted = Answer::Accepted { outcome: Declared::Change(change()), spent: Spend::ZERO };
    assert_eq!(answered(emitted), (1, accepted));
    h.model.reclaim();
    assert_eq!((h.model.runs(), h.model.conversations(), h.model.calls()), (0, 0, 0));
}

#[test]
fn a_change_that_fails_its_checks_or_its_push_goes_back_to_the_llm() {
    let mut h = Harness::new(LIMITS);
    let (_, conversation) = h.coding(1, 100);
    let owner = h.land(conversation, 7);
    let failing = Ran { exit: Exit::Code { code: 1 }, output: bytes(b"test parse ... FAILED"), cut: 12 };
    let emitted = h.step(Event::Checked { owner, ran: failing });
    let failing = Ran { exit: Exit::Code { code: 1 }, output: bytes(b"test parse ... FAILED"), cut: 12 };
    assert_eq!(&*emitted, &[returned(7, Returned::ChecksFailed { repository: bytes(b"temper"), ran: failing })]);
    // A returned call is reclaimed before its conversation can call again,
    // which takes a completion.
    h.model.reclaim();
    let owner = h.land(conversation, 8);
    drop(h.step(Event::Checked { owner, ran: ran(0, b"") }));
    drop(h.step(Event::Checked { owner, ran: ran(0, b"") }));
    assert_eq!(&*h.step(Event::Pushed { owner, push: Push::Moved }), &[returned(8, Returned::Moved)]);
    h.model.reclaim();
    let owner = h.land(conversation, 9);
    drop(h.step(Event::Checked { owner, ran: ran(0, b"") }));
    drop(h.step(Event::Checked { owner, ran: ran(0, b"") }));
    assert_eq!(&*h.step(Event::Pushed { owner, push: Push::Failed }), &[returned(9, Returned::Unpushed)]);
    assert!(h.step(end_turn(conversation)).len() == 1, "the run goes on: a nudge");
}

#[test]
fn a_withdrawn_landing_stops_what_is_in_flight_and_returns_once_it_has() {
    let mut h = Harness::new(LIMITS);
    let (_, conversation) = h.coding(1, 100);
    let owner = h.land(conversation, 7);
    assert!(h.step(Event::Withdraw { conversation, call: Token::new(6) }).is_empty(), "not the landing call");
    assert_eq!(&*h.step(Event::Withdraw { conversation, call: Token::new(7) }), &[Request::Abort { owner }]);
    assert_eq!(&*h.step(Event::Aborted { owner }), &[returned(7, Returned::Cancelled)]);
    assert!(h.step(Event::Withdraw { conversation, call: Token::new(7) }).is_empty(), "returned already");
    h.model.reclaim();

    // Withdrawn while pushing, and the push wins the race: it landed.
    let owner = h.land(conversation, 8);
    drop(h.step(Event::Checked { owner, ran: ran(0, b"") }));
    drop(h.step(Event::Checked { owner, ran: ran(0, b"") }));
    assert_eq!(&*h.step(Event::Withdraw { conversation, call: Token::new(8) }), &[Request::CancelHost { owner }]);
    let emitted = h.step(Event::Pushed { owner, push: Push::Done });
    assert_eq!(&*emitted, &[returned(8, Returned::Accepted), Request::Close { peer: Token::new(100) }]);
}

#[test]
fn past_the_budget_a_finish_in_the_turn_in_flight_still_counts() {
    let mut h = Harness::new(LIMITS);
    let (_, conversation) = h.running(1, 100);
    drop(h.step(Event::Used { conversation, spend: spend(BUDGET.input + 1) }));
    let emitted = h.step(finish(conversation, 7, verdict(b"approve", Box::new([]))));
    assert_eq!(&*emitted, &[returned(7, Returned::Accepted), Request::Close { peer: Token::new(100) }]);
    let total = spend(BUDGET.input + 1);
    let emitted = h.step(Event::Ended { conversation, end: End::Closed, spend: total });
    assert_eq!(answered(emitted), (1, Answer::Accepted { outcome: verdict(b"approve", Box::new([])), spent: total }));

    // A refused one closes main, and the run fails for budget.
    let (_, conversation) = h.running(2, 101);
    drop(h.step(Event::Used { conversation, spend: spend(BUDGET.input + 1) }));
    let emitted = h.step(finish(conversation, 8, verdict(b"reject", Box::new([]))));
    let problems = Problems { listed: Box::new([Problem::UnknownVerdict]), more: 0 };
    assert_eq!(&*emitted, &[returned(8, Returned::Rejected { problems }), Request::Close { peer: Token::new(101) }]);
}

#[test]
fn a_run_out_of_time_while_it_prepares_answers_once_its_look_has_ended() {
    let mut h = Harness::new(LIMITS);
    let run = h.prepare(1);
    h.after(BUDGET.time);
    assert!(h.fire().is_empty(), "the look is in flight");
    let emitted = h.step(Event::Read { owner: run, read: Read::Missing });
    assert_eq!(answered(emitted), (1, failed(Failure::Budget(Exhausted::Time), Spend::ZERO)));
}

// A cancel in every state.

fn cancelled() -> Answer {
    failed(Failure::Cancelled, Spend::ZERO)
}

#[test]
fn a_cancel_while_preparing_stops_the_run_once_its_look_has_ended() {
    let mut h = Harness::new(LIMITS);
    let run = h.prepare(1);
    assert!(h.step(Event::Cancel { run }).is_empty(), "the look is in flight");
    assert_eq!(h.model.next_deadline(), None);
    assert!(h.step(Event::Cancel { run }).is_empty(), "stopping: the ending is decided");
    let emitted = h.step(Event::Read { owner: run, read: Read::Missing });
    assert_eq!(answered(emitted), (1, cancelled()));
    h.model.reclaim();
    assert_eq!((h.model.runs(), h.model.conversations()), (0, 0), "main was never opened, and is gone too");
}

#[test]
fn a_cancel_while_main_opens_closes_main_once_it_starts() {
    let mut h = Harness::new(LIMITS);
    let (run, conversation) = h.admit(1);
    assert!(h.step(Event::Cancel { run }).is_empty(), "main has no peer to close yet");
    assert!(h.step(Event::Cancel { run }).is_empty(), "the ending is decided");
    let peer = Token::new(100);
    assert_eq!(&*h.step(Event::Started { conversation, peer }), &[Request::Close { peer }]);
    let emitted = h.step(Event::Ended { conversation, end: End::Closed, spend: Spend::ZERO });
    assert_eq!(answered(emitted), (1, cancelled()));

    // Or main is refused, and the cancel is still how the run ends.
    let (run, conversation) = h.admit(2);
    drop(h.step(Event::Cancel { run }));
    let emitted = h.step(Event::Ended { conversation, end: End::Busy, spend: Spend::ZERO });
    assert_eq!(answered(emitted), (2, cancelled()));
}

#[test]
fn a_cancel_while_main_works_closes_it() {
    let mut h = Harness::new(LIMITS);
    let (run, conversation) = h.running(1, 100);
    assert_eq!(&*h.step(Event::Cancel { run }), &[Request::Close { peer: Token::new(100) }]);
    // The conversation failed before it saw the close: the cancel still wins.
    let emitted = h.step(Event::Ended { conversation, end: End::Fault(Fault::Provider), spend: Spend::ZERO });
    assert_eq!(answered(emitted), (1, cancelled()));
}

#[test]
fn a_cancel_while_main_is_over_the_budget_closes_it() {
    let mut h = Harness::new(LIMITS);
    let (run, conversation) = h.running(1, 100);
    drop(h.step(Event::Used { conversation, spend: spend(BUDGET.input + 1) }));
    assert_eq!(&*h.step(Event::Cancel { run }), &[Request::Close { peer: Token::new(100) }]);
    let spent = spend(BUDGET.input + 1);
    let emitted = h.step(Event::Ended { conversation, end: End::Budget(Exhausted::Input), spend: spent });
    assert_eq!(answered(emitted), (1, failed(Failure::Cancelled, spent)));
}

#[test]
fn a_cancel_while_a_change_is_checked_stops_the_checks_once_main_withdraws_its_call() {
    let mut h = Harness::new(LIMITS);
    let (run, conversation) = h.coding(1, 100);
    let owner = h.land(conversation, 7);
    assert_eq!(&*h.step(Event::Cancel { run }), &[Request::Close { peer: Token::new(100) }]);
    assert_eq!(&*h.step(Event::Withdraw { conversation, call: Token::new(7) }), &[Request::Abort { owner }]);
    // The checks pass before the abort lands: nothing is pushed.
    assert_eq!(&*h.step(Event::Checked { owner, ran: ran(0, b"") }), &[returned(7, Returned::Cancelled)]);
    // A finish that crossed the close is cancelled too.
    let crossed = finish(conversation, 8, verdict(b"approve", Box::new([])));
    assert_eq!(&*h.step(crossed), &[returned(8, Returned::Cancelled)]);
    let emitted = h.step(Event::Ended { conversation, end: End::Closed, spend: Spend::ZERO });
    assert_eq!(answered(emitted), (1, cancelled()));
}

#[test]
fn a_cancel_while_a_change_is_pushed_wins_over_the_push() {
    let mut h = Harness::new(LIMITS);
    let (run, conversation) = h.coding(1, 100);
    let owner = h.land(conversation, 7);
    drop(h.step(Event::Checked { owner, ran: ran(0, b"") }));
    drop(h.step(Event::Checked { owner, ran: ran(0, b"") }));
    assert_eq!(&*h.step(Event::Cancel { run }), &[Request::Close { peer: Token::new(100) }]);
    assert_eq!(&*h.step(Event::Withdraw { conversation, call: Token::new(7) }), &[Request::CancelHost { owner }]);
    // The push won the race with its cancel, and loses to the run's.
    assert_eq!(&*h.step(Event::Pushed { owner, push: Push::Done }), &[returned(7, Returned::Cancelled)]);
    let emitted = h.step(Event::Ended { conversation, end: End::Closed, spend: Spend::ZERO });
    assert_eq!(answered(emitted), (1, cancelled()));

    // Or the cancel wins its race.
    let (run, conversation) = h.coding(2, 101);
    let owner = h.land(conversation, 8);
    drop(h.step(Event::Checked { owner, ran: ran(0, b"") }));
    drop(h.step(Event::Checked { owner, ran: ran(0, b"") }));
    drop(h.step(Event::Cancel { run }));
    drop(h.step(Event::Withdraw { conversation, call: Token::new(8) }));
    assert_eq!(&*h.step(Event::HostCancelled { owner }), &[returned(8, Returned::Cancelled)]);
    let emitted = h.step(Event::Ended { conversation, end: End::Closed, spend: Spend::ZERO });
    assert_eq!(answered(emitted), (2, cancelled()));
}

#[test]
fn a_cancel_while_the_run_winds_down_changes_nothing() {
    let mut h = Harness::new(LIMITS);
    let (run, conversation) = h.running(1, 100);
    h.after(BUDGET.time);
    drop(h.fire());
    assert!(h.step(Event::Cancel { run }).is_empty(), "the ending is decided");
    let emitted = h.step(Event::Ended { conversation, end: End::Closed, spend: Spend::ZERO });
    assert_eq!(answered(emitted), (1, failed(Failure::Budget(Exhausted::Time), Spend::ZERO)));

    // Winding down to an accepted outcome, too.
    let (run, conversation) = h.running(2, 101);
    drop(h.step(finish(conversation, 7, verdict(b"approve", Box::new([])))));
    assert!(h.step(Event::Cancel { run }).is_empty(), "the ending is decided");
    let emitted = h.step(Event::Ended { conversation, end: End::Closed, spend: Spend::ZERO });
    let accepted = Answer::Accepted { outcome: verdict(b"approve", Box::new([])), spent: Spend::ZERO };
    assert_eq!(answered(emitted), (2, accepted));
}

#[test]
fn a_cancel_of_a_run_that_has_answered_changes_nothing() {
    let mut h = Harness::new(LIMITS);
    let (run, conversation) = h.running(1, 100);
    drop(h.step(Event::Ended { conversation, end: End::Fault(Fault::Provider), spend: Spend::ZERO }));
    assert!(h.step(Event::Cancel { run }).is_empty(), "answered, not reclaimed yet");
    h.model.reclaim();
    assert!(h.step(Event::Cancel { run }).is_empty(), "answered and gone");
    // Its slot taken by another run, the old name still finds nothing.
    let (_, _) = h.running(2, 101);
    assert!(h.step(Event::Cancel { run }).is_empty(), "a stale name");
    assert_eq!(h.model.runs(), 1);
}

#[test]
fn a_finish_with_no_room_for_its_call_is_busy_and_the_run_goes_on() {
    let mut h = Harness::new(Limits { calls: 1, ..LIMITS });
    let (_, first) = h.coding(1, 100);
    let _: Token = h.land(first, 7);
    let (_, second) = h.coding(2, 101);
    let emitted = h.step(finish(second, 8, Declared::Change(change())));
    assert_eq!(&*emitted, &[returned(8, Returned::Busy)]);
    assert_eq!(h.model.calls(), 1);
    assert_eq!(h.step(end_turn(second)).len(), 1, "the run goes on: a nudge");
}
