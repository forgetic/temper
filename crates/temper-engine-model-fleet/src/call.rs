//! What the fleet relays between the parent and the runs (engine-model.md,
//! section 8; worker-model.md, 4.2): inbound events down, a run's host calls
//! up and their answers back, its bounces and its facts up. Each goes through
//! only while its attempt is the parent's live claim; what comes for an
//! attempt cancelled, replaced, lost or answered is dropped (attempts are
//! fenced), and an inbound event that reaches no worker goes back to the
//! parent, which keeps it.
//!
//! A relayed call is the parent's to answer, exactly once: the fleet keeps it
//! until the parent does, whatever becomes of its attempt meanwhile, and
//! passes the answer down only if the attempt is still the live claim, on a
//! worker in contact. A call beyond the room for calls is dropped, and its
//! run withdraws it past its own deadline.

use temper_lib::{Id, Queue, ReplyTo, Token};

use crate::attempt::{Attempt, State, Where};
use crate::boundary::{Bounce, Request, Undelivered};
use crate::channel;
use crate::facts::Fact;
use crate::model::Model;

/// A relayed call the parent has yet to answer.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) struct Call {
    /// The attempt that made it.
    attempt: Id<Attempt>,
    /// The worker's name for it.
    call: Token,
}

/// Where the parent's live claim is, as far as relaying goes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Live {
    /// On the worker of this channel, in contact.
    On(Token),
    /// Its worker's channel was lost.
    Adrift,
    /// The attempt is not the parent's live claim.
    Not,
}

/// Where the attempt `id` is, if it is the parent's live claim.
fn live(model: &Model, id: Id<Attempt>) -> Live {
    let Some(entry) = model.attempts.get(id) else {
        return Live::Not;
    };
    match &entry.state {
        State::Claimed { at: Where::On(channel), .. } => Live::On(channel::token(&model.channels, *channel)),
        State::Claimed { at: Where::Adrift { .. }, .. } => Live::Adrift,
        State::Waiting { .. }
        | State::Adopted { .. }
        | State::Cancelled { .. }
        | State::Handed { .. }
        | State::Acknowledged { .. }
        | State::Stray { .. }
        | State::Kept { .. }
        | State::Fenced { .. }
        | State::Closed => Live::Not,
    }
}

/// Whether the attempt `attempt` of `run` is the parent's live claim, and
/// which it is.
fn named(model: &Model, run: Token, attempt: Token) -> Option<Id<Attempt>> {
    let &id = model.names.get(&(run, attempt))?;
    match live(model, id) {
        Live::On(_) | Live::Adrift => Some(id),
        Live::Not => None,
    }
}

/// A run's host call, from its worker: passed to the parent as a call of the
/// fleet's, if its attempt is the live claim and there is room.
pub(crate) fn relay(model: &mut Model, run: Token, attempt: Token, call: Token, body: Token, out: &mut Queue<Request>) {
    let Some(id) = named(model, run, attempt) else {
        model.facts.push(Fact::Dropped);
        out.push(Request::Drop { payload: body });
        return;
    };
    match model.calls.insert(Call { attempt: id, call }) {
        Ok(call) => out.push(Request::Relay { reply_to: ReplyTo::new(call.token()), run, attempt, body }),
        Err(_) => {
            model.facts.push(Fact::Dropped);
            out.push(Request::Drop { payload: body });
        }
    }
}

/// The parent's answer to a relayed call: passed down to the run's worker,
/// if its attempt is still the live claim and on a worker in contact.
pub(crate) fn relayed(model: &mut Model, to: ReplyTo, answer: Token, out: &mut Queue<Request>) {
    let id = Id::<Call>::from_token(to.into_token());
    let Call { attempt: owner, call } =
        *model.calls.get(id).expect("the parent answers a call in flight, which is kept until it does");
    model.calls.retire(id);
    match live(model, owner) {
        Live::On(channel) => {
            let entry = model.attempts.get(owner).expect("a live claim is tracked");
            out.push(Request::Relayed { channel, run: entry.run, attempt: entry.token, call, answer });
        }
        Live::Adrift | Live::Not => {
            model.facts.push(Fact::Dropped);
            out.push(Request::Drop { payload: answer });
        }
    }
}

/// An inbound event from the parent: down to the attempt's worker, or back
/// to the parent if it reaches none.
pub(crate) fn inbound(model: &mut Model, run: Token, attempt: Token, event: Token, out: &mut Queue<Request>) {
    let undelivered = match model.names.get(&(run, attempt)) {
        Some(&id) => match &model.attempts.get(id).expect("a named attempt is tracked").state {
            State::Claimed { at: Where::On(channel), .. } => {
                let channel = channel::token(&model.channels, *channel);
                out.push(Request::Inbound { channel, run, attempt, event });
                return;
            }
            State::Waiting { .. } => Undelivered::Unplaced,
            State::Adopted { .. } | State::Claimed { at: Where::Adrift { .. }, .. } => Undelivered::Adrift,
            State::Cancelled { .. }
            | State::Handed { .. }
            | State::Acknowledged { .. }
            | State::Stray { .. }
            | State::Kept { .. }
            | State::Fenced { .. } => Undelivered::Gone,
            State::Closed => unreachable!("a closed attempt is no longer named"),
        },
        None => Undelivered::Gone,
    };
    out.push(Request::Undelivered { run, attempt, event, undelivered });
}

/// A bounce, from a worker: up to the parent, if its attempt is the live
/// claim.
pub(crate) fn bounced(model: &mut Model, run: Token, attempt: Token, bounce: Bounce, out: &mut Queue<Request>) {
    if named(model, run, attempt).is_some() {
        out.push(Request::Bounced { run, attempt, bounce });
    } else {
        model.facts.push(Fact::Dropped);
    }
}

/// A fact a run told, from its worker: up to the parent, if its attempt is
/// the live claim.
pub(crate) fn told(model: &mut Model, run: Token, attempt: Token, fact: Token, out: &mut Queue<Request>) {
    if named(model, run, attempt).is_some() {
        out.push(Request::Told { run, attempt, fact });
    } else {
        model.facts.push(Fact::Dropped);
        out.push(Request::Drop { payload: fact });
    }
}
