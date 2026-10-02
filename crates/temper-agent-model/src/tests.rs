//! Feed the model events, see each routed to the session and the session's
//! requests routed back out. What the session does with them is its own tests'
//! business; these name every variant in each direction.

use alloc::boxed::Box;

use temper_agent_model_session as session;
use temper_lib::{Duration, Env, Queue, Time, Token};

use crate::llm::{
    Answer, Block, Completion, Decoded, Descriptor, Endpoint, Failure, Message, Returned, Role, Stop, Usage,
};
use crate::tools::{self, Authority, Done, Effect, Entry, Grants, Kind, Name, Op, Outcome, Part, Path, Place, Repo};
use crate::{
    Budget, Dimension, End, Event, Fact, Limits, Model, Request, Spec, Yield, fire, max_out, step, worst_case,
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
        tool_timeout: Duration::from_secs(20),
        facts: 64,
        parallel_tools: 2,
        tools: tools::Limits {
            kits: 2,
            calls: 4,
            repos: 1,
            path_bytes: 64,
            known_files: 8,
            file_bytes: 65_536,
            read_bytes: 4096,
            list_entries: 16,
            match_lines: 4,
            file_timeout: Duration::from_secs(60),
            env_bytes: 64,
            shell_timeout: Duration::from_secs(60),
            shell_timeout_max: Duration::from_secs(600),
            shell_head: 64,
            shell_tail: 64,
            search_hits: 8,
            search_bytes: 256,
            search_timeout: Duration::from_secs(30),
            facts: 64,
        },
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
            out: Queue::with_capacity(max_out(&LIMITS)),
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
    /// owner and that of the operation its tools ask of io.
    fn open_tool(&mut self) -> (Token, Token) {
        let owner = self.open();
        let Some(Request::Io { owner: op, .. }) = self.complete(owner, ls()) else {
            panic!("expected an operation for the tools");
        };
        (owner, op)
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
        authority: authority(),
        delegated: Box::new([Descriptor { ticket: Token::new(7), effect: Effect::Write }]),
        prompt: bytes(b"fix the bug"),
        max_tokens: 1024,
        budget: BUDGET,
    }
}

/// io's name for the root of the checkout's one repository.
const ROOT: Token = Token::new(70);

/// What the session's tools may do: inspect the one repository, at `/work`.
fn authority() -> Authority {
    Authority {
        cwd: Box::new([work()]),
        repos: Box::new([Repo { mount: Box::new([work()]), root: ROOT, writable: false }]),
        grants: Grants { inspect: true, modify: false, shell: false },
        env: Box::new([]),
    }
}

fn work() -> Name {
    Name::new(bytes(b"work")).expect("a name")
}

fn usage() -> Usage {
    Usage { input_tokens: 10, output_tokens: 5, cache_read_tokens: 3, cache_write_tokens: 2 }
}

/// The working directory's one entry.
fn entries() -> Box<[Entry]> {
    let name = Name::new(bytes(b"main.rs")).expect("a name");
    Box::new([Entry { name, kind: Kind::File }])
}

/// A listing of one file.
fn listed() -> Outcome {
    Outcome::Listed { entries: entries(), more: 0 }
}

/// The LLM asks for `ls`.
fn ls() -> Completion {
    let call = tools::Call::List { path: Path { absolute: false, parts: Box::new([Part::Current]) } };
    let call = Decoded::Owned { call };
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
fn a_completion_reaches_the_session_and_its_tools_operation_comes_back_out() {
    let mut h = Harness::new();
    let owner = h.open();
    let Some(Request::Io { owner: _, op, deadline }) = h.complete(owner, ls()) else {
        panic!("expected an operation for the tools");
    };
    let scan = Op::Scan { at: Place { root: ROOT, path: bytes(b"") }, max: LIMITS.session.tools.list_entries };
    assert_eq!((op, deadline), (scan, Time::ZERO.saturating_add(LIMITS.session.tool_timeout)));
}

#[test]
fn an_operations_end_reaches_the_session_and_its_next_call_comes_back_out() {
    let mut h = Harness::new();
    let (owner, op) = h.open_tool();
    let Some(Request::Complete { owner: next, prompt, timeout: _ }) =
        h.step(Event::Done { owner: op, done: Done::Scanned { entries: entries(), more: 0 } })
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
    let (_, op) = h.open_tool();
    h.expire();
    assert_eq!(h.fire(), Some(Request::CancelIo { owner: op }));
    assert_eq!(h.step(Event::Done { owner: op, done: Done::Cancelled }), Some(ended(OUT_OF_TIME, 1)));
}

#[test]
fn the_sessions_facts_pass_through_to_the_loop() {
    let mut h = Harness::new();
    let session = h.open_yielded();
    assert_eq!(h.model.pop_fact(), Some(Fact::Opened { opener: opener() }));
    assert_eq!(h.step(Event::Close { session }), Some(ended(End::Closed, 1)));
    let (mut before, mut last) = (None, None);
    for _ in 0..LIMITS.session.facts {
        let Some(fact) = h.model.pop_fact() else { break };
        before = last.replace(fact);
    }
    assert_eq!(before, Some(Fact::Ended { opener: opener(), end: End::Closed, turns: 1, usage: usage() }));
    // The session's tools tell of its kit, which closed with it.
    assert_eq!(last, Some(Fact::Tools { fact: tools::Fact::Closed { session } }));
    assert_eq!(h.model.facts_lost(), 0);
}

#[test]
fn a_delegated_call_goes_out_and_its_answer_and_its_withdrawal_reach_the_session() {
    let mut h = Harness::new();
    let owner = h.open();
    let call = Decoded::Delegated { ticket: Token::new(9), effect: Effect::Write };
    let content = Box::new([Block::ToolCall { id: bytes(b"f"), name: bytes(b"finish"), input: bytes(b"{}"), call }]);
    let finish = Completion { content, stop: Stop::ToolUse, usage: usage() };
    let Some(Request::Delegate { owner: run, opener: to, call, deadline: _ }) = h.complete(owner, finish.clone())
    else {
        panic!("expected the call delegated");
    };
    assert_eq!((to, call), (opener(), Token::new(9)));
    let answer = Answer { ticket: Token::new(11), bytes: 2, error: false };
    let Some(Request::Complete { .. }) = h.step(Event::Answered { owner: run, answer }) else {
        panic!("expected the answer sent back");
    };

    let owner = h.open();
    let Some(Request::Delegate { owner: run, .. }) = h.complete(owner, finish) else {
        panic!("expected the call delegated");
    };
    assert_eq!(h.step(Event::Close { session: owner }), Some(Request::Withdraw { owner: run }));
    assert_eq!(h.step(Event::AnswerCancelled { owner: run }), Some(ended(End::Closed, 1)));
}

#[test]
fn an_entry_point_emits_what_the_session_does() {
    assert_eq!(max_out(&LIMITS), session::max_out(&LIMITS.session));
}

#[test]
fn the_worst_case_is_the_sessions_and_the_routing_queues() {
    let sessions = session::worst_case(&LIMITS.session).expect("the test limits fit");
    let queue = Queue::<session::Request>::worst_case(session::max_out(&LIMITS.session)).expect("a step fits");
    assert!(queue > 0, "the routing queue takes room");
    assert_eq!(worst_case(&LIMITS), sessions.checked_add(queue));
    let huge = Limits { session: session::Limits { sessions: u32::MAX, session_bytes: u64::MAX, ..LIMITS.session } };
    assert_eq!(worst_case(&huge), None);
}
