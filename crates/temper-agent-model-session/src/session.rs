//! LLM sessions: a conversation with an LLM, driven turn by turn until the LLM
//! yields, a limit or the budget ends it, or its opener closes it.
//!
//! An `Open` opens a session, and its kit in the tools the session owns
//! (4.5), with the spec's authority; then the session calls the LLM. While the
//! LLM asks for tools, the session runs them and sends their results back, in
//! call order, in another call. A call is to the session's own tools, which
//! answer it, at their entrance or once the operations they ask of io have
//! ended (the session passes those on as they are), or to one the opener
//! serves, delegated to it. Either kind runs in batches: adjacent calls that
//! read run together, up to `Limits::parallel_tools`, and a call that writes
//! runs alone. Each call carries its deadline, which whoever runs it races: a
//! call that runs out of time comes back as such, and goes to the LLM like any
//! other result. A call the protocol layer could not decode is answered with
//! its problem as it is reached, and nothing runs for it; a message with no
//! owned call goes straight back. When the LLM stops calling tools, the
//! session yields to its opener, which continues it with a new user message
//! or closes it; the message goes back after a result for each call the
//! yielded answer made but did not wait for.
//!
//! A session ends once, and only once nothing it asked for is in flight and
//! its kit has closed: closing cancels its call to the LLM, withdraws its
//! delegated calls and closes its kit, which cancels the tools' calls, and
//! waits for all of it to settle (5.3).
//!
//! The transition table. Every other cell is unreachable by the boundary's
//! contract: one terminal event per request, a request only from the states
//! that wait for its terminal event, and a `Continue` only to a yielded
//! session.
//!
//! ```text
//! state     event or alarm             next      requests
//! (none)    open, admitted, kit opened Calling   opened, complete
//!           open, busy or invalid, or  (none)    ended: busy, invalid
//!             its kit refused
//! Calling   completed, tool use        Tooling   used, the first batch's runs
//!           ... all answered at once   Resting   used (on the ready list)
//!           ... and no calls after it  Calling   used, complete
//!           ... time up                Closing   used (its kit closes)
//!           completed, only invalid    Calling   used, complete
//!           completed, no calls        Yielded   used, yielded (malformed)
//!           completed, otherwise       Yielded   used, yielded
//!           completed, does not fit    Closing   used (its kit closes)
//!           failed, transient          Backoff
//!           failed, otherwise          Closing   (its kit closes)
//!           close, expiry              Closing   cancel
//! Backoff   retry                      Calling   complete
//!           close, expiry              Closing   (its kit closes)
//! Tooling   a run done, batch running  Tooling
//!           a run done, more calls     Tooling   the next batch's runs
//!           ... all answered at once   Resting   (on the ready list)
//!           ... and no calls after it  Calling   complete
//!           ... time up                Closing   (its kit closes)
//!           a run done, no more        Calling   complete
//!           a result that does not fit Closing   a withdraw per delegated run
//!           close, expiry              Closing   a withdraw per delegated run
//! Resting   resume                     as from a run done, more calls
//!           close, expiry              Closing   (its kit closes)
//! Yielded   continue                   Calling   complete
//!           continue, does not fit     Closing   (its kit closes)
//!           close, expiry              Closing   (its kit closes)
//! Closing   its kit closed, nothing    Closed    ended
//!             left to wait for
//!           completed                  Closing   used
//!           failed, cancelled          Closing
//!           a run ends                 Closing
//!           close                      Closing   (already closing)
//! Closed    continue, close            Closed    (dropped: the handle is stale)
//! ```
//!
//! "Time up" is the expiry the session checks itself before it starts a
//! batch ([`advance`]), for a completion or a run's end that comes in the
//! iteration its time runs out, before the alarm fires. "Does not fit" is a
//! message the transcript has no room for (see below).
//!
//! A run is done when the tools answer it or the opener does (answered); it
//! ends while closing when it is done all the same, or when the kit's cancel
//! or its withdraw wins (the tools answer it cancelled, answer cancelled).
//! Entering Closing closes the kit ([`settle`]): the tools cancel what they
//! run for it, and say it has closed once each call is answered.
//!
//! Answers within the step (the rule for any parent whose child may answer in
//! the step that gave it the work, as the tools do here and the run and the
//! sessions will at the top level). The tools answer some calls at their
//! entrance, without asking io anything (a path outside the checkout, a family
//! not granted, a file not read first). Were a parent to go on from such an
//! answer at once, starting the next piece of work, hand-offs would chain
//! within one step: what it emits would follow the length of the chain rather
//! than [`crate::max_out`], and what it holds for work ended in the iteration
//! (reclaimed only at the reclaim point) would outgrow its slabs. So a step
//! starts one batch at most, and a batch the tools answer entirely within the
//! step that started it rests (Resting): its results are kept, and the
//! session goes on the model's ready list (programming-model.md, 2), held as
//! its state rather than as a queued record. The loop drains the ready list
//! with [`crate::resume`] at the start of the model's stage in a later
//! iteration, after the runs the batch ended have been reclaimed; a session
//! that rests while the list is drained waits for the next iteration, so the
//! chain never goes on before the reclaim point. A session therefore holds
//! the runs of two batches at most in an iteration (one ending and the next),
//! and each step, alarm and resume emits a bounded number of requests.
//!
//! Wherever the table calls the LLM (`complete`), the session first checks its
//! budget and its transcript, and ends instead: as out of budget, naming the
//! dimension, if its turns, input or output tokens are used up, if the last
//! completion took cache reads or writes past their budget, or if its time has
//! run out; and as transcript full if there is no room for the answer. So it
//! never starts a completion it may not pay for or could not keep, and the
//! turn that crossed a budget still runs its tools and keeps their results.
//! Only time does not wait: its expiry closes the session at once, and no
//! batch starts once it has passed. A completion, a result or a new message
//! that does not fit the byte limit ends the session as transcript full in
//! place of the transition it would have made, cancelling the rest of the
//! batch. A message is charged room for its calls' results with their ids
//! ([`held`]), and what a result holds besides as it arrives; so a refusal
//! at the tools' entrance, which holds nothing more, always fits, and a step
//! either starts a batch or cancels one, never both.
//!
//! The expiry alarm, set for when the time budget runs out, runs in every
//! state but Closing and Closed, and the retry alarm in Backoff; a session is
//! on the ready list while it is Resting. They follow from the state, in one
//! place ([`follow`]), which also retires a session once it is Closed.
//!
//! Every transition tells what happened as facts: the entry point tells of
//! the event it was given (a completion or a delegated call ending, a retry),
//! and [`tell`] of the requests the transition made, in one place; the tools
//! tell of their own calls, and the session passes their facts on.

use alloc::boxed::Box;
use core::mem::{self, size_of};

use temper_agent_model_tools::{self as tools, Call, Effect, Entry, Grants, Outcome, Part, Path};
use temper_lib::{Deadlines, Duration, Env, Id, List, Queue, ReplyTo, Rng, Set, Slab, Time, Token, Writer};

use crate::boundary::{Budget, Dimension, End, Request, Spec, Yield};
use crate::facts::{Fact, Facts};
use crate::limits::Limits;
use crate::llm::{
    Answer, Block, Completion, Decoded, Descriptor, Endpoint, Failure, Message, Problem, Prompt, Returned, Role, Stop,
    Usage,
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
    /// The families of its own tools it offers the LLM, as its kit has them.
    tools: Grants,
    delegated: Box<[Descriptor]>,
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
    /// Its kit in the tools it owns, as the tools name it. A session is
    /// inserted Closed, holding nothing, and given its kit as it opens.
    kit: Token,
}

#[derive(Debug)]
enum State {
    /// A call is in flight, after `attempt` retries.
    Calling { attempt: u32 },
    /// The last call failed transiently: calling again at `until`.
    Backoff { attempt: u32, until: Time },
    /// Running the tool calls of the last assistant message.
    Tooling { tools: Tools },
    /// The tools answered every call of the last batch in the step that
    /// started it: the session is on the ready list, and the next batch
    /// starts once the runs it ended have been reclaimed.
    Resting { tools: Tools },
    /// The LLM stopped calling tools: waiting for the opener to continue or
    /// close the session.
    Yielded,
    /// Ending with `end`, once what it waits for has settled, its kit's close
    /// among it.
    Closing { end: End, waiting: Waiting },
    /// Terminal: holds nothing.
    Closed,
}

/// What a closing session waits for: the terminal event of its call, and of
/// each of its tool runs still `runs`, and its kit's close.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
struct Waiting {
    call: bool,
    runs: u32,
    kit: Kit,
}

/// A closing session's kit: open until the session closes it as it settles
/// ([`settle`]), closing, its calls cancelled, or closed.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Kit {
    Open,
    Closing,
    Closed,
}

/// What a session waits for once nothing it asked for is in flight but its
/// kit, which it closes.
const KIT: Waiting = Waiting { call: false, runs: 0, kit: Kit::Open };

/// A closing session that waits for nothing: it has ended.
const SETTLED: Waiting = Waiting { call: false, runs: 0, kit: Kit::Closed };

/// The tool calls of the last assistant message, run in batches: adjacent
/// calls that read, up to `Limits::parallel_tools` of them, run together, and
/// one that writes runs alone.
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

/// A tool call running for a session, named by its own token: the call at
/// `block` of the session's last message, which fills `slot`, run `by` the
/// tools or the opener.
#[derive(Debug)]
pub(crate) struct Run {
    session: Id<Session>,
    slot: u32,
    block: u32,
    by: By,
}

/// Who runs a call: the tools, or the opener, which serves a delegated one.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum By {
    Tools,
    Opener,
}

// A call's slot, while its message's calls run, takes no more room than the
// block its result becomes, which is what each call is charged ([`held`]).
const _: () = assert!(size_of::<Slot>() <= size_of::<Block>(), "a slot takes no more room than a block");

/// What runs the sessions' tool calls: the runs, the tools sub-model, which
/// the session owns (4.5), and room for what the tools emit in a step.
#[derive(Debug)]
pub(crate) struct Calls {
    pub(crate) runs: Slab<Run>,
    pub(crate) tools: tools::Model,
    pub(crate) out: Queue<tools::Request>,
}

/// A session's timers, named by what they are for.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(crate) enum Alarm {
    Expiry { session: Id<Session> },
    Retry { session: Id<Session> },
}

/// The sessions that rest (programming-model.md, 2), each at most once: those
/// that rested before the last reclaim point, which [`crate::resume`] starts
/// again, and those that rested since, which wait for the next.
#[derive(Debug)]
pub(crate) struct Ready {
    now: Set<Id<Session>>,
    next: Set<Id<Session>>,
}

impl Ready {
    pub(crate) const fn with_capacity(sessions: u32) -> Ready {
        Ready { now: Set::with_capacity(sessions), next: Set::with_capacity(sessions) }
    }

    /// The most heap the list takes for `sessions` sessions, or `None` past a
    /// `u64`.
    pub(crate) fn worst_case(sessions: u32) -> Option<u64> {
        Set::<Id<Session>>::worst_case(sessions)?.checked_mul(2)
    }

    pub(crate) fn is_ready(&self) -> bool {
        !self.now.is_empty()
    }

    /// Takes a session that may be started again now.
    pub(crate) fn pop(&mut self) -> Option<Id<Session>> {
        let id = *self.now.first()?;
        self.now.remove(&id);
        Some(id)
    }

    /// Keeps the session `id` on the list if it is `resting`, and off it if
    /// not.
    fn keep(&mut self, id: Id<Session>, resting: bool) {
        if !resting {
            self.now.remove(&id);
            self.next.remove(&id);
        } else if !self.now.contains(&id) {
            self.next.insert(id).expect("room on the ready list for every session");
        }
    }

    /// The reclaim point: what rested in this iteration may be started again
    /// in the next.
    pub(crate) fn promote(&mut self) {
        for _ in 0..self.next.capacity() {
            let Some(id) = self.next.first().copied() else {
                break;
            };
            self.next.remove(&id);
            self.now.insert(id).expect("room on the ready list for every session");
        }
    }
}

// Entry points, one per event or alarm: look the session up, take its state
// out, run the cell's handler, then settle, tell what happened and follow the
// new state ([`conclude`]).

pub(crate) fn open(model: &mut Model, env: &Env<Limits>, opener: Token, spec: Spec, out: &mut Queue<Request>) {
    let mark = out.len();
    if model.sessions.is_full() {
        refuse(&mut model.facts, opener, End::Busy, out);
        return;
    }
    let Some((conversation, authority)) = admit(opener, spec, &env.limits, env.now) else {
        refuse(&mut model.facts, opener, End::Invalid, out);
        return;
    };
    // Closed until its kit opens and the first call is made, which a budget
    // just admitted pays for.
    let session = Session { conversation, state: State::Closed };
    let id = model.sessions.insert(session).expect("checked for room above");
    let heard = tools_step(&mut model.calls, env, tools::Event::Open { session: id.token(), authority }, out);
    let session = model.sessions.get_mut(id).expect("inserted above");
    match heard.kit {
        Some(News::Opened { kit }) => {
            session.conversation.kit = kit;
            out.push(Request::Opened { opener, session: id.token() });
            session.state = call(&session.conversation, id, 0, env, out);
        }
        // Refused at the tools' entrance: the session never opened.
        Some(News::Refused { refusal }) => {
            let end = match refusal {
                tools::Refusal::Busy => End::Busy,
                tools::Refusal::Invalid => End::Invalid,
            };
            out.push(Request::Ended { opener, end, turns: 0, usage: Usage::ZERO });
        }
        Some(News::Closed { .. }) | None => unreachable!("the tools answer an open with its kit, opened or refused"),
    }
    conclude(model, env, id, out, mark);
}

pub(crate) fn continued(
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
        State::Calling { .. }
        | State::Backoff { .. }
        | State::Tooling { .. }
        | State::Resting { .. }
        | State::Closing { .. } => unreachable!("an opener continues a session only while it is yielded"),
        State::Closed => unreachable!("an addressed session has not ended"),
    };
    conclude(model, env, id, out, mark);
}

pub(crate) fn close(model: &mut Model, env: &Env<Limits>, session: Token, out: &mut Queue<Request>) {
    let mark = out.len();
    let Some(id) = addressed(&model.sessions, session) else {
        return;
    };
    let session = model.sessions.get_mut(id).expect("addressed above");
    let state = mem::replace(&mut session.state, State::Closed);
    session.state = match state {
        State::Calling { attempt: _ } => cancel_call(id, End::Closed, out),
        State::Backoff { .. } | State::Resting { .. } | State::Yielded => finish(End::Closed),
        State::Tooling { tools } => cancel_tools(&model.calls.runs, tools, End::Closed, out),
        // It is already ending, with the end it had first.
        State::Closing { end, waiting } => State::Closing { end, waiting },
        State::Closed => unreachable!("an addressed session has not ended"),
    };
    conclude(model, env, id, out, mark);
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
        State::Calling { attempt: _ } => answered(conversation, id, &mut model.calls, completion, env, out),
        State::Closing { end, waiting: waiting @ Waiting { call: true, .. } } => {
            answered_late(conversation, end, waiting, completion.usage, out)
        }
        State::Backoff { .. }
        | State::Tooling { .. }
        | State::Resting { .. }
        | State::Yielded
        | State::Closing { waiting: Waiting { call: false, .. }, .. }
        | State::Closed => unreachable!("a completion ends a call in flight"),
    };
    conclude(model, env, id, out, mark);
}

pub(crate) fn failed(model: &mut Model, env: &Env<Limits>, owner: Token, failure: Failure, out: &mut Queue<Request>) {
    let mark = out.len();
    let id = Id::from_token(owner);
    let session = model.sessions.get_mut(id).expect("a session lives until its requests have ended");
    let opener = session.conversation.opener;
    model.facts.push(Fact::CompletionFailed { opener, failure });
    let state = mem::replace(&mut session.state, State::Closed);
    session.state = match state {
        State::Calling { attempt } => call_failed(attempt, failure, &mut model.rng, env),
        State::Closing { end, waiting: waiting @ Waiting { call: true, .. } } => {
            State::Closing { end, waiting: Waiting { call: false, ..waiting } }
        }
        State::Backoff { .. }
        | State::Tooling { .. }
        | State::Resting { .. }
        | State::Yielded
        | State::Closing { waiting: Waiting { call: false, .. }, .. }
        | State::Closed => unreachable!("a failure ends a call in flight"),
    };
    // A retry is told with its wait: no request goes out until then.
    match &session.state {
        State::Backoff { attempt, until } => {
            let delay = until.saturating_since(env.now);
            model.facts.push(Fact::CompletionRetried { opener, attempt: *attempt, delay });
        }
        State::Calling { .. }
        | State::Tooling { .. }
        | State::Resting { .. }
        | State::Yielded
        | State::Closing { .. }
        | State::Closed => {}
    }
    conclude(model, env, id, out, mark);
}

pub(crate) fn cancelled(model: &mut Model, env: &Env<Limits>, owner: Token, out: &mut Queue<Request>) {
    let mark = out.len();
    let id = Id::from_token(owner);
    let session = model.sessions.get_mut(id).expect("a session lives until its requests have ended");
    model.facts.push(Fact::CompletionCancelled { opener: session.conversation.opener });
    let state = mem::replace(&mut session.state, State::Closed);
    session.state = match state {
        State::Closing { end, waiting: waiting @ Waiting { call: true, .. } } => {
            State::Closing { end, waiting: Waiting { call: false, ..waiting } }
        }
        State::Calling { .. }
        | State::Backoff { .. }
        | State::Tooling { .. }
        | State::Resting { .. }
        | State::Yielded
        | State::Closing { waiting: Waiting { call: false, .. }, .. }
        | State::Closed => unreachable!("a cancellation answers the cancel of a call, sent on the way to Closing"),
    };
    conclude(model, env, id, out, mark);
}

/// An operation the tools asked of io has ended: the tools take it, and it may
/// answer one of their calls, or end a closing kit with its last.
pub(crate) fn io_done(model: &mut Model, env: &Env<Limits>, owner: Token, done: tools::Done, out: &mut Queue<Request>) {
    let mark = out.len();
    let heard = tools_step(&mut model.calls, env, tools::Event::Done { owner, done }, out);
    let mut concerned = None;
    if let Some((run, outcome)) = heard.answer {
        concerned = Some(owned_answered(model, env, run, outcome, out));
    }
    match heard.kit {
        Some(News::Closed { session }) => {
            kit_closed(&mut model.sessions, session);
            assert!(concerned.is_none() || concerned == Some(session), "a step of the tools concerns one kit");
            concerned = Some(session);
        }
        None => {}
        Some(News::Opened { .. } | News::Refused { .. }) => unreachable!("an operation's end opens no kit"),
    }
    if let Some(id) = concerned {
        conclude(model, env, id, out, mark);
    }
}

/// The tools answered the call of the run `run`: Tooling, the result goes in
/// its slot; Closing, one run fewer to wait for. Returns the run's session.
fn owned_answered(
    model: &mut Model,
    env: &Env<Limits>,
    run: Id<Run>,
    outcome: Outcome,
    out: &mut Queue<Request>,
) -> Id<Session> {
    let Run { session: id, slot, block, by } = ended_run(&mut model.calls.runs, run);
    assert!(by == By::Tools, "the tools answer only the calls they were given");
    let session = model.sessions.get_mut(id).expect("a session lives until its requests have ended");
    let state = mem::replace(&mut session.state, State::Closed);
    let conversation = &mut session.conversation;
    session.state = match state {
        State::Tooling { tools } => {
            let result = Returned::Owned { outcome };
            tool_ran(conversation, id, &mut model.calls, tools, slot, block, result, env, out)
        }
        State::Closing { end, waiting } => settled(end, waiting),
        State::Calling { .. } | State::Backoff { .. } | State::Resting { .. } | State::Yielded | State::Closed => {
            unreachable!("an answer ends a tool run in flight")
        }
    };
    id
}

/// The kit of `id`, closing, has closed.
fn kit_closed(sessions: &mut Slab<Session>, id: Id<Session>) {
    let session = sessions.get_mut(id).expect("a session lives until its kit has closed");
    let state = mem::replace(&mut session.state, State::Closed);
    session.state = match state {
        State::Closing { end, waiting: waiting @ Waiting { kit: Kit::Closing, .. } } => {
            State::Closing { end, waiting: Waiting { kit: Kit::Closed, ..waiting } }
        }
        State::Calling { .. }
        | State::Backoff { .. }
        | State::Tooling { .. }
        | State::Resting { .. }
        | State::Yielded
        | State::Closing { waiting: Waiting { kit: Kit::Open | Kit::Closed, .. }, .. }
        | State::Closed => unreachable!("a kit closes when its session closes it"),
    };
}

pub(crate) fn delegate_answered(
    model: &mut Model,
    env: &Env<Limits>,
    owner: Token,
    answer: Answer,
    out: &mut Queue<Request>,
) {
    let mark = out.len();
    let Run { session: id, slot, block, by } = ended_run(&mut model.calls.runs, Id::from_token(owner));
    assert!(by == By::Opener, "the opener answers only the calls delegated to it");
    let session = model.sessions.get_mut(id).expect("a session lives until its requests have ended");
    let Answer { ticket: _, bytes, error } = answer;
    model.facts.push(Fact::DelegateAnswered { opener: session.conversation.opener, bytes, error });
    let state = mem::replace(&mut session.state, State::Closed);
    let conversation = &mut session.conversation;
    session.state = match state {
        State::Tooling { tools } => {
            let result = Returned::Delegated { answer };
            tool_ran(conversation, id, &mut model.calls, tools, slot, block, result, env, out)
        }
        State::Closing { end, waiting } => settled(end, waiting),
        State::Calling { .. } | State::Backoff { .. } | State::Resting { .. } | State::Yielded | State::Closed => {
            unreachable!("an answer ends a delegated call in flight")
        }
    };
    conclude(model, env, id, out, mark);
}

pub(crate) fn delegate_cancelled(model: &mut Model, env: &Env<Limits>, owner: Token, out: &mut Queue<Request>) {
    let mark = out.len();
    let Run { session: id, slot: _, block: _, by } = ended_run(&mut model.calls.runs, Id::from_token(owner));
    assert!(by == By::Opener, "the opener answers only the calls delegated to it");
    let session = model.sessions.get_mut(id).expect("a session lives until its requests have ended");
    model.facts.push(Fact::DelegateCancelled { opener: session.conversation.opener });
    let state = mem::replace(&mut session.state, State::Closed);
    session.state = match state {
        State::Closing { end, waiting } => settled(end, waiting),
        State::Calling { .. }
        | State::Backoff { .. }
        | State::Tooling { .. }
        | State::Resting { .. }
        | State::Yielded
        | State::Closed => unreachable!("a cancelled answer answers a withdraw, sent on the way to Closing"),
    };
    conclude(model, env, id, out, mark);
}

pub(crate) fn expire(model: &mut Model, env: &Env<Limits>, id: Id<Session>, out: &mut Queue<Request>) {
    let mark = out.len();
    let session = model.sessions.get_mut(id).expect("an alarm is cancelled before its session closes");
    let state = mem::replace(&mut session.state, State::Closed);
    session.state = match state {
        State::Calling { attempt: _ } => cancel_call(id, OUT_OF_TIME, out),
        State::Backoff { .. } | State::Resting { .. } | State::Yielded => finish(OUT_OF_TIME),
        State::Tooling { tools } => cancel_tools(&model.calls.runs, tools, OUT_OF_TIME, out),
        State::Closing { .. } | State::Closed => {
            unreachable!("the expiry alarm runs only in Calling, Backoff, Tooling, Resting and Yielded")
        }
    };
    conclude(model, env, id, out, mark);
}

pub(crate) fn retry(model: &mut Model, env: &Env<Limits>, id: Id<Session>, out: &mut Queue<Request>) {
    let mark = out.len();
    let session = model.sessions.get_mut(id).expect("an alarm is cancelled before its session closes");
    let state = mem::replace(&mut session.state, State::Closed);
    session.state = match state {
        State::Backoff { attempt, until: _ } => call(&session.conversation, id, attempt, env, out),
        State::Calling { .. }
        | State::Tooling { .. }
        | State::Resting { .. }
        | State::Yielded
        | State::Closing { .. }
        | State::Closed => unreachable!("the retry alarm runs only in Backoff"),
    };
    conclude(model, env, id, out, mark);
}

/// Off the ready list: the session rested after a batch the tools answered at
/// once, and starts the next.
pub(crate) fn rested(model: &mut Model, env: &Env<Limits>, id: Id<Session>, out: &mut Queue<Request>) {
    let mark = out.len();
    let session = model.sessions.get_mut(id).expect("an alarm is cancelled before its session closes");
    let state = mem::replace(&mut session.state, State::Closed);
    let conversation = &mut session.conversation;
    session.state = match state {
        State::Resting { tools } => advance(conversation, id, &mut model.calls, tools, env, out),
        State::Calling { .. }
        | State::Backoff { .. }
        | State::Tooling { .. }
        | State::Yielded
        | State::Closing { .. }
        | State::Closed => unreachable!("a session is ready only while it rests"),
    };
    conclude(model, env, id, out, mark);
}

/// Passes the tools' facts on among the session's own, as the tools' parent,
/// each with the opener of the session whose kit it is of. Each pass drains
/// the tools' queue, which holds no more facts than it takes, so a fact goes
/// on at the end of the entry point that told it, while its session is there
/// still: a session is reclaimed at the reclaim point at the earliest.
pub(crate) fn pass_on_facts(model: &mut Model, env: &Env<Limits>) {
    for _ in 0..env.limits.tools.facts {
        let Some(fact) = model.calls.tools.pop_fact() else {
            break;
        };
        let session = model.sessions.get(Id::from_token(kit_session(fact)));
        let opener = session.expect("a kit's session lives until the reclaim point").conversation.opener;
        model.facts.push(Fact::Tools { opener, fact });
    }
}

/// The session, as the tools name it, whose kit `fact` is of.
const fn kit_session(fact: tools::Fact) -> Token {
    match fact {
        tools::Fact::Opened { session }
        | tools::Fact::Refused { session, .. }
        | tools::Fact::Started { session, .. }
        | tools::Fact::Answered { session, .. }
        | tools::Fact::Closing { session, .. }
        | tools::Fact::Closed { session } => session,
    }
}

/// The run `run` names, whose terminal event has come: retired, and copied
/// out.
fn ended_run(runs: &mut Slab<Run>, run: Id<Run>) -> Run {
    let found = runs.get(run).expect("a run lives until its terminal event");
    let ended = Run { session: found.session, slot: found.slot, block: found.block, by: found.by };
    runs.retire(run);
    ended
}

/// What a step of the tools gave back: the answer to a call, if one came, and
/// news of a kit.
struct Heard {
    answer: Option<(Id<Run>, Outcome)>,
    kit: Option<News>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum News {
    Opened { kit: Token },
    Refused { refusal: tools::Refusal },
    Closed { session: Id<Session> },
}

/// Steps the tools the session owns with `event`. The operations they ask of
/// io go out as the session's own requests, as they are, and so do their
/// cancels; what is for the session comes back. One step of the tools answers
/// at most one call: an open, a close or an operation's end concerns one kit,
/// and a call is answered at the entrance or later.
fn tools_step(calls: &mut Calls, env: &Env<Limits>, event: tools::Event, out: &mut Queue<Request>) -> Heard {
    let tools_env = Env { now: env.now, limits: env.limits.tools };
    tools::step(&mut calls.tools, &tools_env, event, &mut calls.out);
    let mut heard = Heard { answer: None, kit: None };
    for _ in 0..tools::max_out(&env.limits.tools) {
        let Some(request) = calls.out.pop() else {
            break;
        };
        match request {
            tools::Request::Io { owner, op, deadline } => out.push(Request::Io { owner, op, deadline }),
            tools::Request::CancelIo { owner } => out.push(Request::CancelIo { owner }),
            tools::Request::Answer { to, outcome } => {
                assert!(heard.answer.is_none(), "a step of the tools answers one call at most");
                heard.answer = Some((Id::from_token(to.into_token()), outcome));
            }
            tools::Request::Opened { session: _, kit } => heard.kit = Some(News::Opened { kit }),
            tools::Request::Refused { session: _, refusal } => heard.kit = Some(News::Refused { refusal }),
            tools::Request::Closed { session } => heard.kit = Some(News::Closed { session: Id::from_token(session) }),
        }
    }
    assert!(calls.out.is_empty(), "the tools emit no more than their max_out");
    heard
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
        | State::Resting { .. }
        | State::Yielded
        | State::Closing { .. } => Some(id),
        State::Closed => None,
    }
}

/// Applied after every transition: a closing session's kit closes, and the
/// session ends once nothing is left ([`settle`]); then what the transition
/// tells, and what the new state implies. `mark` is where the requests the
/// transition made begin in `out`.
fn conclude(model: &mut Model, env: &Env<Limits>, id: Id<Session>, out: &mut Queue<Request>, mark: u32) {
    settle(model, env, id, out);
    let session = model.sessions.get(id).expect("a session lives until it is retired");
    tell(&mut model.facts, &model.calls.runs, session, out, mark);
    follow(&mut model.sessions, &mut model.alarms, &mut model.ready, id);
}

/// What Closing implies: the session's kit closes, which cancels the calls the
/// tools are running for it; and once the kit has closed and nothing the
/// session waits for is left, the opener is told it has ended.
fn settle(model: &mut Model, env: &Env<Limits>, id: Id<Session>, out: &mut Queue<Request>) {
    let session = model.sessions.get_mut(id).expect("a session lives until it is retired");
    let (end, waiting) = match &session.state {
        State::Closing { end, waiting } => (*end, *waiting),
        State::Calling { .. }
        | State::Backoff { .. }
        | State::Tooling { .. }
        | State::Resting { .. }
        | State::Yielded
        | State::Closed => return,
    };
    let kit = match waiting.kit {
        Kit::Open => {
            let close = tools::Event::Close { kit: session.conversation.kit };
            let heard = tools_step(&mut model.calls, env, close, out);
            assert!(heard.answer.is_none(), "a kit's close answers its calls later, as their operations end");
            match heard.kit {
                Some(News::Closed { session: closed }) => {
                    assert!(closed == id, "a close is news of its own kit");
                    Kit::Closed
                }
                None => Kit::Closing,
                Some(News::Opened { .. } | News::Refused { .. }) => unreachable!("a close opens no kit"),
            }
        }
        Kit::Closing | Kit::Closed => waiting.kit,
    };
    let waiting = Waiting { kit, ..waiting };
    let session = model.sessions.get_mut(id).expect("looked up above");
    if waiting == SETTLED {
        let Conversation { opener, turns, usage, .. } = session.conversation;
        out.push(Request::Ended { opener, end, turns, usage });
        session.state = State::Closed;
    } else {
        session.state = State::Closing { end, waiting };
    }
}

/// What a transition tells, derived from the requests it made: a fact for
/// each but the cancels and the tools' operations, which the tools tell.
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
            Request::Delegate { owner, opener: _, call: _, deadline: _ } => {
                let run = runs.get(Id::from_token(*owner)).expect("a run lives while its call is asked for");
                Fact::DelegateStarted { opener, block: run.block }
            }
            Request::Cancel { .. } | Request::Withdraw { .. } | Request::Io { .. } | Request::CancelIo { .. } => {
                continue;
            }
        };
        facts.push(fact);
    }
}

/// The retries of the call a session has just asked for.
fn attempt(state: &State) -> u32 {
    match state {
        State::Calling { attempt } => *attempt,
        State::Backoff { .. }
        | State::Tooling { .. }
        | State::Resting { .. }
        | State::Yielded
        | State::Closing { .. }
        | State::Closed => unreachable!("a session that asks for a completion is calling"),
    }
}

/// What a session's state implies, applied after every transition: which
/// alarms run, whether it is on the ready list, and whether it is retired.
fn follow(sessions: &mut Slab<Session>, alarms: &mut Deadlines<Alarm>, ready: &mut Ready, id: Id<Session>) {
    let session = sessions.get(id).expect("a session lives until it is retired");
    let expires = session.conversation.expires;
    let (expiry, retry, resting, closed) = match &session.state {
        State::Calling { .. } | State::Tooling { .. } | State::Yielded => (Some(expires), None, false, false),
        State::Backoff { until, .. } => (Some(expires), Some(*until), false, false),
        State::Resting { .. } => (Some(expires), None, true, false),
        State::Closing { .. } => (None, None, false, false),
        State::Closed => (None, None, false, true),
    };
    set(alarms, Alarm::Expiry { session: id }, expiry);
    set(alarms, Alarm::Retry { session: id }, retry);
    ready.keep(id, resting);
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
    calls: &mut Calls,
    completion: Completion,
    env: &Env<Limits>,
    out: &mut Queue<Request>,
) -> State {
    used(conversation, completion.usage, out);
    match completion.stop {
        Stop::ToolUse => use_tools(conversation, id, calls, completion.content, env, out),
        Stop::EndTurn => pause(conversation, Yield::Done, completion.content, &env.limits, out),
        Stop::MaxTokens => pause(conversation, Yield::Truncated, completion.content, &env.limits, out),
        Stop::Refusal => pause(conversation, Yield::Refused, completion.content, &env.limits, out),
    }
}

/// Closing, completed: the call won the race with its cancel. The session
/// ends as it was going to, but the provider counted the tokens, and so does
/// the session.
fn answered_late(
    conversation: &mut Conversation,
    end: End,
    waiting: Waiting,
    usage: Usage,
    out: &mut Queue<Request>,
) -> State {
    used(conversation, usage, out);
    State::Closing { end, waiting: Waiting { call: false, ..waiting } }
}

/// Calling, completed with tool use: record the message and go through its
/// calls.
fn use_tools(
    conversation: &mut Conversation,
    id: Id<Session>,
    calls: &mut Calls,
    content: Box<[Block]>,
    env: &Env<Limits>,
    out: &mut Queue<Request>,
) -> State {
    let (count, _) = tally(&content);
    if count == 0 {
        return pause(conversation, Yield::Malformed, content, &env.limits, out);
    }
    // The results go back in another message.
    if conversation.transcript.room() < 2 || !charge(conversation, held(&content, count), &env.limits) {
        return finish(End::TranscriptFull);
    }
    conversation.transcript.push(Message { role: Role::Assistant, content }).expect("checked for room above");
    let tools = Tools { slots: List::with_capacity(count), running: 0, next: 0 };
    advance(conversation, id, calls, tools, env, out)
}

/// With no run in flight, goes on through the tool calls of the last message:
/// answers each invalid one with its problem, and starts the next batch, the
/// adjacent calls that read, up to `Limits::parallel_tools`, or one that
/// writes. Past the last call, sends the results back.
///
/// A step starts one batch at most. The tools may answer a call at their
/// entrance, in this very step, without asking io anything (a path outside the
/// checkout, a family not granted); if they answer every call of the batch so,
/// the session rests on the ready list before it starts another, so that
/// the runs it ended are reclaimed first, and what one step emits and the runs
/// a session holds stay bounded ([`crate::limits`]).
fn advance(
    conversation: &mut Conversation,
    id: Id<Session>,
    calls: &mut Calls,
    mut tools: Tools,
    env: &Env<Limits>,
    out: &mut Queue<Request>,
) -> State {
    // Time does not wait: no batch starts once it is up.
    if env.now >= conversation.expires {
        return finish(OUT_OF_TIME);
    }
    let message = conversation.transcript.last().expect("the assistant message is last while tooling");
    let end = u32::try_from(message.content.len()).expect("a message whose calls were counted has its blocks counted");
    let mut batch: Option<Effect> = None;
    let mut started: u32 = 0;
    let mut next = end;
    for index in tools.next..end {
        let message = conversation.transcript.last().expect("the assistant message is last while tooling");
        let block = message.content.get(usize::try_from(index).expect("a u32 fits in a usize"));
        match block.expect("within the message") {
            Block::ToolCall { id: _, name: _, input: _, call: Decoded::Owned { call } } => {
                let effect = tools::effect(call);
                if !joins(batch, effect, started, env.limits.parallel_tools) {
                    next = index;
                    break;
                }
                let call = call.clone();
                let run = Run { session: id, slot: tools.slots.len(), block: index, by: By::Tools };
                let run = calls.runs.insert(run).expect("the run slab has room for two batches a session");
                let deadline = env.now.saturating_add(env.limits.tool_timeout).min(conversation.expires);
                let (kit, reply_to) = (conversation.kit, ReplyTo::new(run.token()));
                let heard = tools_step(calls, env, tools::Event::Call { kit, reply_to, call, deadline }, out);
                assert!(heard.kit.is_none(), "a call is news of no kit");
                started = started.saturating_add(1);
                batch = Some(effect);
                // Answered at the tools' entrance, or running.
                let Some((answered, outcome)) = heard.answer else {
                    tools.slots.push(Slot::Running { run }).expect("a slot for every call");
                    tools.running = tools.running.saturating_add(1);
                    continue;
                };
                assert!(answered == run, "the tools answer at once only the call they were given");
                calls.runs.retire(run);
                // Refused at the entrance: its block and its id were counted
                // when the message was recorded, and a refusal holds nothing
                // more, so it fits, and the batch it is in goes on.
                let result = Returned::Owned { outcome };
                let fits = charge(conversation, returned_cost(&result), &env.limits);
                assert!(fits, "the tools refuse a call at their entrance with an outcome that holds nothing");
                let result = Block::ToolResult { id: call_id(conversation, index), result };
                tools.slots.push(Slot::Done { result }).expect("a slot for every call");
            }
            Block::ToolCall { id: _, name: _, input: _, call: Decoded::Delegated { ticket, effect } } => {
                if !joins(batch, *effect, started, env.limits.parallel_tools) {
                    next = index;
                    break;
                }
                let run = Run { session: id, slot: tools.slots.len(), block: index, by: By::Opener };
                let run = calls.runs.insert(run).expect("the run slab has room for two batches a session");
                // The opener runs the race, and the session waits for it as
                // long as it lives.
                let (opener, call, deadline) = (conversation.opener, *ticket, conversation.expires);
                out.push(Request::Delegate { owner: run.token(), opener, call, deadline });
                tools.slots.push(Slot::Running { run }).expect("a slot for every call");
                tools.running = tools.running.saturating_add(1);
                started = started.saturating_add(1);
                batch = Some(*effect);
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
    if started > 0 && next < end {
        return State::Resting { tools };
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

/// Whether a call with `effect` joins a batch of `batch` that has started
/// `started` runs: a read joins reads while there is room, and a write runs
/// alone.
const fn joins(batch: Option<Effect>, effect: Effect, started: u32, parallel: u32) -> bool {
    match batch {
        None => true,
        Some(Effect::Write) => false,
        Some(Effect::Read) => match effect {
            Effect::Read => started < parallel,
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
        return finish(End::TranscriptFull);
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
    // A budget the last answer took past its end comes before the room.
    if let Some(spent) = spent(conversation, env.now) {
        return finish(End::Budget { spent });
    }
    let content = unrun(conversation, text);
    if conversation.transcript.room() == 0 || !charge(conversation, content_cost(&content), &env.limits) {
        return finish(End::TranscriptFull);
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

/// Tooling, a run done: keep the result of the call at `block` in its slot,
/// and once the batch is done, go on through the calls after it.
#[expect(clippy::too_many_arguments, reason = "a cell handler takes what its cell needs, and the calls to start more")]
fn tool_ran(
    conversation: &mut Conversation,
    id: Id<Session>,
    calls: &mut Calls,
    mut tools: Tools,
    slot: u32,
    block: u32,
    result: Returned,
    env: &Env<Limits>,
    out: &mut Queue<Request>,
) -> State {
    tools.running = tools.running.checked_sub(1).expect("the run that ended was counted");
    // The result's block and its id were counted when the message was
    // recorded.
    let fits = charge(conversation, returned_cost(&result), &env.limits);
    let result = Block::ToolResult { id: call_id(conversation, block), result };
    *tools.slots.get_mut(slot).expect("a run fills its own slot") = Slot::Done { result };
    if !fits {
        return abandon(&calls.runs, tools, End::TranscriptFull, out);
    }
    if tools.running > 0 {
        return State::Tooling { tools };
    }
    advance(conversation, id, calls, tools, env, out)
}

/// Closing, a tool run ended: one fewer to wait for.
fn settled(end: End, waiting: Waiting) -> State {
    let runs = waiting.runs.checked_sub(1).expect("the run that ended was waited for");
    State::Closing { end, waiting: Waiting { runs, ..waiting } }
}

/// Calling, failed: wait and call again if the failure is transient and
/// retries remain; end otherwise.
fn call_failed(attempt: u32, failure: Failure, rng: &mut Rng, env: &Env<Limits>) -> State {
    match backoff(failure, attempt, &env.limits, rng) {
        Some(delay) => State::Backoff { attempt: attempt.saturating_add(1), until: env.now.saturating_add(delay) },
        None => finish(End::Failed { failure }),
    }
}

/// Calls the LLM with the conversation so far, if the budget pays for another
/// completion and the transcript has room for its answer; ends the session
/// otherwise, rather than pay for an answer it could not keep.
fn call(
    conversation: &Conversation,
    id: Id<Session>,
    attempt: u32,
    env: &Env<Limits>,
    out: &mut Queue<Request>,
) -> State {
    if let Some(spent) = spent(conversation, env.now) {
        return finish(End::Budget { spent });
    }
    if conversation.transcript.room() == 0 {
        return finish(End::TranscriptFull);
    }
    out.push(complete(id, conversation, &env.limits));
    State::Calling { attempt }
}

fn cancel_call(id: Id<Session>, end: End, out: &mut Queue<Request>) -> State {
    out.push(Request::Cancel { owner: id.token() });
    State::Closing { end, waiting: Waiting { call: true, ..KIT } }
}

/// Withdraws the delegated runs in flight, to end with `end` once every run
/// has settled. The tools' runs end as the kit's close cancels them.
fn cancel_tools(runs: &Slab<Run>, tools: Tools, end: End, out: &mut Queue<Request>) -> State {
    for slot in &tools.slots {
        match slot {
            Slot::Running { run } => match runs.get(*run).expect("a run in flight is in the slab").by {
                By::Tools => {}
                By::Opener => out.push(Request::Withdraw { owner: run.token() }),
            },
            Slot::Done { .. } => {}
        }
    }
    State::Closing { end, waiting: Waiting { runs: tools.running, ..KIT } }
}

/// Ends with `end` at once, or once the runs in flight have settled.
fn abandon(runs: &Slab<Run>, tools: Tools, end: End, out: &mut Queue<Request>) -> State {
    if tools.running == 0 {
        return finish(end);
    }
    cancel_tools(runs, tools, end, out)
}

/// Counts a completion that came back, and tells the opener.
fn used(conversation: &mut Conversation, usage: Usage, out: &mut Queue<Request>) {
    conversation.turns = conversation.turns.saturating_add(1);
    conversation.usage = conversation.usage.saturating_add(usage);
    out.push(Request::Used { opener: conversation.opener, usage });
}

/// Ends the session with `end`, which has nothing in flight but its kit: the
/// kit closes, and the opener is told once it has ([`settle`]).
const fn finish(end: End) -> State {
    State::Closing { end, waiting: KIT }
}

// Helpers.

/// How a session ends when its time budget runs out.
const OUT_OF_TIME: End = End::Budget { spent: Dimension::Time };

/// Refuses an open at the entrance: the session ends without having opened.
fn refuse(facts: &mut Facts, opener: Token, end: End, out: &mut Queue<Request>) {
    facts.push(Fact::Ended { opener, end, turns: 0, usage: Usage::ZERO });
    out.push(Request::Ended { opener, end, turns: 0, usage: Usage::ZERO });
}

/// The conversation for `spec`, and the authority its kit opens with; or
/// `None` if the spec does not fit the limits.
fn admit(opener: Token, spec: Spec, limits: &Limits, now: Time) -> Option<(Conversation, tools::Authority)> {
    if spec.max_tokens == 0 || spec.max_tokens > limits.max_tokens || !affordable(&spec.budget, &limits.budget) {
        return None;
    }
    let content: Box<[Block]> = Box::new([Block::Text { text: spec.prompt }]);
    let bytes = spec_cost(&spec.model, &spec.system, &spec.delegated, &content)?;
    if bytes > limits.session_bytes {
        return None;
    }
    let mut transcript = List::with_capacity(limits.messages);
    transcript.push(Message { role: Role::User, content }).ok()?;
    let conversation = Conversation {
        opener,
        endpoint: spec.endpoint,
        model: spec.model,
        system: spec.system,
        tools: spec.authority.grants,
        delegated: spec.delegated,
        max_tokens: spec.max_tokens,
        transcript,
        bytes,
        budget: spec.budget,
        turns: 0,
        usage: Usage::ZERO,
        expires: now.saturating_add(spec.budget.time),
        // Named as the kit opens: until then, the session is Closed, which
        // holds nothing.
        kit: Token::new(0),
    };
    Some((conversation, spec.authority))
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
        delegated: conversation.delegated.clone(),
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
            Block::ToolCall { call: Decoded::Owned { .. } | Decoded::Delegated { .. }, .. } => {
                calls = calls.saturating_add(1);
            }
            Block::ToolCall { call: Decoded::Invalid { .. }, .. } => {
                calls = calls.saturating_add(1);
                invalid = invalid.saturating_add(1);
            }
            Block::Text { .. } | Block::ToolResult { .. } => {}
        }
    }
    (calls, invalid)
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

/// What a spec costs: its names, the tools its opener serves, and its first
/// message.
fn spec_cost(model: &[u8], system: &[u8], delegated: &[Descriptor], content: &[Block]) -> Option<u64> {
    let descriptor = u64::try_from(size_of::<Descriptor>()).ok()?;
    let delegated = descriptor.checked_mul(u64::try_from(delegated.len()).ok()?)?;
    len(model)?.checked_add(len(system)?)?.checked_add(delegated)?.checked_add(content_cost(content)?)
}

/// What an assistant message with `calls` tool calls costs while its tools run:
/// the message, a block for each call's result, which is no less than the slot
/// the result waits in, with the id it echoes, and the answers to the invalid
/// calls, which are known already. What the other results hold is charged as
/// each arrives; so a refusal at the tools' entrance, which holds nothing,
/// always fits, and a step that starts a batch never cancels it.
fn held(content: &[Block], calls: u32) -> Option<u64> {
    let slots = u64::try_from(size_of::<Block>()).ok()?.checked_mul(u64::from(calls))?;
    let mut cost = content_cost(content)?.checked_add(slots)?;
    for block in content {
        match block {
            Block::ToolCall { id, name: _, input: _, call: Decoded::Invalid { problem } } => {
                cost = cost.checked_add(len(id)?)?.checked_add(problem_cost(problem)?)?;
            }
            Block::ToolCall { id, name: _, input: _, call: Decoded::Owned { .. } | Decoded::Delegated { .. } } => {
                cost = cost.checked_add(len(id)?)?;
            }
            Block::Text { .. } | Block::ToolResult { .. } => {}
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
            // A delegated call is the opener's to hold.
            let decoded = match call {
                Decoded::Owned { call } => call_cost(call)?,
                Decoded::Delegated { ticket: _, effect: _ } => 0,
                Decoded::Invalid { problem } => problem_cost(problem)?,
            };
            len(id)?.checked_add(len(name)?)?.checked_add(len(input)?)?.checked_add(decoded)
        }
        Block::ToolResult { id, result } => len(id)?.checked_add(returned_cost(result)?),
    }
}

/// The bytes a tool call's result holds beyond its block and its id.
fn returned_cost(result: &Returned) -> Option<u64> {
    match result {
        Returned::Owned { outcome } => outcome_cost(outcome),
        // The opener holds its answer, and the session counts it.
        Returned::Delegated { answer } => Some(answer.bytes),
        Returned::Invalid { problem } => problem_cost(problem),
        Returned::NotRun => Some(0),
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
        Outcome::Found { hits, more: _, timed_out: _ } => {
            let mut cost = u64::try_from(size_of::<tools::Hit>()).ok()?.checked_mul(u64::try_from(hits.len()).ok()?)?;
            for hit in hits {
                cost = cost.checked_add(len(&hit.path)?)?.checked_add(len(&hit.text)?)?;
            }
            Some(cost)
        }
        Outcome::Exited { exit: _, head, tail, dropped: _ } => len(head)?.checked_add(len(tail)?),
        Outcome::Ambiguous { count: _, lines } => {
            u64::try_from(size_of::<u32>()).ok()?.checked_mul(u64::try_from(lines.len()).ok()?)
        }
        Outcome::Written { .. }
        | Outcome::Edited { .. }
        | Outcome::NoMatch
        | Outcome::Unchanged
        | Outcome::NotGranted
        | Outcome::Outside
        | Outcome::ReadOnly
        | Outcome::TooLong
        | Outcome::NotFound
        | Outcome::NotFile
        | Outcome::Linked
        | Outcome::Protected
        | Outcome::NotDirectory
        | Outcome::TooLarge { .. }
        | Outcome::NotRead
        | Outcome::Stale
        | Outcome::Failed { .. }
        | Outcome::TimedOut
        | Outcome::Cancelled
        | Outcome::Busy
        | Outcome::NulByte => Some(0),
    }
}

fn problem_cost(problem: &Problem) -> Option<u64> {
    match problem {
        Problem::UnknownTool | Problem::NotAnObject | Problem::TooLarge => Some(0),
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
