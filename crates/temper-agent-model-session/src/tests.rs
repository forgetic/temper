//! Feed the model events, inspect the requests that come out.

use alloc::boxed::Box;

use temper_lib::{Duration, Env, Queue, Time, Token};

use crate::llm::{Block, Completion, Endpoint, Failure, Message, Prompt, Role, Stop, Tool, Usage};
use crate::{End, Event, Limits, MAX_OUT, Model, Request, Spec, ToolCall, Yield, fire, step, worst_case};

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

    /// Steps the model with `event`, which emits at most one request.
    fn step(&mut self, event: Event) -> Option<Request> {
        step(&mut self.model, &self.env, event, &mut self.out);
        self.one()
    }

    /// Fires the alarm that is due, which emits at most one request.
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

    /// Opens a session for `opener`, returning the session's name, which also
    /// owns its calls, and its first prompt.
    fn open(&mut self, opener: u64) -> (Token, Prompt) {
        step(&mut self.model, &self.env, Event::Open { opener: Token::new(opener), spec: spec() }, &mut self.out);
        let Some(Request::Opened { opener: to, session }) = self.out.pop() else {
            panic!("expected the session to open");
        };
        assert_eq!(to.raw(), opener);
        let (owner, prompt) = calling(self.one());
        assert_eq!(owner, session, "a session's calls are its own");
        (session, prompt)
    }

    /// Opens a session for opener 1, and has the LLM finish its turn.
    fn yielded(&mut self) -> Token {
        let (session, _) = self.open(1);
        let answer = completion(Box::new([text(b"done")]), Stop::EndTurn);
        drop(yielded(self.step(Event::Completed { owner: session, completion: answer })));
        session
    }

    fn after(&mut self, span: Duration) {
        self.env.now = self.env.now.checked_add(span).expect("the test stays in range");
    }
}

fn bytes(text: &[u8]) -> Box<[u8]> {
    Box::from(text)
}

fn spec() -> Spec {
    Spec {
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
        panic!("expected a call, not {request:?}");
    };
    assert_eq!(timeout, LIMITS.call_timeout);
    (owner, prompt)
}

fn running(request: Option<Request>) -> (Token, ToolCall) {
    let Some(Request::Tool { owner, call }) = request else {
        panic!("expected a tool run, not {request:?}");
    };
    (owner, call)
}

fn yielded(request: Option<Request>) -> (u64, Yield, Box<[u8]>) {
    let Some(Request::Yielded { opener, stop, text }) = request else {
        panic!("expected a yield, not {request:?}");
    };
    (opener.raw(), stop, text)
}

/// The end of opener 1's session, after `turns` completions.
fn ended(end: End, turns: u32) -> Request {
    let usage =
        Usage { input_tokens: 10_u64.saturating_mul(turns.into()), output_tokens: 5_u64.saturating_mul(turns.into()) };
    Request::Ended { opener: Token::new(1), end, turns, usage }
}

#[test]
fn a_session_runs_the_tools_it_is_asked_for_and_yields_once_the_llm_is_done() {
    let mut h = Harness::new(LIMITS);
    let (owner, prompt) = h.open(1);
    assert_eq!(prompt.messages.len(), 1);
    assert_eq!(prompt.tools, spec().tools);

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

    let done = completion(Box::new([text(b"fixed "), text(b"it")]), Stop::EndTurn);
    assert_eq!(yielded(h.step(Event::Completed { owner, completion: done })), (1, Yield::Done, bytes(b"fixed it")));
    let expiry = Time::ZERO.saturating_add(LIMITS.session_timeout);
    assert_eq!(h.model.next_deadline(), Some(expiry), "a yielded session still expires");

    assert_eq!(h.step(Event::Close { session: owner }), Some(ended(End::Closed, 2)));
    assert_eq!(h.model.next_deadline(), None);
    assert_eq!(h.model.sessions(), 1);
    h.model.reclaim();
    assert_eq!(h.model.sessions(), 0);
}

#[test]
fn a_yielded_session_goes_on_with_the_openers_message() {
    let mut h = Harness::new(LIMITS);
    let session = h.yielded();
    let (owner, prompt) = calling(h.step(Event::Continue { session, content: bytes(b"you have not finished") }));
    assert_eq!(owner, session);
    let messages = [
        Message { role: Role::User, content: Box::new([text(b"fix the bug")]) },
        Message { role: Role::Assistant, content: Box::new([text(b"done")]) },
        Message { role: Role::User, content: Box::new([text(b"you have not finished")]) },
    ];
    assert_eq!(&*prompt.messages, &messages);

    let again = completion(Box::new([text(b"finished")]), Stop::EndTurn);
    assert_eq!(yielded(h.step(Event::Completed { owner, completion: again })), (1, Yield::Done, bytes(b"finished")));
}

/// Opens a session whose LLM answers `content` with `stop`, and returns how it
/// yielded.
fn yield_on(stop: Stop, content: Box<[Block]>) -> (u64, Yield, Box<[u8]>) {
    let mut h = Harness::new(LIMITS);
    let (owner, _) = h.open(1);
    yielded(h.step(Event::Completed { owner, completion: completion(content, stop) }))
}

#[test]
fn the_llm_yields_whenever_it_stops_calling_tools() {
    let truncated = yield_on(Stop::MaxTokens, Box::new([text(b"I was say")]));
    assert_eq!(truncated, (1, Yield::Truncated, bytes(b"I was say")));
    assert_eq!(yield_on(Stop::Refusal, Box::new([text(b"no")])), (1, Yield::Refused, bytes(b"no")));
    let malformed = yield_on(Stop::ToolUse, Box::new([text(b"I will call a tool")]));
    assert_eq!(malformed, (1, Yield::Malformed, bytes(b"I will call a tool")));
    assert_eq!(yield_on(Stop::EndTurn, Box::new([])), (1, Yield::Done, bytes(b"")));
}

#[test]
fn opens_beyond_the_session_slots_are_refused_as_busy() {
    let mut h = Harness::new(Limits { sessions: 1, ..LIMITS });
    drop(h.open(1));
    let refused = h.step(Event::Open { opener: Token::new(2), spec: spec() });
    assert_eq!(refused, Some(Request::Ended { opener: Token::new(2), end: End::Busy, turns: 0, usage: Usage::ZERO }));
}

#[test]
fn specs_beyond_the_limits_are_refused_as_invalid() {
    for limits in [Limits { session_bytes: 16, ..LIMITS }, Limits { max_tokens: 1, ..LIMITS }] {
        let mut h = Harness::new(limits);
        let refused = h.step(Event::Open { opener: Token::new(1), spec: spec() });
        assert_eq!(refused, Some(ended(End::Invalid, 0)));
        assert_eq!(h.model.sessions(), 0);
    }
}

#[test]
fn transient_failures_are_retried_after_a_backoff_until_the_retries_run_out() {
    let mut h = Harness::new(LIMITS);
    let (owner, _) = h.open(1);
    for _ in 0..LIMITS.retries {
        assert_eq!(h.step(Event::Failed { owner, failure: Failure::Overloaded }), None);
        let retry = h.model.next_deadline().expect("a retry is armed");
        assert!(retry <= h.env.now.saturating_add(LIMITS.backoff_max), "the backoff is capped");
        h.env.now = retry;
        drop(calling(h.fire()));
    }
    let end = h.step(Event::Failed { owner, failure: Failure::Overloaded });
    assert_eq!(end, Some(ended(End::Failed { failure: Failure::Overloaded }, 0)));
}

#[test]
fn a_rate_limit_is_waited_out_and_lasting_failures_are_not_retried() {
    let mut h = Harness::new(LIMITS);
    let (owner, _) = h.open(2);
    let failure = Failure::RateLimited { retry_after: Duration::from_secs(5) };
    assert_eq!(h.step(Event::Failed { owner, failure }), None);
    assert_eq!(h.model.next_deadline(), Some(Time::ZERO.saturating_add(Duration::from_secs(5))));

    let (owner, _) = h.open(1);
    let end = h.step(Event::Failed { owner, failure: Failure::Unauthorized });
    assert_eq!(end, Some(ended(End::Failed { failure: Failure::Unauthorized }, 0)));
}

#[test]
fn closing_a_calling_session_cancels_its_call_and_ends_once_the_call_has_ended() {
    let mut h = Harness::new(LIMITS);
    let (owner, _) = h.open(1);
    assert_eq!(h.step(Event::Close { session: owner }), Some(Request::Cancel { owner }));
    assert_eq!(h.model.next_deadline(), None, "a closing session waits for nothing but its call");
    assert_eq!(h.step(Event::Cancelled { owner }), Some(ended(End::Closed, 0)));
}

#[test]
fn a_completion_that_wins_the_race_with_a_close_still_ends_the_session() {
    let mut h = Harness::new(LIMITS);
    let (owner, _) = h.open(1);
    assert_eq!(h.step(Event::Close { session: owner }), Some(Request::Cancel { owner }));
    let late = completion(Box::new([tool_call(b"c1", b"ls")]), Stop::ToolUse);
    assert_eq!(h.step(Event::Completed { owner, completion: late }), Some(ended(End::Closed, 0)));

    let (owner, _) = h.open(1);
    assert_eq!(h.step(Event::Close { session: owner }), Some(Request::Cancel { owner }));
    assert_eq!(h.step(Event::Failed { owner, failure: Failure::Overloaded }), Some(ended(End::Closed, 0)));
}

#[test]
fn closing_a_tooling_session_cancels_its_tool() {
    let mut h = Harness::new(LIMITS);
    let (owner, _) = h.open(1);
    let content = Box::new([tool_call(b"c1", b"ls")]);
    drop(running(h.step(Event::Completed { owner, completion: completion(content, Stop::ToolUse) })));
    assert_eq!(h.step(Event::Close { session: owner }), Some(Request::CancelTool { owner }));
    assert_eq!(h.step(Event::ToolCancelled { owner }), Some(ended(End::Closed, 1)));

    // The tool won the race.
    let (owner, _) = h.open(1);
    let content = Box::new([tool_call(b"c1", b"ls")]);
    drop(running(h.step(Event::Completed { owner, completion: completion(content, Stop::ToolUse) })));
    assert_eq!(h.step(Event::Close { session: owner }), Some(Request::CancelTool { owner }));
    let output = bytes(b"main.rs");
    assert_eq!(h.step(Event::ToolDone { owner, output, error: false }), Some(ended(End::Closed, 1)));
}

#[test]
fn closing_a_session_with_nothing_in_flight_ends_it_at_once() {
    let mut h = Harness::new(LIMITS);
    let session = h.yielded();
    assert_eq!(h.step(Event::Close { session }), Some(ended(End::Closed, 1)));
    assert_eq!(h.model.next_deadline(), None);

    let (owner, _) = h.open(1);
    assert_eq!(h.step(Event::Failed { owner, failure: Failure::Unavailable }), None);
    assert_eq!(h.step(Event::Close { session: owner }), Some(ended(End::Closed, 0)));
    assert_eq!(h.model.next_deadline(), None, "the retry is called off");
}

#[test]
fn closing_a_session_that_is_already_closing_changes_nothing() {
    let mut h = Harness::new(LIMITS);
    let (owner, _) = h.open(1);
    h.after(LIMITS.session_timeout);
    assert_eq!(h.fire(), Some(Request::Cancel { owner }));
    assert_eq!(h.step(Event::Close { session: owner }), None);
    assert_eq!(h.step(Event::Cancelled { owner }), Some(ended(End::Expired, 0)));
}

#[test]
fn a_handle_to_a_session_that_has_ended_is_dropped() {
    let mut h = Harness::new(Limits { sessions: 1, ..LIMITS });
    let session = h.yielded();
    assert_eq!(h.step(Event::Close { session }), Some(ended(End::Closed, 1)));
    // Ended, and not yet reclaimed.
    assert_eq!(h.step(Event::Continue { session, content: bytes(b"go on") }), None);
    assert_eq!(h.step(Event::Close { session }), None);
    // Reclaimed, and its slot taken by another session.
    h.model.reclaim();
    let (other, _) = h.open(2);
    assert_ne!(other, session, "a handle is never reused");
    assert_eq!(h.step(Event::Close { session }), None);
    assert_eq!(h.step(Event::Continue { session, content: bytes(b"go on") }), None);
    assert_eq!(h.model.sessions(), 1, "the other session lives on");
}

#[test]
fn an_expiring_session_cancels_its_call_and_ends_once_the_call_has_ended() {
    let mut h = Harness::new(LIMITS);
    let (owner, _) = h.open(1);
    h.after(LIMITS.session_timeout);
    assert_eq!(h.fire(), Some(Request::Cancel { owner }));
    assert_eq!(h.model.next_deadline(), None);
    // The completion won the race with the cancel; the session still expires.
    let late = completion(Box::new([text(b"too late")]), Stop::EndTurn);
    assert_eq!(h.step(Event::Completed { owner, completion: late }), Some(ended(End::Expired, 0)));
}

#[test]
fn an_expiring_session_cancels_its_tool() {
    let mut h = Harness::new(LIMITS);
    let (owner, _) = h.open(1);
    let content = Box::new([tool_call(b"c1", b"ls")]);
    drop(running(h.step(Event::Completed { owner, completion: completion(content, Stop::ToolUse) })));
    h.after(LIMITS.session_timeout);
    assert_eq!(h.fire(), Some(Request::CancelTool { owner }));
    assert_eq!(h.step(Event::ToolCancelled { owner }), Some(ended(End::Expired, 1)));
}

#[test]
fn an_expiring_session_in_backoff_or_yielded_ends_at_once() {
    let mut h = Harness::new(Limits {
        backoff_base: Duration::from_secs(3600),
        backoff_max: Duration::from_secs(3600),
        ..LIMITS
    });
    let (owner, _) = h.open(1);
    assert_eq!(h.step(Event::Failed { owner, failure: Failure::Unavailable }), None);
    h.after(LIMITS.session_timeout);
    assert_eq!(h.fire(), Some(ended(End::Expired, 0)));
    assert_eq!(h.model.next_deadline(), None);

    let mut h = Harness::new(LIMITS);
    let _session: Token = h.yielded();
    h.after(LIMITS.session_timeout);
    assert_eq!(h.fire(), Some(ended(End::Expired, 1)));
    assert_eq!(h.model.next_deadline(), None);
}

#[test]
fn the_llm_gets_as_many_turns_as_the_limits_allow() {
    let mut h = Harness::new(Limits { turns: 1, ..LIMITS });
    let (owner, _) = h.open(1);
    let content = Box::new([tool_call(b"c1", b"ls")]);
    let end = h.step(Event::Completed { owner, completion: completion(content, Stop::ToolUse) });
    assert_eq!(end, Some(ended(End::TurnLimit, 1)));

    let session = h.yielded();
    let end = h.step(Event::Continue { session, content: bytes(b"go on") });
    assert_eq!(end, Some(ended(End::TurnLimit, 1)));
}

#[test]
fn a_conversation_that_outgrows_its_bytes_ends_the_session() {
    let mut h = Harness::new(Limits { session_bytes: 1024, ..LIMITS });
    let (owner, _) = h.open(1);
    let content = Box::new([tool_call(b"c1", b"ls")]);
    drop(running(h.step(Event::Completed { owner, completion: completion(content, Stop::ToolUse) })));
    let huge = Box::from([b'x'; 2048].as_slice());
    let end = h.step(Event::ToolDone { owner, output: huge, error: false });
    assert_eq!(end, Some(ended(End::TranscriptFull, 1)));

    let session = h.yielded();
    let huge = Box::from([b'x'; 2048].as_slice());
    assert_eq!(h.step(Event::Continue { session, content: huge }), Some(ended(End::TranscriptFull, 1)));

    h.model.reclaim();
    let (owner, _) = h.open(1);
    let huge = Box::new([Block::Text { text: Box::from([b'x'; 2048].as_slice()) }]);
    let end = h.step(Event::Completed { owner, completion: completion(huge, Stop::EndTurn) });
    assert_eq!(end, Some(ended(End::TranscriptFull, 1)), "a yield the session could not continue from ends it");
}

#[test]
fn a_transcript_with_no_room_for_another_message_ends_the_session() {
    let mut h = Harness::new(Limits { messages: 2, ..LIMITS });
    let session = h.yielded();
    assert_eq!(h.step(Event::Continue { session, content: bytes(b"go on") }), Some(ended(End::TranscriptFull, 1)));
}

#[test]
fn the_worst_case_is_bounded_or_refused() {
    let bytes = worst_case(&LIMITS).expect("the test limits fit");
    assert!(bytes > 2 * LIMITS.session_bytes, "every session may hold its bytes");
    assert_eq!(worst_case(&Limits { sessions: u32::MAX, session_bytes: u64::MAX, ..LIMITS }), None);
}
