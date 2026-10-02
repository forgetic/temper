//! Routing (4.5): each of the protocol's events to the sub-model it is for,
//! each sub-model's requests to the protocol layer or, translated, to the
//! other, and the hand-offs the ready list deferred.
//!
//! Every match is exhaustive, so a variant added to either side's vocabulary
//! breaks the build here.

use temper_agent_model_run::{self as run, Spend};
use temper_agent_model_session as session;
use temper_lib::{Env, Id, Queue, Token};

use crate::boundary::{Event, Request};
use crate::limits::{self, Limits};
use crate::model::{Due, Flight, Handoff, Model};
use crate::peer::Peer;
use crate::translate;

/// What the run reads: this iteration's time, and its own limits.
pub(crate) const fn run_env(env: &Env<Limits>) -> Env<run::Limits> {
    Env { now: env.now, limits: env.limits.run }
}

/// What the session sub-model reads.
pub(crate) const fn session_env(env: &Env<Limits>) -> Env<session::Limits> {
    Env { now: env.now, limits: env.limits.session }
}

/// Hands one of the protocol's events to the sub-model it is for.
pub(crate) fn event(model: &mut Model, env: &Env<Limits>, event: Event) {
    let event = match event {
        Event::Start { reply_to, worker, charter } => run::Event::Start { reply_to, worker, charter },
        Event::Cancel { run } => run::Event::Cancel { run },
        Event::Pushed { owner, push } => run::Event::Pushed { owner, push },
        Event::HostCancelled { owner } => run::Event::HostCancelled { owner },
        Event::Read { owner, read } => run::Event::Read { owner, read },
        Event::Probed { owner, executable } => run::Event::Probed { owner, executable },
        Event::Checked { owner, ran } => run::Event::Checked { owner, ran },
        Event::Aborted { owner } => run::Event::Aborted { owner },
        Event::Completed { owner, completion } => {
            let id = *model.sessions.get(&owner).expect("a session lives until its call has ended");
            let peer = model.peers.get_mut(id).expect("a peer lives as its session");
            let before = peer.tickets();
            let completion = peer.completion(completion, env.limits.session.session_bytes);
            model.tickets = model.tickets.saturating_add(peer.tickets()).saturating_sub(before);
            return session_step(model, env, session::Event::Completed { owner, completion });
        }
        Event::Failed { owner, failure } => {
            return session_step(model, env, session::Event::Failed { owner, failure });
        }
        Event::Cancelled { owner } => return session_step(model, env, session::Event::Cancelled { owner }),
        Event::Done { owner, done } => return session_step(model, env, session::Event::Done { owner, done }),
    };
    run_step(model, env, event);
}

/// Delivers a hand-off from the run that waited on the ready list.
pub(crate) fn deliver(model: &mut Model, env: &Env<Limits>, handoff: Handoff) {
    let event = match handoff {
        Handoff::Close { peer } => {
            let peer = model.peers.get(peer).expect("a close is forgotten when its peer goes");
            let session = peer.session.expect("the run closes a conversation once it has started");
            session::Event::Close { session }
        }
        Handoff::Answer { owner } => {
            let flight = model.flights.remove(&owner).expect("a call is in flight until it is answered");
            match flight.answer {
                Due::Answered { answer } => session::Event::Answered { owner, answer },
                Due::Cancelled => session::Event::AnswerCancelled { owner },
                Due::Waiting => unreachable!("a call is on the ready list once the run has returned it"),
            }
        }
    };
    session_step(model, env, event);
}

/// Routes what the sub-models emitted, and what that leads to, until both have
/// emitted all they will in this entry point.
pub(crate) fn hand_off(model: &mut Model, env: &Env<Limits>, out: &mut Queue<Request>) {
    let bound = limits::run_out(&env.limits).saturating_add(limits::session_out(&env.limits));
    for _ in 0..bound {
        if let Some(request) = model.session_out.pop() {
            from_session(model, env, request, out);
        } else if let Some(request) = model.run_out.pop() {
            from_run(model, env, request, out);
        } else {
            return;
        }
    }
    assert!(
        model.session_out.is_empty() && model.run_out.is_empty(),
        "an entry point's hand-offs end within its bound"
    );
}

fn run_step(model: &mut Model, env: &Env<Limits>, event: run::Event) {
    assert!(model.run_out.room() >= run::MAX_OUT, "an entry point steps the run no more than its bound");
    run::step(&mut model.run, &run_env(env), event, &mut model.run_out);
}

fn session_step(model: &mut Model, env: &Env<Limits>, event: session::Event) {
    let room = session::max_out(&env.limits.session);
    assert!(model.session_out.room() >= room, "an entry point steps the sessions no more than its bound");
    session::step(&mut model.session, &session_env(env), event, &mut model.session_out);
}

/// One of the sessions' requests: out to the protocol layer, or to the run.
fn from_session(model: &mut Model, env: &Env<Limits>, request: session::Request, out: &mut Queue<Request>) {
    let event = match request {
        session::Request::Complete { owner, prompt, timeout } => {
            let id = *model.sessions.get(&owner).expect("a session asks for completions once it has opened");
            let peer = model.peers.get_mut(id).expect("a peer lives as its session");
            // What the last completion asked and the session did not dispatch,
            // it never will.
            let forgotten = peer.forget_asks(&env.limits.session);
            model.tickets = model.tickets.saturating_sub(forgotten);
            let prompt = peer.prompt(prompt);
            return out.push(Request::Complete { owner, prompt, timeout });
        }
        session::Request::Cancel { owner } => return out.push(Request::Cancel { owner }),
        session::Request::Io { owner, op, deadline } => return out.push(Request::Io { owner, op, deadline }),
        session::Request::CancelIo { owner } => return out.push(Request::CancelIo { owner }),
        session::Request::Opened { opener, session } => {
            let id = peer(model, opener);
            model.peers.get_mut(id).expect("found above").session = Some(session);
            let fresh = model.sessions.insert(session, id).expect("a peer for every session");
            assert!(fresh.is_none(), "a session opens once");
            run::Event::Started { conversation: opener, peer: session }
        }
        session::Request::Yielded { opener, stop, text } => {
            let id = peer(model, opener);
            let peer = model.peers.get_mut(id).expect("found above");
            let forgotten = peer.forget_asks(&env.limits.session);
            model.tickets = model.tickets.saturating_sub(forgotten);
            run::Event::Yielded { conversation: opener, stop: translate::stop(stop), text }
        }
        session::Request::Used { opener, usage } => {
            run::Event::Used { conversation: opener, spend: translate::spend(1, usage) }
        }
        session::Request::Ended { opener, end, turns, usage } => {
            let id = peer(model, opener);
            free(model, id);
            run::Event::Ended { conversation: opener, end: translate::end(end), spend: translate::spend(turns, usage) }
        }
        session::Request::Delegate { owner, opener, call, deadline: _ } => {
            // The deadline is the session's expiry, and the run's own deadline
            // is no later: the run races the call against it.
            let id = peer(model, opener);
            let ask = model.peers.get_mut(id).expect("found above").take(call);
            model.tickets = model.tickets.saturating_sub(1);
            let flight = Flight { peer: id, withdrawn: false, answer: Due::Waiting };
            let fresh = model.flights.insert(owner, flight).expect("room for a batch of each session");
            assert!(fresh.is_none(), "a session names its calls in flight apart");
            run::Event::Delegated { conversation: opener, call: owner, ask }
        }
        session::Request::Withdraw { owner } => {
            let flight = model.flights.get_mut(&owner).expect("a call is withdrawn while it is in flight");
            flight.withdrawn = true;
            match flight.answer {
                Due::Waiting => {}
                // The answer won the race: the run has returned it.
                Due::Answered { .. } | Due::Cancelled => return,
            }
            let conversation = model.peers.get(flight.peer).expect("a peer outlives its calls").conversation;
            run::Event::Withdraw { conversation, call: owner }
        }
    };
    run_step(model, env, event);
}

/// One of the run's requests: out to the protocol layer, or to the sessions.
fn from_run(model: &mut Model, env: &Env<Limits>, request: run::Request, out: &mut Queue<Request>) {
    let event = match request {
        run::Request::Admitted { worker, run } => return out.push(Request::Admitted { worker, run }),
        run::Request::Answer { to, answer } => return out.push(Request::Answer { to, answer }),
        run::Request::Checking { worker, deadline } => return out.push(Request::Checking { worker, deadline }),
        run::Request::Push { worker, owner, change } => return out.push(Request::Push { worker, owner, change }),
        run::Request::CancelHost { owner } => return out.push(Request::CancelHost { owner }),
        run::Request::Read { owner, at, max, deadline } => return out.push(Request::Read { owner, at, max, deadline }),
        run::Request::Probe { owner, at, deadline } => return out.push(Request::Probe { owner, at, deadline }),
        run::Request::Check { owner, program, deadline, tail } => {
            return out.push(Request::Check { owner, program, deadline, tail });
        }
        run::Request::Abort { owner } => return out.push(Request::Abort { owner }),
        run::Request::Open { conversation, opening } => {
            let Some((spec, offered)) = translate::spec(opening) else {
                // Refused at the conversations' entrance, in the run's terms.
                let ended = run::Event::Ended { conversation, end: run::End::Invalid, spend: Spend::ZERO };
                return run_step(model, env, ended);
            };
            let peer = Peer::new(conversation, offered, &env.limits.session);
            let id = model.peers.insert(peer).expect("a peer for every conversation the run has");
            let fresh = model.conversations.insert(conversation, id).expect("a peer for every conversation");
            assert!(fresh.is_none(), "the run names its conversations apart");
            session::Event::Open { opener: conversation, spec }
        }
        run::Request::Say { peer, text } => session::Event::Continue { session: peer, content: text },
        run::Request::Close { peer } => {
            // A close of a session that has ended is stale.
            if let Some(id) = model.sessions.get(&peer) {
                model.ready.defer(Handoff::Close { peer: *id });
            }
            return;
        }
        run::Request::Return { call, result } => {
            let flight = model.flights.get_mut(&call).expect("the run returns a call in flight");
            if flight.withdrawn && result == run::Returned::Cancelled {
                flight.answer = Due::Cancelled;
            } else {
                let peer = model.peers.get_mut(flight.peer).expect("a peer outlives its calls");
                flight.answer = Due::Answered { answer: peer.answer(result) };
                model.tickets = model.tickets.saturating_add(1);
            }
            return model.ready.defer(Handoff::Answer { owner: call });
        }
    };
    session_step(model, env, event);
}

/// The peer whose session's opener is `conversation`.
fn peer(model: &Model, conversation: Token) -> Id<Peer> {
    *model.conversations.get(&conversation).expect("a session's opener is a conversation the run opened")
}

/// Frees a peer whose session has ended, and its tickets: its calls have all
/// been answered.
fn free(model: &mut Model, id: Id<Peer>) {
    let peer = model.peers.get(id).expect("a peer lives as its session");
    model.tickets = model.tickets.checked_sub(peer.tickets()).expect("the peers' tickets are counted");
    let conversation = peer.conversation;
    if let Some(session) = peer.session {
        model.sessions.remove(&session);
    }
    model.conversations.remove(&conversation);
    model.ready.forget(Handoff::Close { peer: id });
    model.peers.retire(id);
}
