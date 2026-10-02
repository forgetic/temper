//! Feed the model events, see each routed to the session and the session's
//! requests routed back out. What the session does with them is its own tests'
//! business; these name every variant in each direction.

use alloc::boxed::Box;

use temper_agent_model_session as session;
use temper_lib::{Duration, Env, Queue, ReplyTo, Time, Token};

use crate::llm::{Block, Completion, Endpoint, Failure, Message, Role, Stop, Tool, Usage};
use crate::{Event, Limits, MAX_OUT, Model, Outcome, Report, Request, Task, ToolCall, fire, step, worst_case};

const LIMITS: Limits = Limits {
    session: session::Limits {
        sessions: 2,
        messages: 8,
        session_bytes: 65_536,
        turns: 4,
        max_tokens: 1024,
        retries: 2,
        backoff_base: Duration::from_millis(100),
        backoff_max: Duration::from_secs(1),
        call_timeout: Duration::from_secs(30),
        session_timeout: Duration::from_secs(600),
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

    fn step(&mut self, event: Event) -> Option<Request> {
        step(&mut self.model, &self.env, event, &mut self.out);
        self.out.pop()
    }

    fn fire(&mut self) -> Option<Request> {
        assert!(self.model.is_due(self.env.now), "an alarm is due");
        fire(&mut self.model, &self.env, &mut self.out);
        self.out.pop()
    }

    /// Runs the task for call 1, returning the owner of the session's first
    /// call.
    fn run(&mut self) -> Token {
        let Some(Request::Complete { owner, .. }) = self.step(Event::Run { reply_to: reply_to(), task: task() }) else {
            panic!("expected a call");
        };
        owner
    }

    /// Runs the task for call 1 and has the LLM ask for `ls`, returning the
    /// session's owner.
    fn run_tool(&mut self) -> Token {
        let owner = self.run();
        let Some(Request::Tool { .. }) = self.step(Event::Completed { owner, completion: ls() }) else {
            panic!("expected a tool run");
        };
        owner
    }

    fn expire(&mut self) {
        self.env.now = self.env.now.checked_add(LIMITS.session.session_timeout).expect("the test stays in range");
    }
}

fn bytes(text: &[u8]) -> Box<[u8]> {
    Box::from(text)
}

fn reply_to() -> ReplyTo {
    ReplyTo::new(Token::new(1))
}

fn task() -> Task {
    Task {
        endpoint: Endpoint(0),
        model: bytes(b"model"),
        system: bytes(b"be brief"),
        tools: Box::new([Tool { name: bytes(b"ls"), description: bytes(b"lists files"), schema: bytes(b"{}") }]),
        prompt: bytes(b"fix the bug"),
        max_tokens: 1024,
    }
}

/// The LLM asks for `ls`.
fn ls() -> Completion {
    let content = Box::new([Block::ToolCall { id: bytes(b"c1"), name: bytes(b"ls"), input: bytes(b"{}") }]);
    Completion { content, stop: Stop::ToolUse, usage: Usage { input_tokens: 10, output_tokens: 5 } }
}

fn ended(outcome: Outcome, turns: u32) -> Request {
    let usage =
        Usage { input_tokens: 10_u64.saturating_mul(turns.into()), output_tokens: 5_u64.saturating_mul(turns.into()) };
    Request::Reply { to: reply_to(), report: Report::Ended { outcome, turns, usage } }
}

#[test]
fn a_run_reaches_the_session_and_its_call_comes_back_out() {
    let mut h = Harness::new();
    let Some(Request::Complete { owner: _, prompt, timeout }) =
        h.step(Event::Run { reply_to: reply_to(), task: task() })
    else {
        panic!("expected a call");
    };
    assert_eq!(timeout, LIMITS.session.call_timeout);
    let first = Message { role: Role::User, content: Box::new([Block::Text { text: bytes(b"fix the bug") }]) };
    assert_eq!(&*prompt.messages, &[first]);
    assert_eq!(h.model.sessions(), 1);
    assert_eq!(h.model.next_deadline(), Some(Time::ZERO.saturating_add(LIMITS.session.session_timeout)));
}

#[test]
fn a_completion_reaches_the_session_and_its_tool_run_comes_back_out() {
    let mut h = Harness::new();
    let owner = h.run();
    let request = h.step(Event::Completed { owner, completion: ls() });
    assert_eq!(request, Some(Request::Tool { owner, call: ToolCall { name: bytes(b"ls"), input: bytes(b"{}") } }));
}

#[test]
fn a_tool_result_reaches_the_session_and_its_next_call_comes_back_out() {
    let mut h = Harness::new();
    let owner = h.run_tool();
    let Some(Request::Complete { owner: next, prompt, timeout: _ }) =
        h.step(Event::ToolDone { owner, output: bytes(b"main.rs"), error: false })
    else {
        panic!("expected a call");
    };
    assert_eq!(next, owner);
    let result = Block::ToolResult { id: bytes(b"c1"), output: bytes(b"main.rs"), error: false };
    let last = prompt.messages.last().expect("the results go back last");
    assert_eq!(&*last.content, &[result]);
}

#[test]
fn a_failure_reaches_the_session_and_its_reply_comes_back_out() {
    let mut h = Harness::new();
    let owner = h.run();
    let reply = h.step(Event::Failed { owner, failure: Failure::Invalid });
    assert_eq!(reply, Some(ended(Outcome::Failed { failure: Failure::Invalid }, 0)));
    assert_eq!(h.model.next_deadline(), None);
    h.model.reclaim();
    assert_eq!(h.model.sessions(), 0);
}

#[test]
fn an_expired_call_is_cancelled_and_its_cancellation_reaches_the_session() {
    let mut h = Harness::new();
    let owner = h.run();
    h.expire();
    assert_eq!(h.fire(), Some(Request::Cancel { owner }));
    assert_eq!(h.step(Event::Cancelled { owner }), Some(ended(Outcome::Expired, 0)));
}

#[test]
fn an_expired_tool_run_is_cancelled_and_its_cancellation_reaches_the_session() {
    let mut h = Harness::new();
    let owner = h.run_tool();
    h.expire();
    assert_eq!(h.fire(), Some(Request::CancelTool { owner }));
    assert_eq!(h.step(Event::ToolCancelled { owner }), Some(ended(Outcome::Expired, 1)));
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
