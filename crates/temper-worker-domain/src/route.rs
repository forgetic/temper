//! Routing (4.5): each of the protocol's events to the child domain or the link
//! it is for, and each child domain's requests to the protocol layer or,
//! translated, to a sibling.
//!
//! Every match is exhaustive, so a variant added to either side's vocabulary
//! breaks the build here.

use crate::wire;
use jig_host as host;
use skein_lib::{Env, Queue, ReplyTo};
use smith_host_domain as agent;
use temper_worker_domain_checkout as checkout;

use crate::boundary::{Event, Request};
use crate::domain::{self, Domain};
use crate::facts::Fact;
use crate::limits::{self, Limits};
use crate::link::{Bounced, Relay, RelayBody};
use crate::translate;
use crate::workspace::{self, Write};

/// What the host reads: this iteration's times, and its own limits.
pub(crate) const fn host_env(env: &Env<Limits>) -> Env<host::Limits> {
    Env { now: env.now, wall: env.wall, limits: env.limits.host }
}

/// What the checkout reads.
pub(crate) const fn checkout_env(env: &Env<Limits>) -> Env<checkout::Limits> {
    Env { now: env.now, wall: env.wall, limits: env.limits.checkout }
}

/// What the agent child domain reads.
pub(crate) const fn agent_env(env: &Env<Limits>) -> Env<agent::Limits> {
    Env { now: env.now, wall: env.wall, limits: env.limits.agent }
}

/// Hands one of the protocol's events to the child domain, or the link, it is
/// for.
pub(crate) fn event(domain: &mut Domain, env: &Env<Limits>, event: Event, out: &mut Queue<Request>) {
    let event = match event {
        Event::Relayed { run, attempt, call, delivery, answer } => {
            domain.link.heard();
            if domain.host.is_relayed_for(run, attempt, delivery, &call) {
                host_step(domain, env, host::Event::Relayed { run, attempt, call: delivery, answer });
            }
            return;
        }
        Event::Assign { assignment } => {
            domain.link.heard();
            let run = assignment.assignment.run;
            let attempt = assignment.assignment.attempt;
            if domain.link.holds(run, attempt) || domain.host.is_hosting(run, attempt) {
                return;
            }
            if env.limits.host.turns == 0 {
                return domain.link.refuse_version(run, attempt, out);
            }
            let answers = domain.link.held();
            host_step(domain, env, host::Event::Unacknowledged { answers });
            let wire::Assignment { assignment, turns, answered } = assignment;
            for call in &answered {
                if translate::named(&call.name).is_none() {
                    return domain.link.refuse(run, attempt, wire::Refusal::Invalid(wire::Invalid::Transcript), out);
                }
            }
            if !crate::delivery_evidence::valid_answered(&answered, env.limits.host.delivery_evidence_bytes) {
                return domain.link.refuse(run, attempt, wire::Refusal::Invalid(wire::Invalid::DeliveryEvidence), out);
            }
            let assignment = match workspace::stage(domain, env, assignment, true) {
                Ok(assignment) => assignment,
                Err(refusal) => return domain.link.refuse(run, attempt, refusal, out),
            };
            let assignment = host::Assignment { assignment, turns, answered };
            return host_step(domain, env, host::Event::Assign { reply_to: ReplyTo::new(run), assignment });
        }
        Event::Inbound { run, attempt, name, sender, words } => {
            domain.link.heard();
            return host_step(domain, env, host::Event::Inbound { run, attempt, name, sender, words });
        }

        Event::ConnectedV2 => {
            domain.link.connected_v2();
            domain::keep(domain, Fact::Connected);
            return host_step(domain, env, host::Event::Report);
        }

        Event::AcknowledgeTurn { run, attempt, turn } => {
            domain.link.heard();
            host_step(domain, env, host::Event::AcknowledgeTurn { run, attempt, turn });
            domain.link.turn_acknowledged(run, attempt, turn, &domain.host);
            return;
        }
        Event::TurnBusy { run, attempt, turn } => {
            domain.link.heard();
            return domain.link.turn_busy(run, attempt, turn, env, &domain.host);
        }

        Event::Lost => {
            let open = domain.link.is_up();
            domain.link.lost(env);
            if open {
                domain::keep(domain, Fact::Lost);
            }
            return;
        }
        Event::Shutdown => {
            domain.link.shut();
            return host_step(domain, env, host::Event::CancelAll { reason: wire::Reason::Shutdown });
        }

        Event::Acknowledged { run, attempt } => {
            domain.link.heard();
            return domain.link.acknowledged(run, attempt, &domain.host);
        }

        Event::Grant { run, attempt, grant } => {
            domain.link.heard();
            return host_step(domain, env, host::Event::Grant { run, attempt, grant });
        }
        Event::Cancel { run, attempt } => {
            domain.link.heard();
            return host_step(domain, env, host::Event::Cancel { run, attempt });
        }

        Event::RelayCancelled { call } => {
            return host_step(domain, env, host::Event::RelayCancelled { call });
        }
        Event::Done { owner, done } => return checkout_step(domain, env, checkout::Event::Done { owner, done }),
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
    agent_step(domain, env, event);
}

/// Routes what the child domains emitted, and what that leads to, until all
/// three have emitted all they will in this entry point.
pub(crate) fn hand_off(domain: &mut Domain, env: &Env<Limits>, out: &mut Queue<Request>) {
    let bound = limits::routed(&env.limits);
    for _ in 0..bound {
        if let Some(request) = domain.agent_out.pop() {
            from_agent(domain, env, request, out);
        } else if let Some(request) = domain.checkout_out.pop() {
            from_checkout(domain, env, request, out);
        } else if let Some(request) = domain.host_out.pop() {
            from_host(domain, env, request, out);
        } else {
            return;
        }
    }
    assert!(
        domain.agent_out.is_empty() && domain.checkout_out.is_empty() && domain.host_out.is_empty(),
        "an entry point's hand-offs end within its bound"
    );
}

pub(crate) fn host_step(domain: &mut Domain, env: &Env<Limits>, event: host::Event) {
    let room = host::max_out(&env.limits.host);
    assert!(domain.host_out.room() >= room, "an entry point steps the host no more than its bound");
    host::step(&mut domain.host, &host_env(env), event, &mut domain.host_out);
}

pub(crate) fn checkout_step(domain: &mut Domain, env: &Env<Limits>, event: checkout::Event) {
    assert!(
        domain.checkout_out.room() >= checkout::MAX_OUT,
        "an entry point steps the checkout no more than its bound"
    );
    checkout::step(&mut domain.checkout, &checkout_env(env), event, &mut domain.checkout_out);
}

pub(crate) fn agent_step(domain: &mut Domain, env: &Env<Limits>, event: agent::Event) {
    assert!(
        domain.agent_out.room() >= agent::max_out(&env.limits.agent),
        "an entry point steps the agents no more than its bound"
    );
    agent::step(&mut domain.agent, &agent_env(env), event, &mut domain.agent_out);
}

/// One of the host's requests: to the engine, or to a capability.
fn from_host(domain: &mut Domain, env: &Env<Limits>, request: host::Request, out: &mut Queue<Request>) {
    let request = match request.to_agent() {
        Ok(request) => return from_host_agent(domain, env, request),
        Err(request) => request,
    };
    match request {
        host::Request::Relay { run, attempt, call, delivery, tool, writes, input, deadline } => {
            domain.link.relay(
                Relay {
                    run,
                    attempt,
                    call: delivery,
                    body: RelayBody::Typed { name: call, tool, writes, input, deadline },
                },
                &domain.host,
                out,
            );
        }
        host::Request::Answer { to, run, attempt, answer } => {
            assert!(to.into_token() == run, "an answer is its assignment's");
            let preparation = workspace::preparation(domain, run);
            let work = workspace::finish(domain, run);
            let answer = translate::answer_v2(answer, work, preparation);
            domain.link.answer_v2(run, attempt, answer, out);
        }

        host::Request::Turn { agent: _, run, attempt, turn } => {
            domain.link.turn(run, attempt, turn, out);
        }
        host::Request::Deliver { owner, workspace, title, body } => {
            workspace::write(domain, env, owner, workspace, Write::PushV2 { title, body });
        }

        host::Request::CancelRelay { call } => {
            if domain.link.cancel_relay(call) {
                host_step(domain, env, host::Event::RelayCancelled { call });
            } else {
                out.push(Request::CancelRelay { call });
            }
        }
        host::Request::Bounced { run, attempt, name, bounce } => {
            domain.link.bounce(Bounced { run, attempt, name, bounce }, out);
        }
        host::Request::Hosting { runs } => {
            domain.link.hello(&runs, &domain.host, &domain.checkout, &env.limits, out);
        }
        host::Request::Prepare { owner, workspace } => workspace::prepare(domain, env, owner, workspace),
        host::Request::Abort { owner } => workspace::abort(domain, env, owner),

        host::Request::Save { owner, workspace } => {
            workspace::save(domain, env, owner, workspace);
        }
        host::Request::Release { workspace } => workspace::release(domain, env, workspace),
        host::Request::Message { .. }
        | host::Request::Reply { .. }
        | host::Request::Start { .. }
        | host::Request::Grant { .. }
        | host::Request::AcknowledgeAgentTurn { .. }
        | host::Request::Stop { .. } => unreachable!("agent capability was taken above"),
    }
}

fn from_host_agent(domain: &mut Domain, env: &Env<Limits>, request: host::ToAgent) {
    crate::agents::to_agent(domain, env, request);
}

/// One of the checkout's requests: out to io, or to the host.
fn from_checkout(domain: &mut Domain, env: &Env<Limits>, request: checkout::Request, out: &mut Queue<Request>) {
    match request {
        checkout::Request::Held { client, hold } => workspace::held(domain, env, client, hold),
        checkout::Request::Prepared { client, prepared } => workspace::prepared(domain, env, client, prepared),
        checkout::Request::Pushed { client, outcome } => workspace::wrote(domain, env, client, outcome, true),
        checkout::Request::Saved { client, outcome } => workspace::wrote(domain, env, client, outcome, false),
        checkout::Request::Released { client } => workspace::released(domain, env, client),
        checkout::Request::Io { owner, op, deadline } => out.push(Request::Io { owner, op, deadline }),
        checkout::Request::Cancel { owner } => out.push(Request::CancelIo { owner }),
    }
}

/// One of the agent child domain's requests: out to io, to the host, or a fact
/// of a run for the engine.
fn from_agent(domain: &mut Domain, env: &Env<Limits>, request: agent::Request, out: &mut Queue<Request>) {
    crate::agents::from_agent(domain, env, request, out);
}

pub(crate) const fn channel_grant(grant: wire::Grant) -> agent::Grant {
    agent::Grant { account: grant.account, generation: grant.generation, valid: grant.valid }
}
