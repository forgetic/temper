//! Feed the model events, inspect the requests that come out.

use alloc::boxed::Box;

use temper_lib::{Duration, Env, List, Queue, ReplyTo, Time, Token};

use crate::charter::{Checkout, Endpoint, Grants, Llm, Outlet, Repository, Tools};
use crate::outcome::{Children, OutcomeSpec, VerdictRule};
use crate::{
    Answer, Budget, Charter, End, Event, Exhausted, Failure, Fault, Invalid, Limits, MAX_OUT, Model, Opening, Policy,
    Refusal, Request, Spend, Stop, fire, step, worst_case,
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
    budget: Budget {
        turns: 100,
        input: 1_000_000,
        output: 100_000,
        cache_read: 1_000_000,
        cache_write: 1_000_000,
        time: Duration::from_secs(3600),
    },
    max_tokens: 4096,
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
    /// the run's token and its main conversation's.
    fn admit(&mut self, call: u64) -> (Token, Token) {
        let emitted = self.start(call, charter());
        let [Request::Admitted { worker, run }, Request::Open { conversation, opening: _ }] = &*emitted else {
            panic!("expected an admitted run, got {emitted:?}");
        };
        assert_eq!(*worker, Token::new(call));
        (*run, *conversation)
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

fn bytes(text: &[u8]) -> Box<[u8]> {
    Box::from(text)
}

fn rule(name: &[u8], min: u32, max: u32) -> VerdictRule {
    VerdictRule {
        name: bytes(name),
        children: Children { min, max },
        kinds: Box::new([bytes(b"blocking"), bytes(b"nit")]),
        fields: Box::new([bytes(b"path"), bytes(b"body")]),
    }
}

fn charter() -> Charter {
    Charter {
        brief: bytes(b"Review the change."),
        checkout: Checkout {
            repositories: Box::new([Repository {
                name: bytes(b"temper"),
                path: bytes(b"/work/temper"),
                writable: false,
            }]),
        },
        grants: Grants {
            tools: Tools { inspect: true, modify: false, shell: true },
            forge: true,
            agents: false,
            outlets: Box::new([Outlet { name: bytes(b"comment") }]),
        },
        outcome: OutcomeSpec { change: false, verdicts: Box::new([rule(b"approve", 0, 0), rule(b"request", 1, 8)]) },
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
fn an_admitted_run_names_itself_and_opens_its_main_conversation_with_the_whole_budget() {
    let mut h = Harness::new(LIMITS);
    let emitted = h.start(7, charter());
    let [Request::Admitted { worker, run: _ }, Request::Open { conversation: _, opening }] = &*emitted else {
        panic!("expected an admitted run, got {emitted:?}");
    };
    assert_eq!(*worker, Token::new(7));
    let expected = Opening {
        llm: charter().llm,
        system: charter().brief,
        prompt: bytes(super::run::BEGIN),
        tools: charter().grants.tools,
        checkout: charter().checkout,
        budget: BUDGET,
    };
    assert_eq!(opening, &expected);
    assert_eq!((h.model.runs(), h.model.conversations()), (1, 1));
    assert_eq!(h.model.next_deadline(), Some(Time::ZERO.saturating_add(BUDGET.time)));
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
    drop(h.start(1, Charter { outcome: OutcomeSpec { change: true, verdicts: Box::new([]) }, ..charter() }));
    assert_eq!(h.model.runs(), 1);
}

fn repository(name: &[u8]) -> Repository {
    Repository { name: bytes(name), path: bytes(b"/work"), writable: true }
}

fn spec(verdicts: Box<[VerdictRule]>) -> OutcomeSpec {
    OutcomeSpec { change: false, verdicts }
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
fn a_yield_closes_main_and_the_run_answers_once_main_has_ended() {
    let mut h = Harness::new(LIMITS);
    let (_, conversation) = h.running(1, 100);
    let peer = Token::new(100);
    let yielded = Event::Yielded { conversation, stop: Stop::EndTurn, text: bytes(b"done, I think") };
    assert_eq!(&*h.step(yielded), &[Request::Close { peer }]);
    assert_eq!(h.model.next_deadline(), None);
    // A turn that won the race with the close is spent all the same.
    assert!(h.step(Event::Used { conversation, spend: spend(5) }).is_empty(), "winding down");
    let emitted = h.step(Event::Ended { conversation, end: End::Closed, spend: spend(5) });
    let unfinished = Failure::Policy(Policy::Unfinished { nudges: 0 });
    assert_eq!(answered(emitted), (1, failed(unfinished, spend(5))));
}

#[test]
fn a_yield_that_shows_a_fault_fails_the_run_with_it() {
    for (stop, fault) in
        [(Stop::MaxTokens, Fault::Truncated), (Stop::Refusal, Fault::Refused), (Stop::NoCalls, Fault::Malformed)]
    {
        let mut h = Harness::new(LIMITS);
        let (_, conversation) = h.running(1, 100);
        drop(h.step(Event::Yielded { conversation, stop, text: bytes(b"") }));
        let emitted = h.step(Event::Ended { conversation, end: End::Closed, spend: Spend::ZERO });
        assert_eq!(answered(emitted), (1, failed(Failure::Model(fault), Spend::ZERO)));
    }
}

#[test]
fn spending_past_the_budget_closes_main_and_fails_the_run_for_budget() {
    let mut h = Harness::new(LIMITS);
    let (_, conversation) = h.running(1, 100);
    assert!(h.step(Event::Used { conversation, spend: spend(BUDGET.input) }).is_empty(), "at the budget");
    assert_eq!(&*h.step(Event::Used { conversation, spend: spend(1) }), &[Request::Close { peer: Token::new(100) }]);
    // The conversation ran out too, and ended before it saw the close.
    let total = spend(BUDGET.input).saturating_add(spend(1));
    let emitted = h.step(Event::Ended { conversation, end: End::Budget(Exhausted::Input), spend: total });
    assert_eq!(answered(emitted), (1, failed(Failure::Budget(Exhausted::Input), total)));
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
fn a_cancel_closes_main_and_later_cancels_change_nothing() {
    let mut h = Harness::new(LIMITS);
    let (run, conversation) = h.running(1, 100);
    assert_eq!(&*h.step(Event::Cancel { run }), &[Request::Close { peer: Token::new(100) }]);
    assert!(h.step(Event::Cancel { run }).is_empty(), "the ending is decided");
    // The conversation failed before it saw the close: the cancel still wins.
    let emitted = h.step(Event::Ended { conversation, end: End::Fault(Fault::Provider), spend: Spend::ZERO });
    assert_eq!(answered(emitted), (1, failed(Failure::Cancelled, Spend::ZERO)));
    assert!(h.step(Event::Cancel { run }).is_empty(), "answered, not reclaimed yet");
    h.model.reclaim();
    assert!(h.step(Event::Cancel { run }).is_empty(), "answered and gone");
}

#[test]
fn a_cancel_before_main_starts_closes_main_once_it_does() {
    let mut h = Harness::new(LIMITS);
    let (run, conversation) = h.admit(1);
    assert!(h.step(Event::Cancel { run }).is_empty(), "main has no peer to close yet");
    let peer = Token::new(100);
    assert_eq!(&*h.step(Event::Started { conversation, peer }), &[Request::Close { peer }]);
    let emitted = h.step(Event::Ended { conversation, end: End::Closed, spend: Spend::ZERO });
    assert_eq!(answered(emitted), (1, failed(Failure::Cancelled, Spend::ZERO)));

    // Or main is refused, and the cancel is still how the run ends.
    let (run, conversation) = h.admit(2);
    drop(h.step(Event::Cancel { run }));
    let emitted = h.step(Event::Ended { conversation, end: End::Busy, spend: Spend::ZERO });
    assert_eq!(answered(emitted), (2, failed(Failure::Cancelled, Spend::ZERO)));
}

#[test]
fn the_worst_case_is_bounded_or_refused() {
    let bytes = worst_case(&LIMITS).expect("the test limits fit");
    assert!(bytes > 2 * LIMITS.run_bytes, "every run may hold its bytes");
    assert_eq!(worst_case(&Limits { runs: u32::MAX, run_bytes: u64::MAX, ..LIMITS }), None);
}
