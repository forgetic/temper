//! Feed the model events, see each routed to the session and the session's
//! requests routed back out. What the session does with them is its own tests'
//! business; these name every variant in each direction.

use alloc::boxed::Box;

use temper_agent_model_session as session;
use temper_lib::{Duration, Env, Queue, Time, Token};

use crate::llm::{Block, Completion, Decoded, Endpoint, Failure, Message, Returned, Role, Stop, Usage};
use crate::tools::{Call, Entry, Grants, Kind, Name, Outcome, Part, Path};
use crate::{
    Budget, Dimension, End, Event, Fact, Limits, MAX_OUT, Model, Request, Spec, Yield, fire, step, worst_case,
};

const BUDGET: Budget = Budget {
    turns: 4,
    input: 1_000_000,
    output: 1_000_000,
    cache_read: 1_000_000,
    cache_write: 1_000_000,
    time: Duration::from_secs(600),
};

const LIMITS: Limits = Limits {
    session: session::Limits {
        sessions: 2,
        messages: 8,
        session_bytes: 65_536,
        budget: BUDGET,
        max_tokens: 1024,
        retries: 2,
        backoff_base: Duration::from_millis(100),
        backoff_max: Duration::from_secs(1),
        call_timeout: Duration::from_secs(30),
        facts: 64,
    },
};

/// The model, its environment, and room for one step's output.
struct Harness {
    model: Model,
    env: Env<Limits>,
    out: Queue<Request>,
}

impl Harness {
    fn new() -> Harness {
        Harness {
            model: Model::new(&LIMITS, 1),
            env: Env { now: Time::ZERO, limits: LIMITS },
            out: Queue::with_capacity(MAX_OUT),
        }
    }

    /// Steps the model with `event`, which emits at most one request.
    fn step(&mut self, event: Event) -> Option<Request> {
        step(&mut self.model, &self.env, event, &mut self.out);
        self.one()
    }

    /// Steps the model with a completion, which comes back out as `Used` and
    /// then what the session does next.
    fn complete(&mut self, owner: Token, completion: Completion) -> Option<Request> {
        step(&mut self.model, &self.env, Event::Completed { owner, completion }, &mut self.out);
        assert_eq!(self.out.pop(), Some(Request::Used { opener: opener(), usage: usage() }));
        self.one()
    }

    fn fire(&mut self) -> Option<Request> {
        assert!(self.model.is_due(self.env.now), "an alarm is due");
        fire(&mut self.model, &self.env, &mut self.out);
        self.one()
    }

    fn one(&mut self) -> Option<Request> {
        let request = self.out.pop();
        assert!(self.out.is_empty(), "one request at most");
        request
    }

    /// Opens a session for opener 1, returning the owner of its first call.
    fn open(&mut self) -> Token {
        step(&mut self.model, &self.env, Event::Open { opener: opener(), spec: spec() }, &mut self.out);
        let Some(Request::Opened { opener: _, session }) = self.out.pop() else {
            panic!("expected the session to open");
        };
        let Some(Request::Complete { owner, .. }) = self.one() else {
            panic!("expected a call");
        };
        assert_eq!(owner, session);
        owner
    }

    /// Opens a session and has the LLM ask for `ls`, returning the session's
    /// owner.
    fn open_tool(&mut self) -> Token {
        let owner = self.open();
        let Some(Request::Tool { .. }) = self.complete(owner, ls()) else {
            panic!("expected a tool run");
        };
        owner
    }

    /// Opens a session and has the LLM finish its turn, returning the
    /// session's name.
    fn open_yielded(&mut self) -> Token {
        let owner = self.open();
        let Some(Request::Yielded { .. }) = self.complete(owner, done()) else {
            panic!("expected a yield");
        };
        owner
    }

    fn expire(&mut self) {
        self.env.now = self.env.now.checked_add(BUDGET.time).expect("the test stays in range");
    }
}

fn bytes(text: &[u8]) -> Box<[u8]> {
    Box::from(text)
}

fn opener() -> Token {
    Token::new(1)
}

fn spec() -> Spec {
    Spec {
        endpoint: Endpoint(0),
        model: bytes(b"model"),
        system: bytes(b"be brief"),
        tools: Grants { inspect: true, modify: false, shell: false },
        prompt: bytes(b"fix the bug"),
        max_tokens: 1024,
        budget: BUDGET,
    }
}

fn usage() -> Usage {
    Usage { input_tokens: 10, output_tokens: 5, cache_read_tokens: 3, cache_write_tokens: 2 }
}

/// Lists the working directory.
fn list() -> Call {
    Call::List { path: Path { absolute: false, parts: Box::new([Part::Current]) } }
}

/// A listing of one file.
fn listed() -> Outcome {
    let name = Name::new(bytes(b"main.rs")).expect("a name");
    Outcome::Listed { entries: Box::new([Entry { name, kind: Kind::File }]), more: 0 }
}

/// The LLM asks for `ls`.
fn ls() -> Completion {
    let call = Decoded::Owned { call: list() };
    let content = Box::new([Block::ToolCall { id: bytes(b"c1"), name: bytes(b"ls"), input: bytes(b"{}"), call }]);
    Completion { content, stop: Stop::ToolUse, usage: usage() }
}

/// The LLM finishes its turn.
fn done() -> Completion {
    Completion { content: Box::new([Block::Text { text: bytes(b"done") }]), stop: Stop::EndTurn, usage: usage() }
}

fn ended(end: End, turns: u32) -> Request {
    let mut total = Usage::ZERO;
    for _ in 0..turns {
        total = total.saturating_add(usage());
    }
    Request::Ended { opener: opener(), end, turns, usage: total }
}

const OUT_OF_TIME: End = End::Budget { spent: Dimension::Time };

#[test]
fn an_open_reaches_the_session_and_its_opening_and_call_come_back_out() {
    let mut h = Harness::new();
    step(&mut h.model, &h.env, Event::Open { opener: opener(), spec: spec() }, &mut h.out);
    let Some(Request::Opened { opener: to, session }) = h.out.pop() else {
        panic!("expected the session to open");
    };
    assert_eq!(to, opener());
    let Some(Request::Complete { owner, prompt, timeout }) = h.one() else {
        panic!("expected a call");
    };
    assert_eq!((owner, timeout), (session, LIMITS.session.call_timeout));
    let first = Message { role: Role::User, content: Box::new([Block::Text { text: bytes(b"fix the bug") }]) };
    assert_eq!(&*prompt.messages, &[first]);
    assert_eq!(h.model.sessions(), 1);
    assert_eq!(h.model.next_deadline(), Some(Time::ZERO.saturating_add(BUDGET.time)));
}

#[test]
fn a_completion_reaches_the_session_and_its_tool_run_comes_back_out() {
    let mut h = Harness::new();
    let owner = h.open();
    let request = h.complete(owner, ls());
    assert_eq!(request, Some(Request::Tool { owner, call: list() }));
}

#[test]
fn a_tool_result_reaches_the_session_and_its_next_call_comes_back_out() {
    let mut h = Harness::new();
    let owner = h.open_tool();
    let Some(Request::Complete { owner: next, prompt, timeout: _ }) =
        h.step(Event::ToolDone { owner, outcome: listed() })
    else {
        panic!("expected a call");
    };
    assert_eq!(next, owner);
    let result = Block::ToolResult { id: bytes(b"c1"), result: Returned::Owned { outcome: listed() } };
    let last = prompt.messages.last().expect("the results go back last");
    assert_eq!(&*last.content, &[result]);
}

#[test]
fn a_yield_and_its_usage_come_back_out_and_a_continue_reaches_the_session() {
    let mut h = Harness::new();
    let owner = h.open();
    let request = h.complete(owner, done());
    assert_eq!(request, Some(Request::Yielded { opener: opener(), stop: Yield::Done, text: bytes(b"done") }));
    let Some(Request::Complete { owner: next, prompt, timeout: _ }) =
        h.step(Event::Continue { session: owner, content: bytes(b"go on") })
    else {
        panic!("expected a call");
    };
    assert_eq!(next, owner);
    assert_eq!(prompt.messages.len(), 3);
}

#[test]
fn a_close_reaches_the_session_and_its_end_comes_back_out() {
    let mut h = Harness::new();
    let session = h.open_yielded();
    assert_eq!(h.step(Event::Close { session }), Some(ended(End::Closed, 1)));
    h.model.reclaim();
    assert_eq!(h.model.sessions(), 0);
}

#[test]
fn a_failure_reaches_the_session_and_its_end_comes_back_out() {
    let mut h = Harness::new();
    let owner = h.open();
    let end = h.step(Event::Failed { owner, failure: Failure::Invalid });
    assert_eq!(end, Some(ended(End::Failed { failure: Failure::Invalid }, 0)));
    assert_eq!(h.model.next_deadline(), None);
    h.model.reclaim();
    assert_eq!(h.model.sessions(), 0);
}

#[test]
fn an_expired_call_is_cancelled_and_its_cancellation_reaches_the_session() {
    let mut h = Harness::new();
    let owner = h.open();
    h.expire();
    assert_eq!(h.fire(), Some(Request::Cancel { owner }));
    assert_eq!(h.step(Event::Cancelled { owner }), Some(ended(OUT_OF_TIME, 0)));
}

#[test]
fn an_expired_tool_run_is_cancelled_and_its_cancellation_reaches_the_session() {
    let mut h = Harness::new();
    let owner = h.open_tool();
    h.expire();
    assert_eq!(h.fire(), Some(Request::CancelTool { owner }));
    assert_eq!(h.step(Event::ToolCancelled { owner }), Some(ended(OUT_OF_TIME, 1)));
}

#[test]
fn the_sessions_facts_pass_through_to_the_loop() {
    let mut h = Harness::new();
    let session = h.open_yielded();
    assert_eq!(h.model.pop_fact(), Some(Fact::Opened { opener: opener() }));
    assert_eq!(h.step(Event::Close { session }), Some(ended(End::Closed, 1)));
    let mut last = None;
    for _ in 0..LIMITS.session.facts {
        let Some(fact) = h.model.pop_fact() else { break };
        last = Some(fact);
    }
    assert_eq!(last, Some(Fact::Ended { opener: opener(), end: End::Closed, turns: 1, usage: usage() }));
    assert_eq!(h.model.facts_lost(), 0);
}

#[test]
fn an_entry_point_emits_what_the_session_does() {
    assert_eq!(MAX_OUT, session::MAX_OUT);
}

#[test]
fn the_worst_case_is_the_sessions_and_the_routing_queues() {
    let sessions = session::worst_case(&LIMITS.session).expect("the test limits fit");
    let queue = Queue::<session::Request>::worst_case(session::MAX_OUT).expect("one request fits");
    assert!(queue > 0, "the routing queue takes room");
    assert_eq!(worst_case(&LIMITS), sessions.checked_add(queue));
    let huge = Limits { session: session::Limits { sessions: u32::MAX, session_bytes: u64::MAX, ..LIMITS.session } };
    assert_eq!(worst_case(&huge), None);
}
