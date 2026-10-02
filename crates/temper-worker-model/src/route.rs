//! Routing (4.5): each of the protocol's events to the sub-model or the link
//! it is for, and each sub-model's requests to the protocol layer or,
//! translated, to a sibling.
//!
//! Every match is exhaustive, so a variant added to either side's vocabulary
//! breaks the build here.

use temper_lib::{Env, Queue, ReplyTo};
use temper_worker_model_agent as agent;
use temper_worker_model_checkout as checkout;
use temper_worker_model_host as host;

use crate::boundary::{Event, Request};
use crate::facts::Fact;
use crate::limits::{self, Limits};
use crate::link::Stalled;
use crate::model::{self, Model};
use crate::translate;
use crate::workspace::{self, Write};

/// What the host reads: this iteration's time, and its own limits.
pub(crate) const fn host_env(env: &Env<Limits>) -> Env<host::Limits> {
    Env { now: env.now, limits: env.limits.host }
}

/// What the checkout reads.
pub(crate) const fn checkout_env(env: &Env<Limits>) -> Env<checkout::Limits> {
    Env { now: env.now, limits: env.limits.checkout }
}

/// What the agent sub-model reads.
pub(crate) const fn agent_env(env: &Env<Limits>) -> Env<agent::Limits> {
    Env { now: env.now, limits: env.limits.agent }
}

/// Hands one of the protocol's events to the sub-model, or the link, it is
/// for.
pub(crate) fn event(model: &mut Model, env: &Env<Limits>, event: Event) {
    let event = match event {
        Event::Connected => {
            model.link.connected();
            model::keep(model, Fact::Connected);
            // The host reports what it hosts, and the hello follows.
            return host_step(model, env, host::Event::Report);
        }
        Event::Lost => {
            let open = model.link.is_up();
            model.link.lost(env);
            if open {
                model::keep(model, Fact::Lost);
            }
            return;
        }
        Event::Shutdown => {
            model.link.shut();
            return host_step(model, env, host::Event::CancelAll { reason: host::Reason::Shutdown });
        }
        Event::Assign { assignment } => {
            let reply_to = ReplyTo::new(assignment.run);
            return host_step(model, env, host::Event::Assign { reply_to, assignment });
        }
        Event::Inbound { run, attempt, event } => {
            return host_step(model, env, host::Event::Inbound { run, attempt, event });
        }
        Event::Cancel { run, attempt } => return host_step(model, env, host::Event::Cancel { run, attempt }),
        Event::Relayed { run, attempt, call, answer } => {
            return host_step(model, env, host::Event::Relayed { run, attempt, call, answer });
        }
        Event::Done { owner, done } => return checkout_step(model, env, checkout::Event::Done { owner, done }),
        Event::Spawned { owner, process } => agent::Event::Spawned { owner, process },
        Event::Unspawned { owner, detail } => agent::Event::Unspawned { owner, detail },
        Event::Sent { owner } => agent::Event::Sent { owner },
        Event::Unsent { owner } => agent::Event::Unsent { owner },
        Event::Received { owner, message } => agent::Event::Received { owner, message },
        Event::Malformed { owner } => agent::Event::Malformed { owner },
        Event::Hangup { owner } => agent::Event::Hangup { owner },
        Event::Signalled { owner } => agent::Event::Signalled { owner },
        Event::Exited { owner } => agent::Event::Exited { owner },
        Event::Reaped { owner, detail } => agent::Event::Reaped { owner, detail },
    };
    agent_step(model, env, event);
}

/// Routes what the sub-models emitted, and what that leads to, until all three
/// have emitted all they will in this entry point.
pub(crate) fn hand_off(model: &mut Model, env: &Env<Limits>, out: &mut Queue<Request>) {
    let bound = limits::routed(&env.limits);
    for _ in 0..bound {
        if let Some(request) = model.agent_out.pop() {
            from_agent(model, env, request, out);
        } else if let Some(request) = model.checkout_out.pop() {
            from_checkout(model, env, request, out);
        } else if let Some(request) = model.host_out.pop() {
            from_host(model, env, request, out);
        } else {
            return;
        }
    }
    assert!(
        model.agent_out.is_empty() && model.checkout_out.is_empty() && model.host_out.is_empty(),
        "an entry point's hand-offs end within its bound"
    );
}

pub(crate) fn host_step(model: &mut Model, env: &Env<Limits>, event: host::Event) {
    let room = host::max_out(&env.limits.host);
    assert!(model.host_out.room() >= room, "an entry point steps the host no more than its bound");
    host::step(&mut model.host, &host_env(env), event, &mut model.host_out);
}

pub(crate) fn checkout_step(model: &mut Model, env: &Env<Limits>, event: checkout::Event) {
    assert!(model.checkout_out.room() >= checkout::MAX_OUT, "an entry point steps the checkout no more than its bound");
    checkout::step(&mut model.checkout, &checkout_env(env), event, &mut model.checkout_out);
}

pub(crate) fn agent_step(model: &mut Model, env: &Env<Limits>, event: agent::Event) {
    assert!(model.agent_out.room() >= agent::MAX_OUT, "an entry point steps the agents no more than its bound");
    agent::step(&mut model.agent, &agent_env(env), event, &mut model.agent_out);
}

/// One of the host's requests: to the engine, or to a capability.
fn from_host(model: &mut Model, env: &Env<Limits>, request: host::Request, out: &mut Queue<Request>) {
    let event = match request {
        host::Request::Answer { to, run, attempt, answer } => {
            assert!(to.into_token() == run, "an answer is its assignment's");
            return model.link.answer(run, attempt, answer, out);
        }
        host::Request::Relay { run, attempt, call, body } => {
            return model.link.stall(Stalled::Relay { run, attempt, call, body }, out);
        }
        host::Request::Bounced { run, attempt, bounce } => {
            return model.link.stall(Stalled::Bounced { run, attempt, bounce }, out);
        }
        host::Request::Hosting { runs } => return model.link.hello(&runs, &model.checkout, &env.limits, out),
        host::Request::Prepare { owner, workspace } => return workspace::prepare(model, env, owner, workspace),
        host::Request::Abort { owner } => return workspace::abort(model, env, owner),
        host::Request::Start { owner, workspace, charter, snapshot } => {
            return workspace::start(model, env, owner, workspace, charter, snapshot);
        }
        host::Request::Push { owner, workspace, message } => {
            return workspace::write(model, env, owner, workspace, Write::Push { message });
        }
        host::Request::Save { owner, workspace, branch } => {
            return workspace::write(model, env, owner, workspace, Write::Save { branch });
        }
        host::Request::Release { workspace } => return workspace::release(model, env, workspace),
        host::Request::Deliver { agent, event } => agent::Event::Deliver { agent, event },
        host::Request::Reply { agent, call, reply } => {
            agent::Event::Answer { agent, call, reply: translate::reply(reply) }
        }
        host::Request::Stop { agent } => agent::Event::Stop { agent },
    };
    agent_step(model, env, event);
}

/// One of the checkout's requests: out to io, or to the host.
fn from_checkout(model: &mut Model, env: &Env<Limits>, request: checkout::Request, out: &mut Queue<Request>) {
    match request {
        checkout::Request::Held { client, hold } => workspace::held(model, env, client, hold),
        checkout::Request::Prepared { client, prepared } => workspace::prepared(model, env, client, prepared),
        checkout::Request::Pushed { client, outcome } => workspace::wrote(model, env, client, outcome, true),
        checkout::Request::Saved { client, outcome } => workspace::wrote(model, env, client, outcome, false),
        checkout::Request::Released { client } => workspace::released(model, env, client),
        checkout::Request::Io { owner, op, deadline } => out.push(Request::Io { owner, op, deadline }),
        checkout::Request::Cancel { owner } => out.push(Request::CancelIo { owner }),
    }
}

/// One of the agent sub-model's requests: out to io, to the host, or a fact
/// of a run for the engine.
fn from_agent(model: &mut Model, env: &Env<Limits>, request: agent::Request, out: &mut Queue<Request>) {
    let event = match request {
        agent::Request::Spawn { owner, workspace, deadline } => {
            return out.push(Request::Spawn { owner, workspace, deadline });
        }
        agent::Request::Send { owner, process, message } => return out.push(Request::Send { owner, process, message }),
        agent::Request::Read { owner, process } => return out.push(Request::Read { owner, process }),
        agent::Request::Signal { owner, process, signal } => {
            return out.push(Request::Signal { owner, process, signal });
        }
        agent::Request::Wait { owner, process } => return out.push(Request::Wait { owner, process }),
        agent::Request::Reap { owner, process } => return out.push(Request::Reap { owner, process }),
        agent::Request::Told { client, fact } => return model::tell(model, client, fact),
        agent::Request::Started { client, agent } => host::Event::Started { owner: client, agent },
        agent::Request::Called { client, call, ask } => {
            host::Event::Called { owner: client, call, ask: translate::ask(ask) }
        }
        agent::Request::Withdrawn { client, call } => host::Event::Withdrawn { owner: client, call },
        agent::Request::Waiting { client } => host::Event::Yielded { owner: client },
        agent::Request::Finished { client, finish } => {
            host::Event::Finished { owner: client, finish: translate::finish(finish) }
        }
        agent::Request::Faulted { client, fault } => {
            host::Event::Faulted { owner: client, fault: translate::fault(fault) }
        }
        agent::Request::Bounced { client, bounce } => {
            host::Event::Bounced { owner: client, bounce: translate::bounce(bounce) }
        }
        // However it went (refused at the entrance, which the limits rule
        // out, unspawned, or stopped), the agent has gone.
        agent::Request::Gone { client, end: _, detail } => host::Event::Gone { owner: client, detail },
    };
    host_step(model, env, event);
}
