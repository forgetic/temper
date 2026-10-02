//! LLM sessions: a conversation with an LLM, driven turn by turn until the LLM
//! yields, a limit or the budget ends it, or its opener closes it.
//!
//! An `Open` opens a session and the session calls the LLM. While the LLM asks
//! for tools, the session runs them and sends their results back, in call
//! order, in another call. They run in batches: adjacent calls that read run
//! together, up to `Limits::parallel_tools`, and a call that writes runs
//! alone; each run has a token of its own. A call the protocol layer could
//! not decode is answered with its problem, and nothing runs for it. When the LLM stops calling
//! tools, the session yields to its opener, which continues it with a new user
//! message or closes it; the message goes back after a result for each call
//! the yielded answer made but did not wait for. A session ends once, and only
//! once nothing it asked for is in flight: closing cancels what is, and waits
//! for it to settle (5.3).
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
//! Calling        completed, tool use          Tooling      run the first batch
//!                completed, otherwise         Yielded      yielded
//!                failed, transient            Backoff
//!                failed, otherwise            Closed       ended: failed
//!                close, expiry                Closing      cancel the call, wait for it
//! Backoff        retry                        Calling      call again
//!                close, expiry                Closed       ended: closed, out of time
//! Tooling        tool done, batch running     Tooling      keep the result
//!                tool done, more calls        Tooling      run the next batch
//!                tool done, no more           Calling      send the results
//!                close, expiry                Closing      cancel the batch, wait for each run
//! Yielded        continue                     Calling      call with the message
//!                close, expiry                Closed       ended: closed, out of time
//! Closing        what it waits for ends       Closed       ended
//!                a run ends, others pending   Closing      wait for the rest
//!                close                        Closing      (already closing)
//! Closed         continue, close              Closed       (dropped: the handle is stale)
//! ```
//!
//! Invalid calls are answered as they are reached, in Calling or Tooling, and a
//! message with no owned call goes straight back.
//!
//! Every completion that comes back is reported to the opener as `Used`, in
//! Calling and in Closing alike. Before it calls the LLM, the session checks
//! its budget, and ends instead, naming the dimension, if its turns, input or
//! output tokens are used up, if the last completion took cache reads or
//! writes past their budget, or if its time has run out. So it never starts a
//! completion it may not pay for, and the turn that crossed a budget still
//! runs its tools and keeps their results. Only time does not wait: its
//! expiry closes the session at once. A byte limit that a completion, a
//! tool's result or a new message would cross ends the session in place of
//! the transition it would have made.
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

use temper_agent_model_tools::{self as tools, Call, Effect, Entry, Grants, Outcome, Part, Path};
use temper_lib::{Deadlines, Duration, Env, Id, List, Queue, Rng, Slab, Time, Token, Writer};

use crate::boundary::{Budget, Dimension, End, Request, Spec, Yield};
use crate::facts::{Fact, Facts};
use crate::limits::Limits;
use crate::llm::{
    Block, Completion, Decoded, Endpoint, Failure, Message, Problem, Prompt, Returned, Role, Stop, Usage,
};
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
    tools: Grants,
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
    Closing { end: End, waiting: Waiting },
    /// Terminal: holds nothing.
    Closed,
}

/// What a closing session waits for: the terminal event of its call, or of
/// each of its tool runs still `pending`.
#[derive(Debug)]
enum Waiting {
    Call,
    Tools { pending: u32 },
}

/// The tool calls of the last assistant message, run in batches: adjacent
/// owned calls that read, up to `Limits::parallel_tools` of them, run
/// together, and one that writes runs alone.
#[derive(Debug)]
struct Tools {
    /// A slot for each call reached, in call order, with room for them all.
    slots: List<Slot>,
    /// The runs of the batch still in flight.
    running: u32,
    /// The block where the calls not reached yet begin.
    next: u32,
}

#[derive(Debug)]
enum Slot {
    /// The call is running as `run`.
    Running { run: Id<Run> },
    /// The call has its result.
    Done { result: Block },
}

/// An owned tool call running for a session, named by its own token: the
/// call at `block` of the session's last message, which fills `slot`.
#[derive(Debug)]
pub(crate) struct Run {
    session: Id<Session>,
    slot: u32,
    block: u32,
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
        State::Tooling { tools } => cancel_tools(tools, End::Closed, out),
        // It is already ending, with the end it had first.
        State::Closing { end, waiting } => State::Closing { end, waiting },
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
    let (calls, invalid) = tally(&completion.content);
    model.facts.push(Fact::CompletionAnswered { opener, stop, blocks, calls, invalid });
    let state = mem::replace(&mut session.state, State::Closed);
    let conversation = &mut session.conversation;
    session.state = match state {
        State::Calling { attempt: _ } => answered(conversation, id, &mut model.runs, completion, env, out),
        State::Closing { end, waiting: Waiting::Call } => answered_late(conversation, end, completion.usage, out),
        State::Backoff { .. }
        | State::Tooling { .. }
        | State::Yielded
        | State::Closing { waiting: Waiting::Tools { .. }, .. }
        | State::Closed => unreachable!("a completion ends a call in flight"),
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
        State::Closing { end, waiting: Waiting::Call } => finish(conversation, end, out),
        State::Backoff { .. }
        | State::Tooling { .. }
        | State::Yielded
        | State::Closing { waiting: Waiting::Tools { .. }, .. }
        | State::Closed => unreachable!("a failure ends a call in flight"),
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
        State::Closing { end, waiting: Waiting::Call } => finish(&session.conversation, end, out),
        State::Calling { .. }
        | State::Backoff { .. }
        | State::Tooling { .. }
        | State::Yielded
        | State::Closing { waiting: Waiting::Tools { .. }, .. }
        | State::Closed => unreachable!("a cancellation answers the cancel of a call, sent on the way to Closing"),
    };
    conclude(model, id, out, mark);
}

pub(crate) fn tool_done(
    model: &mut Model,
    env: &Env<Limits>,
    owner: Token,
    outcome: Outcome,
    out: &mut Queue<Request>,
) {
    let mark = out.len();
    let Run { session: id, slot, block } = ended_run(&mut model.runs, owner);
    let session = model.sessions.get_mut(id).expect("a session lives until its requests have ended");
    let output = outcome_cost(&outcome).unwrap_or(u64::MAX);
    let fact = Fact::ToolFinished { opener: session.conversation.opener, output, failed: !succeeded(&outcome) };
    model.facts.push(fact);
    let state = mem::replace(&mut session.state, State::Closed);
    let conversation = &mut session.conversation;
    session.state = match state {
        State::Tooling { tools } => {
            let result = Block::ToolResult { id: call_id(conversation, block), result: Returned::Owned { outcome } };
            tool_ran(conversation, id, &mut model.runs, tools, slot, result, env, out)
        }
        State::Closing { end, waiting: Waiting::Tools { pending } } => settled(conversation, end, pending, out),
        State::Calling { .. }
        | State::Backoff { .. }
        | State::Yielded
        | State::Closing { waiting: Waiting::Call, .. }
        | State::Closed => unreachable!("a tool result ends a tool run in flight"),
    };
    conclude(model, id, out, mark);
}

pub(crate) fn tool_cancelled(model: &mut Model, owner: Token, out: &mut Queue<Request>) {
    let mark = out.len();
    let Run { session: id, slot: _, block: _ } = ended_run(&mut model.runs, owner);
    let session = model.sessions.get_mut(id).expect("a session lives until its requests have ended");
    model.facts.push(Fact::ToolCancelled { opener: session.conversation.opener });
    let state = mem::replace(&mut session.state, State::Closed);
    session.state = match state {
        State::Closing { end, waiting: Waiting::Tools { pending } } => {
            settled(&session.conversation, end, pending, out)
        }
        State::Calling { .. }
        | State::Backoff { .. }
        | State::Tooling { .. }
        | State::Yielded
        | State::Closing { waiting: Waiting::Call, .. }
        | State::Closed => unreachable!("a cancellation answers the cancel of a run, sent on the way to Closing"),
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
        State::Tooling { tools } => cancel_tools(tools, OUT_OF_TIME, out),
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

/// The run `owner` names, whose terminal event has come: retired, and copied
/// out.
fn ended_run(runs: &mut Slab<Run>, owner: Token) -> Run {
    let id = Id::from_token(owner);
    let run = runs.get(id).expect("a run lives until its terminal event");
    let ended = Run { session: run.session, slot: run.slot, block: run.block };
    runs.retire(id);
    ended
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
    tell(&mut model.facts, &model.runs, session, out, mark);
    follow(&mut model.sessions, &mut model.alarms, id);
}

/// What a transition tells, derived from the requests it made: a fact for
/// each but the cancels, whose outcome is told when it comes.
fn tell(facts: &mut Facts, runs: &Slab<Run>, session: &Session, out: &Queue<Request>, mark: u32) {
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
            Request::Tool { owner, call: _ } => {
                let run = runs.get(Id::from_token(*owner)).expect("a run lives while its call is asked for");
                Fact::ToolStarted { opener, block: run.block }
            }
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
    runs: &mut Slab<Run>,
    completion: Completion,
    env: &Env<Limits>,
    out: &mut Queue<Request>,
) -> State {
    used(conversation, completion.usage, out);
    match completion.stop {
        Stop::ToolUse => use_tools(conversation, id, runs, completion.content, env, out),
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

/// Calling, completed with tool use: record the message and go through its
/// calls.
fn use_tools(
    conversation: &mut Conversation,
    id: Id<Session>,
    runs: &mut Slab<Run>,
    content: Box<[Block]>,
    env: &Env<Limits>,
    out: &mut Queue<Request>,
) -> State {
    let (calls, _) = tally(&content);
    if calls == 0 {
        return pause(conversation, Yield::Malformed, content, &env.limits, out);
    }
    // The results go back in another message.
    if conversation.transcript.room() < 2 || !charge(conversation, held(&content, calls), &env.limits) {
        return finish(conversation, End::TranscriptFull, out);
    }
    conversation.transcript.push(Message { role: Role::Assistant, content }).expect("checked for room above");
    let tools = Tools { slots: List::with_capacity(calls), running: 0, next: 0 };
    advance(conversation, id, runs, tools, env, out)
}

/// With no run in flight, goes on through the tool calls of the last message:
/// answers each invalid one with its problem, and starts the next batch, the
/// adjacent owned calls that read, up to `Limits::parallel_tools`, or one
/// that writes. Past the last call, sends the results back.
fn advance(
    conversation: &mut Conversation,
    id: Id<Session>,
    runs: &mut Slab<Run>,
    mut tools: Tools,
    env: &Env<Limits>,
    out: &mut Queue<Request>,
) -> State {
    let message = conversation.transcript.last().expect("the assistant message is last while tooling");
    let end = u32::try_from(message.content.len()).expect("a message whose calls were counted has its blocks counted");
    let mut batch: Option<Effect> = None;
    let mut next = end;
    for index in tools.next..end {
        let message = conversation.transcript.last().expect("the assistant message is last while tooling");
        let block = message.content.get(usize::try_from(index).expect("a u32 fits in a usize"));
        match block.expect("within the message") {
            Block::ToolCall { id: _, name: _, input: _, call: Decoded::Owned { call } } => {
                let effect = tools::effect(call);
                if !joins(batch, effect, tools.running, env.limits.parallel_tools) {
                    next = index;
                    break;
                }
                let run = Run { session: id, slot: tools.slots.len(), block: index };
                let run = runs.insert(run).expect("the run slab has room for every session's batches");
                out.push(Request::Tool { owner: run.token(), call: call.clone() });
                tools.slots.push(Slot::Running { run }).expect("a slot for every call");
                tools.running = tools.running.saturating_add(1);
                batch = Some(effect);
            }
            Block::ToolCall { id: call, name: _, input: _, call: Decoded::Invalid { problem } } => {
                // Its answer was counted when the message was recorded.
                let answer = Returned::Invalid { problem: problem.clone() };
                let result = Block::ToolResult { id: call.clone(), result: answer };
                tools.slots.push(Slot::Done { result }).expect("a slot for every call");
            }
            Block::Text { .. } | Block::ToolResult { .. } => {}
        }
    }
    tools.next = next;
    if tools.running > 0 {
        return State::Tooling { tools };
    }
    let mut results = List::with_capacity(tools.slots.len());
    for slot in tools.slots.into_boxed() {
        match slot {
            Slot::Done { result } => results.push(result).expect("a result for every slot"),
            Slot::Running { .. } => unreachable!("no run is in flight"),
        }
    }
    let results = Message { role: Role::User, content: results.into_boxed() };
    conversation.transcript.push(results).expect("room was checked when the message was recorded");
    call(conversation, id, 0, env, out)
}

/// Whether an owned call with `effect` joins a batch of `batch` with `running`
/// runs: a read joins reads while there is room, and a write runs alone.
const fn joins(batch: Option<Effect>, effect: Effect, running: u32, parallel: u32) -> bool {
    match batch {
        None => true,
        Some(Effect::Write) => false,
        Some(Effect::Read) => match effect {
            Effect::Read => running < parallel,
            Effect::Write => false,
        },
    }
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

/// Yielded, continue: the opener's message goes to the LLM, after a result for
/// each tool call the yielded message made, as every call needs one.
fn resumed(
    conversation: &mut Conversation,
    id: Id<Session>,
    text: Box<[u8]>,
    env: &Env<Limits>,
    out: &mut Queue<Request>,
) -> State {
    let content = unrun(conversation, text);
    if conversation.transcript.room() == 0 || !charge(conversation, content_cost(&content), &env.limits) {
        return finish(conversation, End::TranscriptFull, out);
    }
    conversation.transcript.push(Message { role: Role::User, content }).expect("checked for room above");
    call(conversation, id, 0, env, out)
}

/// The opener's message `text`, after a `NotRun` result for each tool call of
/// the last message, the one the session yielded with.
fn unrun(conversation: &Conversation, text: Box<[u8]>) -> Box<[Block]> {
    let message = conversation.transcript.last().expect("a yielded session's transcript ends with its answer");
    let (calls, _) = tally(&message.content);
    let mut content = List::with_capacity(calls.saturating_add(1));
    for block in &message.content {
        match block {
            Block::ToolCall { id, .. } => {
                let result = Block::ToolResult { id: id.clone(), result: Returned::NotRun };
                content.push(result).expect("room for a result per call");
            }
            Block::Text { .. } | Block::ToolResult { .. } => {}
        }
    }
    content.push(Block::Text { text }).expect("room for the message after the results");
    content.into_boxed()
}

/// Tooling, tool done: keep the result in its slot, and once the batch is
/// done, go on through the calls after it.
#[expect(clippy::too_many_arguments, reason = "a cell handler takes what its cell needs, and the runs to start more")]
fn tool_ran(
    conversation: &mut Conversation,
    id: Id<Session>,
    runs: &mut Slab<Run>,
    mut tools: Tools,
    slot: u32,
    result: Block,
    env: &Env<Limits>,
    out: &mut Queue<Request>,
) -> State {
    tools.running = tools.running.checked_sub(1).expect("the run that ended was counted");
    // The result's block was counted when the message was recorded.
    let fits = charge(conversation, payload_cost(&result), &env.limits);
    *tools.slots.get_mut(slot).expect("a run fills its own slot") = Slot::Done { result };
    if !fits {
        return abandon(conversation, tools, End::TranscriptFull, out);
    }
    if tools.running > 0 {
        return State::Tooling { tools };
    }
    advance(conversation, id, runs, tools, env, out)
}

/// Closing, a tool run ended: one fewer to wait for.
fn settled(conversation: &Conversation, end: End, pending: u32, out: &mut Queue<Request>) -> State {
    match pending.checked_sub(1).expect("the run that ended was waited for") {
        0 => finish(conversation, end, out),
        pending => State::Closing { end, waiting: Waiting::Tools { pending } },
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
    State::Closing { end, waiting: Waiting::Call }
}

/// Cancels the runs in flight, to end with `end` once each has settled.
fn cancel_tools(tools: Tools, end: End, out: &mut Queue<Request>) -> State {
    for slot in &tools.slots {
        match slot {
            Slot::Running { run } => out.push(Request::CancelTool { owner: run.token() }),
            Slot::Done { .. } => {}
        }
    }
    State::Closing { end, waiting: Waiting::Tools { pending: tools.running } }
}

/// Ends with `end` at once, or once the runs in flight have settled.
fn abandon(conversation: &Conversation, tools: Tools, end: End, out: &mut Queue<Request>) -> State {
    if tools.running == 0 {
        return finish(conversation, end, out);
    }
    cancel_tools(tools, end, out)
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
    let bytes = spec_cost(&spec.model, &spec.system, &content)?;
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

/// The first dimension of the budget that keeps the session from starting
/// another completion at `now`, if any: its turns, input or output tokens used
/// up, its cache reads or writes taken past their budget by a completion (whose
/// tokens are known only once it comes back), or its time run out.
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
    if usage.cache_read_tokens > budget.cache_read {
        return Some(Dimension::CacheRead);
    }
    if usage.cache_write_tokens > budget.cache_write {
        return Some(Dimension::CacheWrite);
    }
    if now >= *expires {
        return Some(Dimension::Time);
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
        tools: conversation.tools,
        messages: conversation.transcript.to_boxed(),
        max_tokens,
    };
    Request::Complete { owner: id.token(), prompt, timeout: limits.call_timeout }
}

/// The provider's name for the tool call at `block` of the last message, which
/// its result echoes.
fn call_id(conversation: &Conversation, block: u32) -> Box<[u8]> {
    let message = conversation.transcript.last().expect("the assistant message is last while tooling");
    let index = usize::try_from(block).expect("a u32 fits in a usize");
    match message.content.get(index).expect("the call in flight is a block of the message") {
        Block::ToolCall { id, .. } => id.clone(),
        Block::Text { .. } | Block::ToolResult { .. } => unreachable!("the call in flight is a tool call"),
    }
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

/// How many tool calls `content` holds, and how many of them are invalid. A
/// message too long to count in a `u32` counts as having none, and yields.
fn tally(content: &[Block]) -> (u32, u32) {
    if u32::try_from(content.len()).is_err() {
        return (0, 0);
    }
    let (mut calls, mut invalid): (u32, u32) = (0, 0);
    for block in content {
        match block {
            Block::ToolCall { call: Decoded::Owned { .. }, .. } => calls = calls.saturating_add(1),
            Block::ToolCall { call: Decoded::Invalid { .. }, .. } => {
                calls = calls.saturating_add(1);
                invalid = invalid.saturating_add(1);
            }
            Block::Text { .. } | Block::ToolResult { .. } => {}
        }
    }
    (calls, invalid)
}

/// Whether `outcome` is a success.
const fn succeeded(outcome: &Outcome) -> bool {
    match outcome {
        Outcome::Read { .. } | Outcome::Listed { .. } | Outcome::Written { .. } => true,
        Outcome::NotGranted
        | Outcome::Outside
        | Outcome::ReadOnly
        | Outcome::TooLong
        | Outcome::NotFound
        | Outcome::NotFile
        | Outcome::NotDirectory
        | Outcome::TooLarge { .. }
        | Outcome::NotRead
        | Outcome::Stale
        | Outcome::Failed { .. }
        | Outcome::TimedOut
        | Outcome::Cancelled
        | Outcome::Busy
        | Outcome::Unsupported => false,
    }
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

/// What a spec costs: its names and its first message.
fn spec_cost(model: &[u8], system: &[u8], content: &[Block]) -> Option<u64> {
    len(model)?.checked_add(len(system)?)?.checked_add(content_cost(content)?)
}

/// What an assistant message with `calls` tool calls costs while its tools run:
/// the message, a slot for each call's result, and the answers to the invalid
/// calls, which are known already.
fn held(content: &[Block], calls: u32) -> Option<u64> {
    let slots = u64::try_from(size_of::<Slot>()).ok()?.checked_mul(u64::from(calls))?;
    let mut cost = content_cost(content)?.checked_add(slots)?;
    for block in content {
        match block {
            Block::ToolCall { id, name: _, input: _, call: Decoded::Invalid { problem } } => {
                cost = cost.checked_add(len(id)?)?.checked_add(problem_cost(problem)?)?;
            }
            Block::ToolCall { call: Decoded::Owned { .. }, .. } | Block::Text { .. } | Block::ToolResult { .. } => {}
        }
    }
    Some(cost)
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

/// The bytes a block holds beyond its fixed size: its own, and those of the
/// call or outcome it carries.
fn payload_cost(block: &Block) -> Option<u64> {
    match block {
        Block::Text { text } => len(text),
        Block::ToolCall { id, name, input, call } => {
            let decoded = match call {
                Decoded::Owned { call } => call_cost(call)?,
                Decoded::Invalid { problem } => problem_cost(problem)?,
            };
            len(id)?.checked_add(len(name)?)?.checked_add(len(input)?)?.checked_add(decoded)
        }
        Block::ToolResult { id, result } => {
            let returned = match result {
                Returned::Owned { outcome } => outcome_cost(outcome)?,
                Returned::Invalid { problem } => problem_cost(problem)?,
                Returned::NotRun => 0,
            };
            len(id)?.checked_add(returned)
        }
    }
}

fn call_cost(call: &Call) -> Option<u64> {
    match call {
        Call::Read { path, skip: _, lines: _ } | Call::List { path } => path_cost(path),
        Call::Search { path, pattern, glob } => {
            let glob = match glob {
                Some(glob) => len(glob)?,
                None => 0,
            };
            path_cost(path)?.checked_add(len(pattern)?)?.checked_add(glob)
        }
        Call::Write { path, content } => path_cost(path)?.checked_add(len(content)?),
        Call::Edit { path, old, new, all: _ } => path_cost(path)?.checked_add(len(old)?)?.checked_add(len(new)?),
        Call::Shell { command, timeout: _ } => len(command),
    }
}

/// A path's parts and the names in them.
fn path_cost(path: &Path) -> Option<u64> {
    let parts = u64::try_from(size_of::<Part>()).ok()?.checked_mul(u64::try_from(path.parts.len()).ok()?)?;
    let mut cost = parts;
    for part in &path.parts {
        match part {
            Part::Name { name } => cost = cost.checked_add(len(name.as_bytes())?)?,
            Part::Current | Part::Parent => {}
        }
    }
    Some(cost)
}

fn outcome_cost(outcome: &Outcome) -> Option<u64> {
    match outcome {
        Outcome::Read { content, .. } => len(content),
        Outcome::Listed { entries, more: _ } => {
            let mut cost = u64::try_from(size_of::<Entry>()).ok()?.checked_mul(u64::try_from(entries.len()).ok()?)?;
            for entry in entries {
                cost = cost.checked_add(len(entry.name.as_bytes())?)?;
            }
            Some(cost)
        }
        Outcome::Written { .. }
        | Outcome::NotGranted
        | Outcome::Outside
        | Outcome::ReadOnly
        | Outcome::TooLong
        | Outcome::NotFound
        | Outcome::NotFile
        | Outcome::NotDirectory
        | Outcome::TooLarge { .. }
        | Outcome::NotRead
        | Outcome::Stale
        | Outcome::Failed { .. }
        | Outcome::TimedOut
        | Outcome::Cancelled
        | Outcome::Busy
        | Outcome::Unsupported => Some(0),
    }
}

fn problem_cost(problem: &Problem) -> Option<u64> {
    match problem {
        Problem::UnknownTool | Problem::NotAnObject => Some(0),
        Problem::Missing { field } | Problem::WrongType { field } | Problem::BadValue { field } => len(field),
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
