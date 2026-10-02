//! LLM sessions: a conversation with an LLM, driven turn by turn until the LLM
//! finishes, a limit ends it, or it expires.
//!
//! A `Run` opens a session and the session calls the LLM. While the LLM asks
//! for tools, the session runs them one at a time and sends their results back
//! in another call. The session answers its run once, when it ends, and only
//! once nothing it asked for is in flight.
//!
//! The transition table. Every other cell is unreachable by the boundary's
//! contract: one terminal event per request, and a request only from the
//! states that wait for its terminal event.
//!
//! ```text
//! state          event or alarm               next
//! Calling        completed, end turn          Closed       reply: done
//!                completed, tool use          Tooling      run the first tool
//!                completed, max tokens        Closed       reply: truncated
//!                completed, refusal           Closed       reply: refused
//!                failed, transient            Backoff
//!                failed, otherwise            Closed       reply: failed
//!                expiry                       ClosingCall  cancel the call
//! Backoff        retry                        Calling      call again
//!                expiry                       Closed       reply: expired
//! Tooling        tool done, more tools        Tooling      run the next tool
//!                tool done, last tool         Calling      send the results
//!                expiry                       ClosingTool  cancel the tool
//! ClosingCall    completed, failed, cancelled Closed       reply
//! ClosingTool    tool done, tool cancelled    Closed       reply
//! ```
//!
//! The expiry alarm runs in Calling, Backoff and Tooling; the retry alarm in
//! Backoff. Both follow from the state, in one place ([`follow`]), which also
//! retires a session once it is Closed.

use alloc::boxed::Box;
use core::mem::{self, size_of};

use temper_lib::{Deadlines, Duration, Env, Id, List, Queue, ReplyTo, Rng, Slab, Time, Token};

use crate::boundary::{Outcome, Report, Request, Task, ToolCall};
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
    endpoint: Endpoint,
    model: Box<[u8]>,
    system: Box<[u8]>,
    tools: Box<[Tool]>,
    max_tokens: u32,
    /// The conversation so far, oldest first, starting with the task's prompt.
    transcript: List<Message>,
    /// Bytes held, counted against `Limits::session_bytes`.
    bytes: u64,
    /// Completions received.
    turns: u32,
    usage: Usage,
    /// When the session expires.
    expires: Time,
}

#[derive(Debug)]
enum State {
    /// A call is in flight, after `attempt` retries.
    Calling { reply_to: ReplyTo, attempt: u32 },
    /// The last call failed transiently: calling again at `until`.
    Backoff { reply_to: ReplyTo, attempt: u32, until: Time },
    /// Running the tool calls of the last assistant message.
    Tooling { reply_to: ReplyTo, tools: Tools },
    /// Ending with `outcome`, once the cancelled call's terminal event arrives.
    ClosingCall { reply_to: ReplyTo, outcome: Outcome },
    /// Ending with `outcome`, once the cancelled tool's terminal event arrives.
    ClosingTool { reply_to: ReplyTo, outcome: Outcome },
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
// out, run the cell's handler, follow the new state.

pub(crate) fn run(model: &mut Model, env: &Env<Limits>, reply_to: ReplyTo, task: Task, out: &mut Queue<Request>) {
    if model.sessions.is_full() {
        out.push(Request::Reply { to: reply_to, report: Report::Busy });
        return;
    }
    let Some(conversation) = admit(task, &env.limits, env.now) else {
        out.push(Request::Reply { to: reply_to, report: Report::Invalid });
        return;
    };
    let session = Session { conversation, state: State::Calling { reply_to, attempt: 0 } };
    let id = model.sessions.insert(session).expect("checked for room above");
    let session = model.sessions.get(id).expect("inserted above");
    out.push(complete(id, &session.conversation, &env.limits));
    follow(&mut model.sessions, &mut model.alarms, id);
}

pub(crate) fn completed(
    model: &mut Model,
    env: &Env<Limits>,
    owner: Token,
    completion: Completion,
    out: &mut Queue<Request>,
) {
    let id = Id::from_token(owner);
    let session = model.sessions.get_mut(id).expect("a session lives until its requests have ended");
    let state = mem::replace(&mut session.state, State::Closed);
    let conversation = &mut session.conversation;
    session.state = match state {
        State::Calling { reply_to, attempt: _ } => answered(conversation, id, reply_to, completion, &env.limits, out),
        State::ClosingCall { reply_to, outcome } => finish(conversation, reply_to, outcome, out),
        State::Backoff { .. } | State::Tooling { .. } | State::ClosingTool { .. } | State::Closed => {
            unreachable!("a completion ends a call in flight")
        }
    };
    follow(&mut model.sessions, &mut model.alarms, id);
}

pub(crate) fn failed(model: &mut Model, env: &Env<Limits>, owner: Token, failure: Failure, out: &mut Queue<Request>) {
    let Model { sessions, alarms, rng } = model;
    let id = Id::from_token(owner);
    let session = sessions.get_mut(id).expect("a session lives until its requests have ended");
    let state = mem::replace(&mut session.state, State::Closed);
    let conversation = &mut session.conversation;
    session.state = match state {
        State::Calling { reply_to, attempt } => call_failed(conversation, reply_to, attempt, failure, rng, env, out),
        State::ClosingCall { reply_to, outcome } => finish(conversation, reply_to, outcome, out),
        State::Backoff { .. } | State::Tooling { .. } | State::ClosingTool { .. } | State::Closed => {
            unreachable!("a failure ends a call in flight")
        }
    };
    follow(sessions, alarms, id);
}

pub(crate) fn cancelled(model: &mut Model, owner: Token, out: &mut Queue<Request>) {
    let id = Id::from_token(owner);
    let session = model.sessions.get_mut(id).expect("a session lives until its requests have ended");
    let state = mem::replace(&mut session.state, State::Closed);
    session.state = match state {
        State::ClosingCall { reply_to, outcome } => finish(&session.conversation, reply_to, outcome, out),
        State::Calling { .. }
        | State::Backoff { .. }
        | State::Tooling { .. }
        | State::ClosingTool { .. }
        | State::Closed => {
            unreachable!("a cancellation answers a cancel, sent only by ClosingCall")
        }
    };
    follow(&mut model.sessions, &mut model.alarms, id);
}

pub(crate) fn tool_done(
    model: &mut Model,
    env: &Env<Limits>,
    owner: Token,
    output: Box<[u8]>,
    error: bool,
    out: &mut Queue<Request>,
) {
    let id = Id::from_token(owner);
    let session = model.sessions.get_mut(id).expect("a session lives until its requests have ended");
    let state = mem::replace(&mut session.state, State::Closed);
    let conversation = &mut session.conversation;
    session.state = match state {
        State::Tooling { reply_to, tools } => {
            let result = Block::ToolResult { id: call_id(conversation, tools.block), output, error };
            tool_ran(conversation, id, reply_to, tools, result, &env.limits, out)
        }
        State::ClosingTool { reply_to, outcome } => finish(conversation, reply_to, outcome, out),
        State::Calling { .. } | State::Backoff { .. } | State::ClosingCall { .. } | State::Closed => {
            unreachable!("a tool result ends a tool run in flight")
        }
    };
    follow(&mut model.sessions, &mut model.alarms, id);
}

pub(crate) fn tool_cancelled(model: &mut Model, owner: Token, out: &mut Queue<Request>) {
    let id = Id::from_token(owner);
    let session = model.sessions.get_mut(id).expect("a session lives until its requests have ended");
    let state = mem::replace(&mut session.state, State::Closed);
    session.state = match state {
        State::ClosingTool { reply_to, outcome } => finish(&session.conversation, reply_to, outcome, out),
        State::Calling { .. }
        | State::Backoff { .. }
        | State::Tooling { .. }
        | State::ClosingCall { .. }
        | State::Closed => {
            unreachable!("a cancellation answers a cancel, sent only by ClosingTool")
        }
    };
    follow(&mut model.sessions, &mut model.alarms, id);
}

pub(crate) fn expire(model: &mut Model, id: Id<Session>, out: &mut Queue<Request>) {
    let session = model.sessions.get_mut(id).expect("an alarm is cancelled before its session closes");
    let state = mem::replace(&mut session.state, State::Closed);
    session.state = match state {
        State::Calling { reply_to, attempt: _ } => cancel_call(id, reply_to, Outcome::Expired, out),
        State::Backoff { reply_to, attempt: _, until: _ } => {
            finish(&session.conversation, reply_to, Outcome::Expired, out)
        }
        State::Tooling { reply_to, tools: _ } => cancel_tool(id, reply_to, Outcome::Expired, out),
        State::ClosingCall { .. } | State::ClosingTool { .. } | State::Closed => {
            unreachable!("the expiry alarm runs only in Calling, Backoff and Tooling")
        }
    };
    follow(&mut model.sessions, &mut model.alarms, id);
}

pub(crate) fn retry(model: &mut Model, env: &Env<Limits>, id: Id<Session>, out: &mut Queue<Request>) {
    let session = model.sessions.get_mut(id).expect("an alarm is cancelled before its session closes");
    let state = mem::replace(&mut session.state, State::Closed);
    session.state = match state {
        State::Backoff { reply_to, attempt, until: _ } => {
            call(&session.conversation, id, reply_to, attempt, &env.limits, out)
        }
        State::Calling { .. }
        | State::Tooling { .. }
        | State::ClosingCall { .. }
        | State::ClosingTool { .. }
        | State::Closed => {
            unreachable!("the retry alarm runs only in Backoff")
        }
    };
    follow(&mut model.sessions, &mut model.alarms, id);
}

/// What a session's state implies, applied after every transition: which
/// alarms run, and whether the session is retired.
fn follow(sessions: &mut Slab<Session>, alarms: &mut Deadlines<Alarm>, id: Id<Session>) {
    let session = sessions.get(id).expect("a session lives until it is retired");
    let expires = session.conversation.expires;
    let (expiry, retry, closed) = match &session.state {
        State::Calling { .. } | State::Tooling { .. } => (Some(expires), None, false),
        State::Backoff { until, .. } => (Some(expires), Some(*until), false),
        State::ClosingCall { .. } | State::ClosingTool { .. } => (None, None, false),
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
    reply_to: ReplyTo,
    completion: Completion,
    limits: &Limits,
    out: &mut Queue<Request>,
) -> State {
    conversation.turns = conversation.turns.saturating_add(1);
    conversation.usage = conversation.usage.saturating_add(completion.usage);
    match completion.stop {
        Stop::EndTurn => finish(conversation, reply_to, Outcome::Done { content: completion.content }, out),
        Stop::ToolUse => use_tools(conversation, id, reply_to, completion.content, limits, out),
        Stop::MaxTokens => finish(conversation, reply_to, Outcome::Truncated, out),
        Stop::Refusal => finish(conversation, reply_to, Outcome::Refused, out),
    }
}

/// Calling, completed with tool use: record the message and run its first tool.
fn use_tools(
    conversation: &mut Conversation,
    id: Id<Session>,
    reply_to: ReplyTo,
    content: Box<[Block]>,
    limits: &Limits,
    out: &mut Queue<Request>,
) -> State {
    let (Some(first), Some(calls)) = (next_tool_call(&content, 0), tool_calls(&content)) else {
        return finish(conversation, reply_to, Outcome::Malformed, out);
    };
    // The results go back in another completion, and in another message.
    if conversation.turns >= limits.turns {
        return finish(conversation, reply_to, Outcome::TurnLimit, out);
    }
    if conversation.transcript.room() < 2 || !charge(conversation, held(&content, calls), limits) {
        return finish(conversation, reply_to, Outcome::TranscriptFull, out);
    }
    conversation.transcript.push(Message { role: Role::Assistant, content }).expect("checked for room above");
    out.push(run_tool(conversation, id, first));
    State::Tooling { reply_to, tools: Tools { block: first, results: List::with_capacity(calls) } }
}

/// Tooling, tool done: keep the result, then run the next tool or send the
/// results back.
fn tool_ran(
    conversation: &mut Conversation,
    id: Id<Session>,
    reply_to: ReplyTo,
    mut tools: Tools,
    result: Block,
    limits: &Limits,
    out: &mut Queue<Request>,
) -> State {
    // The result's block was counted when the tools started.
    if !charge(conversation, payload_cost(&result), limits) {
        return finish(conversation, reply_to, Outcome::TranscriptFull, out);
    }
    tools.results.push(result).expect("room for one result per tool call");
    let message = conversation.transcript.last().expect("the assistant message is last while tooling");
    match next_tool_call(&message.content, tools.block.saturating_add(1)) {
        Some(block) => {
            out.push(run_tool(conversation, id, block));
            State::Tooling { reply_to, tools: Tools { block, results: tools.results } }
        }
        None => {
            let results = Message { role: Role::User, content: tools.results.into_boxed() };
            conversation.transcript.push(results).expect("room was checked when the tools started");
            call(conversation, id, reply_to, 0, limits, out)
        }
    }
}

/// Calling, failed: wait and call again if the failure is transient and
/// retries remain; end otherwise.
fn call_failed(
    conversation: &Conversation,
    reply_to: ReplyTo,
    attempt: u32,
    failure: Failure,
    rng: &mut Rng,
    env: &Env<Limits>,
    out: &mut Queue<Request>,
) -> State {
    match backoff(failure, attempt, &env.limits, rng) {
        Some(delay) => {
            State::Backoff { reply_to, attempt: attempt.saturating_add(1), until: env.now.saturating_add(delay) }
        }
        None => finish(conversation, reply_to, Outcome::Failed { failure }, out),
    }
}

/// Calls the LLM with the conversation so far.
fn call(
    conversation: &Conversation,
    id: Id<Session>,
    reply_to: ReplyTo,
    attempt: u32,
    limits: &Limits,
    out: &mut Queue<Request>,
) -> State {
    out.push(complete(id, conversation, limits));
    State::Calling { reply_to, attempt }
}

fn cancel_call(id: Id<Session>, reply_to: ReplyTo, outcome: Outcome, out: &mut Queue<Request>) -> State {
    out.push(Request::Cancel { owner: id.token() });
    State::ClosingCall { reply_to, outcome }
}

fn cancel_tool(id: Id<Session>, reply_to: ReplyTo, outcome: Outcome, out: &mut Queue<Request>) -> State {
    out.push(Request::CancelTool { owner: id.token() });
    State::ClosingTool { reply_to, outcome }
}

/// Ends the session, answering its run. Nothing may be in flight.
fn finish(conversation: &Conversation, reply_to: ReplyTo, outcome: Outcome, out: &mut Queue<Request>) -> State {
    let report = Report::Ended { outcome, turns: conversation.turns, usage: conversation.usage };
    out.push(Request::Reply { to: reply_to, report });
    State::Closed
}

// Helpers.

/// The conversation for `task`, or `None` if the task does not fit the limits.
fn admit(task: Task, limits: &Limits, now: Time) -> Option<Conversation> {
    if task.max_tokens == 0 || task.max_tokens > limits.max_tokens {
        return None;
    }
    let content: Box<[Block]> = Box::new([Block::Text { text: task.prompt }]);
    let bytes = task_cost(&task.model, &task.system, &task.tools, &content)?;
    if bytes > limits.session_bytes {
        return None;
    }
    let mut transcript = List::with_capacity(limits.messages);
    transcript.push(Message { role: Role::User, content }).ok()?;
    Some(Conversation {
        endpoint: task.endpoint,
        model: task.model,
        system: task.system,
        tools: task.tools,
        max_tokens: task.max_tokens,
        transcript,
        bytes,
        turns: 0,
        usage: Usage::ZERO,
        expires: now.saturating_add(limits.session_timeout),
    })
}

/// The request for the next assistant message. The conversation is copied: the
/// session keeps it, and the protocol layer holds the copy (copy at emission).
fn complete(id: Id<Session>, conversation: &Conversation, limits: &Limits) -> Request {
    let prompt = Prompt {
        endpoint: conversation.endpoint,
        model: conversation.model.clone(),
        system: conversation.system.clone(),
        tools: conversation.tools.clone(),
        messages: conversation.transcript.to_boxed(),
        max_tokens: conversation.max_tokens,
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

/// What a task costs: its names, its tools and its first message.
fn task_cost(model: &[u8], system: &[u8], tools: &[Tool], content: &[Block]) -> Option<u64> {
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
