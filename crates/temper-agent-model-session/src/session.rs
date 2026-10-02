//! LLM sessions: a conversation with an LLM, driven turn by turn until the LLM
//! yields, a limit or the budget ends it, or its opener closes it.
//!
//! An `Open` opens a session and the session calls the LLM. While the LLM asks
//! for tools, the session runs them one at a time and sends their results back
//! in another call. When the LLM stops calling tools, the session yields to its
//! opener, which continues it with a new user message or closes it. A session
//! ends once, and only once nothing it asked for is in flight: closing cancels
//! what is, and waits for it to settle (5.3).
//!
//! The transition table. Every other cell is unreachable by the boundary's
//! contract: one terminal event per request, a request only from the states
//! that wait for its terminal event, and a `Continue` only to a yielded
//! session.
//!
//! ```text
//! state          event or alarm               next
//! (none)         open, admitted               Calling      opened; call the LLM
//!                open, busy or invalid        (none)       ended: busy, invalid
//! Calling        completed, tool use          Tooling      run the first tool
//!                completed, otherwise         Yielded      yielded
//!                failed, transient            Backoff
//!                failed, otherwise            Closed       ended: failed
//!                close, expiry                Closing      cancel the call
//! Backoff        retry                        Calling      call again
//!                close, expiry                Closed       ended: closed, out of time
//! Tooling        tool done, more tools        Tooling      run the next tool
//!                tool done, last tool         Calling      send the results
//!                close, expiry                Closing      cancel the tool
//! Yielded        continue                     Calling      call with the new message
//!                close, expiry                Closed       ended: closed, out of time
//! Closing        what was cancelled ends      Closed       ended
//!                close                        Closing      (already closing)
//! Closed         continue, close              Closed       (dropped: the handle is stale)
//! ```
//!
//! Every completion that comes back is reported to the opener as `Used`, in
//! Calling and in Closing alike, and checked against the budget: one that
//! took a dimension past its end ends the session at once. Before it calls
//! the LLM, the session checks that it has turns, input and output tokens and
//! time left, and ends instead if not, so it never starts a completion it
//! may not pay for. Either end names the dimension. A byte limit that a completion, a tool's result or a new message would
//! cross ends the session in place of the transition it would have made.
//!
//! The expiry alarm, set for when the time budget runs out, runs in Calling,
//! Backoff, Tooling and Yielded; the retry alarm in Backoff. Both follow from
//! the state, in one place ([`follow`]), which also retires a session once it
//! is Closed.
//!
//! Every transition tells what happened as facts: the entry point tells of
//! the event it was given (a completion or a tool run ending, a retry), and
//! [`tell`] of the requests the transition made, in one place.

use alloc::boxed::Box;
use core::mem::{self, size_of};

use temper_lib::{Deadlines, Duration, Env, Id, List, Queue, Rng, Slab, Time, Token, Writer};

use crate::boundary::{Budget, Dimension, End, Request, Spec, ToolCall, Yield};
use crate::facts::{Fact, Facts};
use crate::limits::Limits;
use crate::llm::{Block, Completion, Endpoint, Failure, Message, Prompt, Role, Stop, Tool, Usage};
use crate::model::Model;

#[derive(Debug)]
pub(crate) struct Session {
    conversation: Conversation,
    state: State,
}

/// What a session holds in every state.
#[derive(Debug)]
struct Conversation {
    /// The opener's token, echoed on every record back to it.
    opener: Token,
    endpoint: Endpoint,
    model: Box<[u8]>,
    system: Box<[u8]>,
    tools: Box<[Tool]>,
    max_tokens: u32,
    /// The conversation so far, oldest first, starting with the spec's prompt.
    transcript: List<Message>,
    /// Bytes held, counted against `Limits::session_bytes`.
    bytes: u64,
    /// What the session may spend, and what it has: completions received, and
    /// the tokens they used.
    budget: Budget,
    turns: u32,
    usage: Usage,
    /// When the time budget runs out.
    expires: Time,
}

#[derive(Debug)]
enum State {
    /// A call is in flight, after `attempt` retries.
    Calling { attempt: u32 },
    /// The last call failed transiently: calling again at `until`.
    Backoff { attempt: u32, until: Time },
    /// Running the tool calls of the last assistant message.
    Tooling { tools: Tools },
    /// The LLM stopped calling tools: waiting for the opener to continue or
    /// close the session.
    Yielded,
    /// Ending with `end`, once what was cancelled has settled.
    Closing { end: End },
    /// Terminal: holds nothing.
    Closed,
}

/// The tool calls of the last assistant message, run one at a time.
#[derive(Debug)]
struct Tools {
    /// The block of the call in flight.
    block: u32,
    /// The results so far, in call order, with room for one per call.
    results: List<Block>,
}

/// A session's timers, named by what they are for.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(crate) enum Alarm {
    Expiry { session: Id<Session> },
    Retry { session: Id<Session> },
}

// Entry points, one per event or alarm: look the session up, take its state
// out, run the cell's handler, then tell what happened and follow the new
// state ([`conclude`]).

pub(crate) fn open(model: &mut Model, env: &Env<Limits>, opener: Token, spec: Spec, out: &mut Queue<Request>) {
    let mark = out.len();
    if model.sessions.is_full() {
        refuse(&mut model.facts, opener, End::Busy, out);
        return;
    }
    let Some(conversation) = admit(opener, spec, &env.limits, env.now) else {
        refuse(&mut model.facts, opener, End::Invalid, out);
        return;
    };
    // Closed until the first call is made, which a budget just admitted pays
    // for.
    let session = Session { conversation, state: State::Closed };
    let id = model.sessions.insert(session).expect("checked for room above");
    out.push(Request::Opened { opener, session: id.token() });
    let session = model.sessions.get_mut(id).expect("inserted above");
    session.state = call(&session.conversation, id, 0, env, out);
    conclude(model, id, out, mark);
}

pub(crate) fn resume(
    model: &mut Model,
    env: &Env<Limits>,
    session: Token,
    content: Box<[u8]>,
    out: &mut Queue<Request>,
) {
    let mark = out.len();
    let Some(id) = addressed(&model.sessions, session) else {
        return;
    };
    let session = model.sessions.get_mut(id).expect("addressed above");
    let state = mem::replace(&mut session.state, State::Closed);
    let conversation = &mut session.conversation;
    session.state = match state {
        State::Yielded => resumed(conversation, id, content, env, out),
        State::Calling { .. } | State::Backoff { .. } | State::Tooling { .. } | State::Closing { .. } => {
            unreachable!("an opener continues a session only while it is yielded")
        }
        State::Closed => unreachable!("an addressed session has not ended"),
    };
    conclude(model, id, out, mark);
}

pub(crate) fn close(model: &mut Model, session: Token, out: &mut Queue<Request>) {
    let mark = out.len();
    let Some(id) = addressed(&model.sessions, session) else {
        return;
    };
    let session = model.sessions.get_mut(id).expect("addressed above");
    let state = mem::replace(&mut session.state, State::Closed);
    session.state = match state {
        State::Calling { attempt: _ } => cancel_call(id, End::Closed, out),
        State::Backoff { attempt: _, until: _ } | State::Yielded => finish(&session.conversation, End::Closed, out),
        State::Tooling { tools: _ } => cancel_tool(id, End::Closed, out),
        // It is already ending, with the end it had first.
        State::Closing { end } => State::Closing { end },
        State::Closed => unreachable!("an addressed session has not ended"),
    };
    conclude(model, id, out, mark);
}

pub(crate) fn completed(
    model: &mut Model,
    env: &Env<Limits>,
    owner: Token,
    completion: Completion,
    out: &mut Queue<Request>,
) {
    let mark = out.len();
    let id = Id::from_token(owner);
    let session = model.sessions.get_mut(id).expect("a session lives until its requests have ended");
    let (opener, stop, blocks) = (session.conversation.opener, completion.stop, count(completion.content.len()));
    model.facts.push(Fact::CompletionAnswered { opener, stop, blocks });
    let state = mem::replace(&mut session.state, State::Closed);
    let conversation = &mut session.conversation;
    session.state = match state {
        State::Calling { attempt: _ } => answered(conversation, id, completion, env, out),
        State::Closing { end } => answered_late(conversation, end, completion.usage, out),
        State::Backoff { .. } | State::Tooling { .. } | State::Yielded | State::Closed => {
            unreachable!("a completion ends a call in flight")
        }
    };
    conclude(model, id, out, mark);
}

pub(crate) fn failed(model: &mut Model, env: &Env<Limits>, owner: Token, failure: Failure, out: &mut Queue<Request>) {
    let mark = out.len();
    let id = Id::from_token(owner);
    let session = model.sessions.get_mut(id).expect("a session lives until its requests have ended");
    let opener = session.conversation.opener;
    model.facts.push(Fact::CompletionFailed { opener, failure });
    let state = mem::replace(&mut session.state, State::Closed);
    let conversation = &mut session.conversation;
    session.state = match state {
        State::Calling { attempt } => call_failed(conversation, attempt, failure, &mut model.rng, env, out),
        State::Closing { end } => finish(conversation, end, out),
        State::Backoff { .. } | State::Tooling { .. } | State::Yielded | State::Closed => {
            unreachable!("a failure ends a call in flight")
        }
    };
    // A retry is told with its wait: no request goes out until then.
    match &session.state {
        State::Backoff { attempt, until } => {
            let delay = until.saturating_since(env.now);
            model.facts.push(Fact::CompletionRetried { opener, attempt: *attempt, delay });
        }
        State::Calling { .. } | State::Tooling { .. } | State::Yielded | State::Closing { .. } | State::Closed => {}
    }
    conclude(model, id, out, mark);
}

pub(crate) fn cancelled(model: &mut Model, owner: Token, out: &mut Queue<Request>) {
    let mark = out.len();
    let id = Id::from_token(owner);
    let session = model.sessions.get_mut(id).expect("a session lives until its requests have ended");
    model.facts.push(Fact::CompletionCancelled { opener: session.conversation.opener });
    let state = mem::replace(&mut session.state, State::Closed);
    session.state = match state {
        State::Closing { end } => finish(&session.conversation, end, out),
        State::Calling { .. } | State::Backoff { .. } | State::Tooling { .. } | State::Yielded | State::Closed => {
            unreachable!("a cancellation answers a cancel, sent only on the way to Closing")
        }
    };
    conclude(model, id, out, mark);
}

pub(crate) fn tool_done(
    model: &mut Model,
    env: &Env<Limits>,
    owner: Token,
    output: Box<[u8]>,
    error: bool,
    out: &mut Queue<Request>,
) {
    let mark = out.len();
    let id = Id::from_token(owner);
    let session = model.sessions.get_mut(id).expect("a session lives until its requests have ended");
    let fact = Fact::ToolFinished { opener: session.conversation.opener, output: size(&output), error };
    model.facts.push(fact);
    let state = mem::replace(&mut session.state, State::Closed);
    let conversation = &mut session.conversation;
    session.state = match state {
        State::Tooling { tools } => {
            let result = Block::ToolResult { id: call_id(conversation, tools.block), output, error };
            tool_ran(conversation, id, tools, result, env, out)
        }
        State::Closing { end } => finish(conversation, end, out),
        State::Calling { .. } | State::Backoff { .. } | State::Yielded | State::Closed => {
            unreachable!("a tool result ends a tool run in flight")
        }
    };
    conclude(model, id, out, mark);
}

pub(crate) fn tool_cancelled(model: &mut Model, owner: Token, out: &mut Queue<Request>) {
    let mark = out.len();
    let id = Id::from_token(owner);
    let session = model.sessions.get_mut(id).expect("a session lives until its requests have ended");
    model.facts.push(Fact::ToolCancelled { opener: session.conversation.opener });
    let state = mem::replace(&mut session.state, State::Closed);
    session.state = match state {
        State::Closing { end } => finish(&session.conversation, end, out),
        State::Calling { .. } | State::Backoff { .. } | State::Tooling { .. } | State::Yielded | State::Closed => {
            unreachable!("a cancellation answers a cancel, sent only on the way to Closing")
        }
    };
    conclude(model, id, out, mark);
}

pub(crate) fn expire(model: &mut Model, id: Id<Session>, out: &mut Queue<Request>) {
    let mark = out.len();
    let session = model.sessions.get_mut(id).expect("an alarm is cancelled before its session closes");
    let state = mem::replace(&mut session.state, State::Closed);
    session.state = match state {
        State::Calling { attempt: _ } => cancel_call(id, OUT_OF_TIME, out),
        State::Backoff { attempt: _, until: _ } | State::Yielded => finish(&session.conversation, OUT_OF_TIME, out),
        State::Tooling { tools: _ } => cancel_tool(id, OUT_OF_TIME, out),
        State::Closing { .. } | State::Closed => {
            unreachable!("the expiry alarm runs only in Calling, Backoff, Tooling and Yielded")
        }
    };
    conclude(model, id, out, mark);
}

pub(crate) fn retry(model: &mut Model, env: &Env<Limits>, id: Id<Session>, out: &mut Queue<Request>) {
    let mark = out.len();
    let session = model.sessions.get_mut(id).expect("an alarm is cancelled before its session closes");
    let state = mem::replace(&mut session.state, State::Closed);
    session.state = match state {
        State::Backoff { attempt, until: _ } => call(&session.conversation, id, attempt, env, out),
        State::Calling { .. } | State::Tooling { .. } | State::Yielded | State::Closing { .. } | State::Closed => {
            unreachable!("the retry alarm runs only in Backoff")
        }
    };
    conclude(model, id, out, mark);
}

/// The session an opener's handle names, or `None` if it has ended: the handle
/// travelled down while the session's `Ended` travelled up, and is dropped
/// (5.2).
fn addressed(sessions: &Slab<Session>, session: Token) -> Option<Id<Session>> {
    let id = Id::from_token(session);
    match &sessions.get(id)?.state {
        State::Calling { .. }
        | State::Backoff { .. }
        | State::Tooling { .. }
        | State::Yielded
        | State::Closing { .. } => Some(id),
        State::Closed => None,
    }
}

/// Applied after every transition: what it tells, and what the new state
/// implies. `mark` is where the requests the transition made begin in `out`.
fn conclude(model: &mut Model, id: Id<Session>, out: &Queue<Request>, mark: u32) {
    let session = model.sessions.get(id).expect("a session lives until it is retired");
    tell(&mut model.facts, session, out, mark);
    follow(&mut model.sessions, &mut model.alarms, id);
}

/// What a transition tells, derived from the requests it made: a fact for
/// each but the cancels, whose outcome is told when it comes.
fn tell(facts: &mut Facts, session: &Session, out: &Queue<Request>, mark: u32) {
    let opener = session.conversation.opener;
    let made = usize::try_from(mark).expect("a u32 fits in a usize");
    for request in out.iter().skip(made) {
        let fact = match request {
            Request::Opened { opener, session: _ } => Fact::Opened { opener: *opener },
            Request::Yielded { opener, stop, text: _ } => Fact::Yielded { opener: *opener, stop: *stop },
            Request::Used { opener, usage } => Fact::Used { opener: *opener, usage: *usage },
            Request::Ended { opener, end, turns, usage } => {
                Fact::Ended { opener: *opener, end: *end, turns: *turns, usage: *usage }
            }
            Request::Complete { owner: _, prompt, timeout: _ } => {
                let (messages, max_tokens) = (count(prompt.messages.len()), prompt.max_tokens);
                Fact::CompletionStarted { opener, attempt: attempt(&session.state), messages, max_tokens }
            }
            Request::Tool { owner: _, call: _ } => Fact::ToolStarted { opener, block: block(&session.state) },
            Request::Cancel { .. } | Request::CancelTool { .. } => continue,
        };
        facts.push(fact);
    }
}

/// The retries of the call a session has just asked for.
fn attempt(state: &State) -> u32 {
    match state {
        State::Calling { attempt } => *attempt,
        State::Backoff { .. } | State::Tooling { .. } | State::Yielded | State::Closing { .. } | State::Closed => {
            unreachable!("a session that asks for a completion is calling")
        }
    }
}

/// The block of the tool call a session has just asked to run.
fn block(state: &State) -> u32 {
    match state {
        State::Tooling { tools } => tools.block,
        State::Calling { .. } | State::Backoff { .. } | State::Yielded | State::Closing { .. } | State::Closed => {
            unreachable!("a session that runs a tool is tooling")
        }
    }
}

/// What a session's state implies, applied after every transition: which
/// alarms run, and whether the session is retired.
fn follow(sessions: &mut Slab<Session>, alarms: &mut Deadlines<Alarm>, id: Id<Session>) {
    let session = sessions.get(id).expect("a session lives until it is retired");
    let expires = session.conversation.expires;
    let (expiry, retry, closed) = match &session.state {
        State::Calling { .. } | State::Tooling { .. } | State::Yielded => (Some(expires), None, false),
        State::Backoff { until, .. } => (Some(expires), Some(*until), false),
        State::Closing { .. } => (None, None, false),
        State::Closed => (None, None, true),
    };
    set(alarms, Alarm::Expiry { session: id }, expiry);
    set(alarms, Alarm::Retry { session: id }, retry);
    if closed {
        sessions.retire(id);
    }
}

fn set(alarms: &mut Deadlines<Alarm>, alarm: Alarm, at: Option<Time>) {
    if let Some(at) = at {
        alarms.arm(alarm, at).expect("the alarm table has room for two alarms per session");
    } else {
        alarms.cancel(alarm);
    }
}

// Cell handlers: each takes the source state's data by value and returns the
// target state.

/// Calling, completed: the LLM produced its next message.
fn answered(
    conversation: &mut Conversation,
    id: Id<Session>,
    completion: Completion,
    env: &Env<Limits>,
    out: &mut Queue<Request>,
) -> State {
    used(conversation, completion.usage, out);
    if let Some(spent) = overspent(conversation) {
        return finish(conversation, End::Budget { spent }, out);
    }
    match completion.stop {
        Stop::ToolUse => use_tools(conversation, id, completion.content, &env.limits, out),
        Stop::EndTurn => pause(conversation, Yield::Done, completion.content, &env.limits, out),
        Stop::MaxTokens => pause(conversation, Yield::Truncated, completion.content, &env.limits, out),
        Stop::Refusal => pause(conversation, Yield::Refused, completion.content, &env.limits, out),
    }
}

/// Closing, completed: the call won the race with its cancel. The session
/// ends as it was going to, but the provider counted the tokens, and so does
/// the session.
fn answered_late(conversation: &mut Conversation, end: End, usage: Usage, out: &mut Queue<Request>) -> State {
    used(conversation, usage, out);
    finish(conversation, end, out)
}

/// Calling, completed with tool use: record the message and run its first tool.
fn use_tools(
    conversation: &mut Conversation,
    id: Id<Session>,
    content: Box<[Block]>,
    limits: &Limits,
    out: &mut Queue<Request>,
) -> State {
    let Some(first) = next_tool_call(&content, 0) else {
        return pause(conversation, Yield::Malformed, content, limits, out);
    };
    let calls = tool_calls(&content).expect("blocks counted in a u32 count their calls in one");
    // The results go back in another message.
    if conversation.transcript.room() < 2 || !charge(conversation, held(&content, calls), limits) {
        return finish(conversation, End::TranscriptFull, out);
    }
    conversation.transcript.push(Message { role: Role::Assistant, content }).expect("checked for room above");
    out.push(run_tool(conversation, id, first));
    State::Tooling { tools: Tools { block: first, results: List::with_capacity(calls) } }
}

/// Calling, completed without tools to run: record the message, and yield to
/// the opener with its text.
fn pause(
    conversation: &mut Conversation,
    stop: Yield,
    content: Box<[Block]>,
    limits: &Limits,
    out: &mut Queue<Request>,
) -> State {
    // A session continues from its transcript, so the message must fit there.
    if conversation.transcript.room() == 0 || !charge(conversation, content_cost(&content), limits) {
        return finish(conversation, End::TranscriptFull, out);
    }
    let text = text_of(&content);
    conversation.transcript.push(Message { role: Role::Assistant, content }).expect("checked for room above");
    out.push(Request::Yielded { opener: conversation.opener, stop, text });
    State::Yielded
}

/// Yielded, continue: the opener's message goes to the LLM.
fn resumed(
    conversation: &mut Conversation,
    id: Id<Session>,
    text: Box<[u8]>,
    env: &Env<Limits>,
    out: &mut Queue<Request>,
) -> State {
    let content: Box<[Block]> = Box::new([Block::Text { text }]);
    if conversation.transcript.room() == 0 || !charge(conversation, content_cost(&content), &env.limits) {
        return finish(conversation, End::TranscriptFull, out);
    }
    conversation.transcript.push(Message { role: Role::User, content }).expect("checked for room above");
    call(conversation, id, 0, env, out)
}

/// Tooling, tool done: keep the result, then run the next tool or send the
/// results back.
fn tool_ran(
    conversation: &mut Conversation,
    id: Id<Session>,
    mut tools: Tools,
    result: Block,
    env: &Env<Limits>,
    out: &mut Queue<Request>,
) -> State {
    // The result's block was counted when the tools started.
    if !charge(conversation, payload_cost(&result), &env.limits) {
        return finish(conversation, End::TranscriptFull, out);
    }
    tools.results.push(result).expect("room for one result per tool call");
    let message = conversation.transcript.last().expect("the assistant message is last while tooling");
    match next_tool_call(&message.content, tools.block.saturating_add(1)) {
        Some(block) => {
            out.push(run_tool(conversation, id, block));
            State::Tooling { tools: Tools { block, results: tools.results } }
        }
        None => {
            let results = Message { role: Role::User, content: tools.results.into_boxed() };
            conversation.transcript.push(results).expect("room was checked when the tools started");
            call(conversation, id, 0, env, out)
        }
    }
}

/// Calling, failed: wait and call again if the failure is transient and
/// retries remain; end otherwise.
fn call_failed(
    conversation: &Conversation,
    attempt: u32,
    failure: Failure,
    rng: &mut Rng,
    env: &Env<Limits>,
    out: &mut Queue<Request>,
) -> State {
    match backoff(failure, attempt, &env.limits, rng) {
        Some(delay) => State::Backoff { attempt: attempt.saturating_add(1), until: env.now.saturating_add(delay) },
        None => finish(conversation, End::Failed { failure }, out),
    }
}

/// Calls the LLM with the conversation so far, if the budget pays for another
/// completion; ends the session otherwise.
fn call(
    conversation: &Conversation,
    id: Id<Session>,
    attempt: u32,
    env: &Env<Limits>,
    out: &mut Queue<Request>,
) -> State {
    if let Some(spent) = spent(conversation, env.now) {
        return finish(conversation, End::Budget { spent }, out);
    }
    out.push(complete(id, conversation, &env.limits));
    State::Calling { attempt }
}

fn cancel_call(id: Id<Session>, end: End, out: &mut Queue<Request>) -> State {
    out.push(Request::Cancel { owner: id.token() });
    State::Closing { end }
}

fn cancel_tool(id: Id<Session>, end: End, out: &mut Queue<Request>) -> State {
    out.push(Request::CancelTool { owner: id.token() });
    State::Closing { end }
}

/// Counts a completion that came back, and tells the opener.
fn used(conversation: &mut Conversation, usage: Usage, out: &mut Queue<Request>) {
    conversation.turns = conversation.turns.saturating_add(1);
    conversation.usage = conversation.usage.saturating_add(usage);
    out.push(Request::Used { opener: conversation.opener, usage });
}

/// Ends the session, telling its opener. Nothing may be in flight.
fn finish(conversation: &Conversation, end: End, out: &mut Queue<Request>) -> State {
    let opener = conversation.opener;
    out.push(Request::Ended { opener, end, turns: conversation.turns, usage: conversation.usage });
    State::Closed
}

// Helpers.

/// How a session ends when its time budget runs out.
const OUT_OF_TIME: End = End::Budget { spent: Dimension::Time };

/// Refuses an open at the entrance: the session ends without having opened.
fn refuse(facts: &mut Facts, opener: Token, end: End, out: &mut Queue<Request>) {
    facts.push(Fact::Ended { opener, end, turns: 0, usage: Usage::ZERO });
    out.push(Request::Ended { opener, end, turns: 0, usage: Usage::ZERO });
}

/// The conversation for `spec`, or `None` if the spec does not fit the limits.
fn admit(opener: Token, spec: Spec, limits: &Limits, now: Time) -> Option<Conversation> {
    if spec.max_tokens == 0 || spec.max_tokens > limits.max_tokens || !affordable(&spec.budget, &limits.budget) {
        return None;
    }
    let content: Box<[Block]> = Box::new([Block::Text { text: spec.prompt }]);
    let bytes = spec_cost(&spec.model, &spec.system, &spec.tools, &content)?;
    if bytes > limits.session_bytes {
        return None;
    }
    let mut transcript = List::with_capacity(limits.messages);
    transcript.push(Message { role: Role::User, content }).ok()?;
    Some(Conversation {
        opener,
        endpoint: spec.endpoint,
        model: spec.model,
        system: spec.system,
        tools: spec.tools,
        max_tokens: spec.max_tokens,
        transcript,
        bytes,
        budget: spec.budget,
        turns: 0,
        usage: Usage::ZERO,
        expires: now.saturating_add(spec.budget.time),
    })
}

/// Whether `budget` asks for no more than `most` in any dimension.
fn affordable(budget: &Budget, most: &Budget) -> bool {
    budget.turns <= most.turns
        && budget.input <= most.input
        && budget.output <= most.output
        && budget.cache_read <= most.cache_read
        && budget.cache_write <= most.cache_write
        && budget.time <= most.time
}

/// The first dimension that keeps the session from starting a completion at
/// `now`, if any: its turns, its input or output tokens used up, or its time.
fn spent(conversation: &Conversation, now: Time) -> Option<Dimension> {
    let Conversation { budget, turns, usage, expires, .. } = conversation;
    if *turns >= budget.turns {
        return Some(Dimension::Turns);
    }
    if usage.input_tokens >= budget.input {
        return Some(Dimension::Input);
    }
    if usage.output_tokens >= budget.output {
        return Some(Dimension::Output);
    }
    if now >= *expires {
        return Some(Dimension::Time);
    }
    None
}

/// The first dimension whose tokens a completion took past its budget, if any.
/// No turn starts past its budget, and an answer's `max_tokens` is cut to the
/// output left, but a call's input and cache tokens are known only once it
/// comes back.
fn overspent(conversation: &Conversation) -> Option<Dimension> {
    let Conversation { budget, usage, .. } = conversation;
    if usage.input_tokens > budget.input {
        return Some(Dimension::Input);
    }
    if usage.output_tokens > budget.output {
        return Some(Dimension::Output);
    }
    if usage.cache_read_tokens > budget.cache_read {
        return Some(Dimension::CacheRead);
    }
    if usage.cache_write_tokens > budget.cache_write {
        return Some(Dimension::CacheWrite);
    }
    None
}

/// The request for the next assistant message, its answer cut to the output
/// budget left. The conversation is copied: the session keeps it, and the
/// protocol layer holds the copy (copy at emission).
fn complete(id: Id<Session>, conversation: &Conversation, limits: &Limits) -> Request {
    let left = conversation.budget.output.saturating_sub(conversation.usage.output_tokens);
    let max_tokens = u32::try_from(left).unwrap_or(u32::MAX).min(conversation.max_tokens);
    let prompt = Prompt {
        endpoint: conversation.endpoint,
        model: conversation.model.clone(),
        system: conversation.system.clone(),
        tools: conversation.tools.clone(),
        messages: conversation.transcript.to_boxed(),
        max_tokens,
    };
    Request::Complete { owner: id.token(), prompt, timeout: limits.call_timeout }
}

/// The request to run the tool call at `block` of the last message.
fn run_tool(conversation: &Conversation, id: Id<Session>, block: u32) -> Request {
    match tool_call(conversation, block) {
        Block::ToolCall { id: _, name, input } => {
            Request::Tool { owner: id.token(), call: ToolCall { name: name.clone(), input: input.clone() } }
        }
        Block::Text { .. } | Block::ToolResult { .. } => unreachable!("blocks to run are found by next_tool_call"),
    }
}

/// The provider's name for the tool call at `block` of the last message, which
/// its result echoes.
fn call_id(conversation: &Conversation, block: u32) -> Box<[u8]> {
    match tool_call(conversation, block) {
        Block::ToolCall { id, name: _, input: _ } => id.clone(),
        Block::Text { .. } | Block::ToolResult { .. } => unreachable!("blocks to run are found by next_tool_call"),
    }
}

fn tool_call(conversation: &Conversation, block: u32) -> &Block {
    let message = conversation.transcript.last().expect("the assistant message is last while tooling");
    let index = usize::try_from(block).expect("a u32 fits in a usize");
    message.content.get(index).expect("blocks to run are found by next_tool_call")
}

/// The text blocks of `content`, one after another, in a box of their own: the
/// transcript keeps the message, and the opener gets the copy (copy at
/// emission).
fn text_of(content: &[Block]) -> Box<[u8]> {
    let mut len: usize = 0;
    for block in content {
        match block {
            Block::Text { text } => len = len.checked_add(text.len()).expect("bytes held fit in a usize"),
            Block::ToolCall { .. } | Block::ToolResult { .. } => {}
        }
    }
    let mut text = Writer::new(len);
    for block in content {
        match block {
            Block::Text { text: part } => text.put(part).expect("the length was counted above"),
            Block::ToolCall { .. } | Block::ToolResult { .. } => {}
        }
    }
    text.finish()
}

/// How many tool calls `content` holds, or `None` if they cannot be counted in
/// a `u32`.
fn tool_calls(content: &[Block]) -> Option<u32> {
    let mut calls: u32 = 0;
    for block in content {
        match block {
            Block::ToolCall { .. } => calls = calls.checked_add(1)?,
            Block::Text { .. } | Block::ToolResult { .. } => {}
        }
    }
    Some(calls)
}

/// The first tool call in `content` at or after block `from`, or `None` if
/// there is none or the blocks cannot be counted in a `u32`.
fn next_tool_call(content: &[Block], from: u32) -> Option<u32> {
    let end = u32::try_from(content.len()).ok()?;
    for index in from..end {
        match content.get(usize::try_from(index).ok()?)? {
            Block::ToolCall { .. } => return Some(index),
            Block::Text { .. } | Block::ToolResult { .. } => {}
        }
    }
    None
}

/// How long to wait before retrying a call that failed with `failure` after
/// `attempt` retries, or `None` if it is not to be retried.
fn backoff(failure: Failure, attempt: u32, limits: &Limits, rng: &mut Rng) -> Option<Duration> {
    let floor = match failure {
        Failure::Overloaded | Failure::Unavailable | Failure::TimedOut => Duration::ZERO,
        Failure::RateLimited { retry_after } => retry_after,
        Failure::ContextTooLong | Failure::Invalid | Failure::Unauthorized => return None,
    };
    if attempt >= limits.retries {
        return None;
    }
    // Exponential and capped, with equal jitter: half fixed, half random.
    let factor = 1_u64.checked_shl(attempt).unwrap_or(u64::MAX);
    let ceiling = limits.backoff_base.saturating_mul(factor).min(limits.backoff_max);
    let half = ceiling.as_nanos() / 2;
    let jittered = Duration::from_nanos(half.saturating_add(rng.below(half.saturating_add(1))));
    Some(jittered.max(floor))
}

/// Counts `cost` more bytes against the session's limit, or says they do not
/// fit.
#[must_use]
fn charge(conversation: &mut Conversation, cost: Option<u64>, limits: &Limits) -> bool {
    let Some(cost) = cost else {
        return false;
    };
    let Some(bytes) = conversation.bytes.checked_add(cost) else {
        return false;
    };
    if bytes > limits.session_bytes {
        return false;
    }
    conversation.bytes = bytes;
    true
}

/// What a spec costs: its names, its tools and its first message.
fn spec_cost(model: &[u8], system: &[u8], tools: &[Tool], content: &[Block]) -> Option<u64> {
    let mut cost = len(model)?.checked_add(len(system)?)?.checked_add(content_cost(content)?)?;
    let tool = u64::try_from(size_of::<Tool>()).ok()?;
    for Tool { name, description, schema } in tools {
        cost = cost
            .checked_add(tool)?
            .checked_add(len(name)?)?
            .checked_add(len(description)?)?
            .checked_add(len(schema)?)?;
    }
    Some(cost)
}

/// What an assistant message with `calls` tool calls costs while its tools run:
/// the message, and the room for their results.
fn held(content: &[Block], calls: u32) -> Option<u64> {
    let results = u64::try_from(size_of::<Block>()).ok()?.checked_mul(u64::from(calls))?;
    content_cost(content)?.checked_add(results)
}

fn content_cost(content: &[Block]) -> Option<u64> {
    let mut cost: u64 = 0;
    for block in content {
        cost = cost.checked_add(block_cost(block)?)?;
    }
    Some(cost)
}

/// A block's fixed size plus its payload.
fn block_cost(block: &Block) -> Option<u64> {
    u64::try_from(size_of::<Block>()).ok()?.checked_add(payload_cost(block)?)
}

fn payload_cost(block: &Block) -> Option<u64> {
    match block {
        Block::Text { text } => len(text),
        Block::ToolCall { id, name, input } => len(id)?.checked_add(len(name)?)?.checked_add(len(input)?),
        Block::ToolResult { id, output, error: _ } => len(id)?.checked_add(len(output)?),
    }
}

fn len(bytes: &[u8]) -> Option<u64> {
    u64::try_from(bytes.len()).ok()
}

/// A count for a fact, which saturates rather than fails: facts decide
/// nothing.
fn count(items: usize) -> u32 {
    u32::try_from(items).unwrap_or(u32::MAX)
}

/// A size for a fact, likewise.
fn size(bytes: &[u8]) -> u64 {
    u64::try_from(bytes.len()).unwrap_or(u64::MAX)
}
