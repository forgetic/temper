//! Feed the model events, inspect the requests that come out.

use alloc::boxed::Box;

use temper_lib::{Duration, Env, Queue, ReplyTo, Time, Token};

use crate::llm::{Block, Completion, Endpoint, Failure, Prompt, Stop, Tool, Usage};
use crate::{Event, Limits, MAX_OUT, Model, Outcome, Report, Request, Task, ToolCall, fire, step, worst_case};

const LIMITS: Limits = Limits {
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
};

/// The model, its environment, and room for one step's output.
struct Harness {
    model: Model,
    env: Env<Limits>,
    out: Queue<Request>,
}

impl Harness {
    fn new(limits: Limits) -> Harness {
        Harness {
            model: Model::new(&limits, 1),
            env: Env { now: Time::ZERO, limits },
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

    fn run(&mut self, call: u64) -> Option<Request> {
        self.step(Event::Run { reply_to: ReplyTo::new(Token::new(call)), task: task() })
    }

    fn after(&mut self, span: Duration) {
        self.env.now = self.env.now.checked_add(span).expect("the test stays in range");
    }
}

fn bytes(text: &[u8]) -> Box<[u8]> {
    Box::from(text)
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

fn text(text: &[u8]) -> Block {
    Block::Text { text: bytes(text) }
}

fn tool_call(id: &[u8], name: &[u8]) -> Block {
    Block::ToolCall { id: bytes(id), name: bytes(name), input: bytes(b"{}") }
}

fn completion(content: Box<[Block]>, stop: Stop) -> Completion {
    Completion { content, stop, usage: Usage { input_tokens: 10, output_tokens: 5 } }
}

fn calling(request: Option<Request>) -> (Token, Prompt) {
    let Some(Request::Complete { owner, prompt, timeout }) = request else {
        panic!("expected a call");
    };
    assert_eq!(timeout, LIMITS.call_timeout);
    (owner, prompt)
}

fn running(request: Option<Request>) -> (Token, ToolCall) {
    let Some(Request::Tool { owner, call }) = request else {
        panic!("expected a tool run");
    };
    (owner, call)
}

fn replied(request: Option<Request>) -> (u64, Report) {
    let Some(Request::Reply { to, report }) = request else {
        panic!("expected a reply");
    };
    (to.into_token().raw(), report)
}

fn ended(outcome: Outcome, turns: u32) -> Report {
    let usage =
        Usage { input_tokens: 10_u64.saturating_mul(turns.into()), output_tokens: 5_u64.saturating_mul(turns.into()) };
    Report::Ended { outcome, turns, usage }
}

#[test]
fn a_session_runs_the_tools_it_is_asked_for_and_answers_once_the_llm_is_done() {
    let mut h = Harness::new(LIMITS);
    let (owner, prompt) = calling(h.run(1));
    assert_eq!(prompt.messages.len(), 1);
    assert_eq!(prompt.tools, task().tools);

    let content = Box::new([text(b"let me look"), tool_call(b"c1", b"ls"), tool_call(b"c2", b"cat")]);
    let (tool_owner, call) =
        running(h.step(Event::Completed { owner, completion: completion(content, Stop::ToolUse) }));
    assert_eq!((tool_owner, &*call.name), (owner, &b"ls"[..]));
    let (_, call) = running(h.step(Event::ToolDone { owner, output: bytes(b"main.rs"), error: false }));
    assert_eq!(&*call.name, b"cat");

    let (_, prompt) = calling(h.step(Event::ToolDone { owner, output: bytes(b"no such file"), error: true }));
    assert_eq!(prompt.messages.len(), 3);
    let results = [
        Block::ToolResult { id: bytes(b"c1"), output: bytes(b"main.rs"), error: false },
        Block::ToolResult { id: bytes(b"c2"), output: bytes(b"no such file"), error: true },
    ];
    assert_eq!(&*prompt.messages[2].content, &results);

    let done = completion(Box::new([text(b"fixed")]), Stop::EndTurn);
    let report = replied(h.step(Event::Completed { owner, completion: done }));
    assert_eq!(report, (1, ended(Outcome::Done { content: Box::new([text(b"fixed")]) }, 2)));
    assert_eq!(h.model.next_deadline(), None);
    assert_eq!(h.model.sessions(), 1);
    h.model.reclaim();
    assert_eq!(h.model.sessions(), 0);
}

#[test]
fn runs_beyond_the_session_slots_are_refused_as_busy() {
    let mut h = Harness::new(Limits { sessions: 1, ..LIMITS });
    drop(calling(h.run(1)));
    assert_eq!(replied(h.run(2)), (2, Report::Busy));
}

#[test]
fn tasks_beyond_the_limits_are_refused_as_invalid() {
    let mut h = Harness::new(Limits { session_bytes: 16, ..LIMITS });
    assert_eq!(replied(h.run(1)), (1, Report::Invalid));
    let mut h = Harness::new(Limits { max_tokens: 1, ..LIMITS });
    assert_eq!(replied(h.run(1)), (1, Report::Invalid));
    assert_eq!(h.model.sessions(), 0);
}

#[test]
fn transient_failures_are_retried_after_a_backoff_until_the_retries_run_out() {
    let mut h = Harness::new(LIMITS);
    let (owner, _) = calling(h.run(1));
    for _ in 0..LIMITS.retries {
        assert_eq!(h.step(Event::Failed { owner, failure: Failure::Overloaded }), None);
        let retry = h.model.next_deadline().expect("a retry is armed");
        assert!(retry <= h.env.now.saturating_add(LIMITS.backoff_max), "the backoff is capped");
        h.env.now = retry;
        drop(calling(h.fire()));
    }
    let report = replied(h.step(Event::Failed { owner, failure: Failure::Overloaded }));
    assert_eq!(report, (1, ended(Outcome::Failed { failure: Failure::Overloaded }, 0)));
}

#[test]
fn a_rate_limit_is_waited_out_and_lasting_failures_are_not_retried() {
    let mut h = Harness::new(LIMITS);
    let (owner, _) = calling(h.run(1));
    let failure = Failure::RateLimited { retry_after: Duration::from_secs(5) };
    assert_eq!(h.step(Event::Failed { owner, failure }), None);
    assert_eq!(h.model.next_deadline(), Some(Time::ZERO.saturating_add(Duration::from_secs(5))));

    let (owner, _) = calling(h.run(2));
    let report = replied(h.step(Event::Failed { owner, failure: Failure::Unauthorized }));
    assert_eq!(report, (2, ended(Outcome::Failed { failure: Failure::Unauthorized }, 0)));
}

#[test]
fn an_expiring_session_cancels_its_call_and_answers_once_the_call_has_ended() {
    let mut h = Harness::new(LIMITS);
    let (owner, _) = calling(h.run(1));
    h.after(LIMITS.session_timeout);
    assert_eq!(h.fire(), Some(Request::Cancel { owner }));
    assert_eq!(h.model.next_deadline(), None);
    // The completion won the race with the cancel; the session still expires.
    let late = completion(Box::new([text(b"too late")]), Stop::EndTurn);
    assert_eq!(replied(h.step(Event::Completed { owner, completion: late })), (1, ended(Outcome::Expired, 0)));
}

#[test]
fn an_expiring_session_cancels_its_tool() {
    let mut h = Harness::new(LIMITS);
    let (owner, _) = calling(h.run(1));
    let content = Box::new([tool_call(b"c1", b"ls")]);
    drop(running(h.step(Event::Completed { owner, completion: completion(content, Stop::ToolUse) })));
    h.after(LIMITS.session_timeout);
    assert_eq!(h.fire(), Some(Request::CancelTool { owner }));
    assert_eq!(replied(h.step(Event::ToolCancelled { owner })), (1, ended(Outcome::Expired, 1)));
}

#[test]
fn an_expiring_session_in_backoff_answers_at_once() {
    let mut h = Harness::new(Limits {
        backoff_base: Duration::from_secs(3600),
        backoff_max: Duration::from_secs(3600),
        ..LIMITS
    });
    let (owner, _) = calling(h.run(1));
    assert_eq!(h.step(Event::Failed { owner, failure: Failure::Unavailable }), None);
    h.after(LIMITS.session_timeout);
    assert_eq!(replied(h.fire()), (1, ended(Outcome::Expired, 0)));
    assert_eq!(h.model.next_deadline(), None);
}

#[test]
fn the_llm_gets_as_many_turns_as_the_limits_allow() {
    let mut h = Harness::new(Limits { turns: 1, ..LIMITS });
    let (owner, _) = calling(h.run(1));
    let content = Box::new([tool_call(b"c1", b"ls")]);
    let report = replied(h.step(Event::Completed { owner, completion: completion(content, Stop::ToolUse) }));
    assert_eq!(report, (1, ended(Outcome::TurnLimit, 1)));
}

#[test]
fn tool_use_without_tool_calls_is_malformed() {
    let mut h = Harness::new(LIMITS);
    let (owner, _) = calling(h.run(1));
    let content = Box::new([text(b"I will call a tool")]);
    let report = replied(h.step(Event::Completed { owner, completion: completion(content, Stop::ToolUse) }));
    assert_eq!(report, (1, ended(Outcome::Malformed, 1)));
}

#[test]
fn a_conversation_that_outgrows_its_bytes_ends_the_session() {
    let mut h = Harness::new(Limits { session_bytes: 1024, ..LIMITS });
    let (owner, _) = calling(h.run(1));
    let content = Box::new([tool_call(b"c1", b"ls")]);
    drop(running(h.step(Event::Completed { owner, completion: completion(content, Stop::ToolUse) })));
    let huge = Box::from([b'x'; 2048].as_slice());
    let report = replied(h.step(Event::ToolDone { owner, output: huge, error: false }));
    assert_eq!(report, (1, ended(Outcome::TranscriptFull, 1)));
}

#[test]
fn the_worst_case_is_bounded_or_refused() {
    let bytes = worst_case(&LIMITS).expect("the test limits fit");
    assert!(bytes > 2 * LIMITS.session_bytes, "every session may hold its bytes");
    assert_eq!(worst_case(&Limits { sessions: u32::MAX, session_bytes: u64::MAX, ..LIMITS }), None);
}
