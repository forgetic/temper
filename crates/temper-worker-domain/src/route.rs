//! Routing (4.5): each of the protocol's events to the child domain or the link
//! it is for, and each child domain's requests to the protocol layer or,
//! translated, to a sibling.
//!
//! Every match is exhaustive, so a variant added to either side's vocabulary
//! breaks the build here.

use alloc::boxed::Box;

use crate::wire;
use jig_worker_host as host;
use skein_lib::{Env, Queue, ReplyTo};
use temper_worker_domain_agent as agent;
use temper_worker_domain_checkout as checkout;

use crate::boundary::{Event, Request};
use crate::domain::{self, Domain};
use crate::facts::Fact;
use crate::limits::{self, Limits};
use crate::link::{Bounced, Relay};
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
#[expect(clippy::too_many_lines, reason = "one exhaustive boundary routing table")]
pub(crate) fn event(domain: &mut Domain, env: &Env<Limits>, event: Event, out: &mut Queue<Request>) {
    let event = match event {
        Event::RelayedV2 { run, attempt, call, delivery, answer } => {
            domain.link.heard();
            if domain.host.is_relayed_named_for(run, attempt, delivery, call) {
                host_step(domain, env, host::Event::Relayed { run, attempt, call: delivery, answer });
            }
            return;
        }
        Event::ConnectedV2 => {
            domain.link.connected_v2();
            domain::keep(domain, Fact::Connected);
            return host_step(domain, env, host::Event::Report);
        }
        Event::AssignV2 { assignment } => {
            domain.link.heard();
            let run = assignment.assignment.run;
            let attempt = assignment.assignment.attempt;
            if domain.link.holds(run, attempt) || domain.host.is_hosting(run, attempt) {
                return;
            }
            if !domain.link.is_v2() || env.limits.host.turns == 0 {
                return domain.link.refuse_version(run, attempt, out);
            }
            let answers = domain.link.held();
            host_step(domain, env, host::Event::Unacknowledged { answers });
            let wire::AssignmentV2 { assignment, transcript } = assignment;
            let assignment = match workspace::stage(domain, env, assignment, true) {
                Ok(assignment) => assignment,
                Err(refusal) => return domain.link.refuse(run, attempt, refusal, true, out),
            };
            let assignment = host::AssignmentV2 { assignment, transcript };
            return host_step(domain, env, host::Event::AssignV2 { reply_to: ReplyTo::new(run), assignment });
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
        Event::Connected => {
            domain.link.connected();
            domain::keep(domain, Fact::Connected);
            // The host reports what it hosts, and the hello follows.
            return host_step(domain, env, host::Event::Report);
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
        Event::Assign { assignment } => {
            domain.link.heard();
            // The attempt answered, assigned again, is dropped: its one answer
            // is on its way.
            if domain.link.holds(assignment.run, assignment.attempt)
                || domain.host.is_hosting(assignment.run, assignment.attempt)
            {
                return;
            }
            let answers = domain.link.held();
            host_step(domain, env, host::Event::Unacknowledged { answers });
            let reply_to = ReplyTo::new(assignment.run);
            let attempt = assignment.attempt;
            let assignment = match workspace::stage(domain, env, assignment, false) {
                Ok(assignment) => assignment,
                Err(refusal) => return domain.link.refuse(reply_to.into_token(), attempt, refusal, false, out),
            };
            return host_step(domain, env, host::Event::Assign { reply_to, assignment });
        }
        Event::Acknowledged { run, attempt } => {
            domain.link.heard();
            return domain.link.acknowledged(run, attempt, &domain.host);
        }
        Event::Inbound { run, attempt, name, event } => {
            domain.link.heard();
            return host_step(domain, env, host::Event::Inbound { run, attempt, name, event });
        }
        Event::Grant { run, attempt, grant } => {
            domain.link.heard();
            return host_step(domain, env, host::Event::Grant { run, attempt, grant });
        }
        Event::Cancel { run, attempt } => {
            domain.link.heard();
            return host_step(domain, env, host::Event::Cancel { run, attempt });
        }
        Event::Relayed { run, attempt, call, answer } => {
            domain.link.heard();
            if domain.host.is_relayed_for(run, attempt, call) {
                return host_step(domain, env, host::Event::Relayed { run, attempt, call, answer });
            }
            return;
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
    assert!(domain.agent_out.room() >= agent::MAX_OUT, "an entry point steps the agents no more than its bound");
    agent::step(&mut domain.agent, &agent_env(env), event, &mut domain.agent_out);
}

/// One of the host's requests: to the engine, or to a capability.
fn from_host(domain: &mut Domain, env: &Env<Limits>, request: host::Request, out: &mut Queue<Request>) {
    let request = match request.to_agent() {
        Ok(request) => return from_host_agent(domain, env, request),
        Err(request) => request,
    };
    match request {
        host::Request::AnswerV2 { to, run, attempt, answer } => {
            assert!(to.into_token() == run, "an answer is its assignment's");
            let preparation = workspace::preparation(domain, run);
            let work = workspace::finish(domain, run);
            let answer = translate::answer_v2(answer, work, preparation);
            domain.link.answer_v2(run, attempt, answer, out);
        }
        host::Request::RelayV2 { run, attempt, call, delivery, body } => {
            domain.link.relay(Relay { run, attempt, call: delivery, stable: Some(call), body }, &domain.host, out);
        }
        host::Request::Turn { agent: _, run, attempt, turn } => {
            domain.link.turn(run, attempt, turn, out);
        }
        host::Request::DeliverV2 { owner, workspace, title, body } => {
            workspace::write(domain, env, owner, workspace, Write::PushV2 { title, body });
        }
        host::Request::Answer { to, run, attempt, answer } => {
            assert!(to.into_token() == run, "an answer is its assignment's");
            let preparation = workspace::preparation(domain, run);
            let work = workspace::finish(domain, run);
            let answer = translate::answer(answer, work, preparation);
            domain.link.answer(run, attempt, answer, out);
        }
        host::Request::Relay { run, attempt, call, body } => {
            domain.link.relay(Relay { run, attempt, call, stable: None, body }, &domain.host, out);
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
        host::Request::DeliverWorkspace { owner, workspace, message } => {
            workspace::write(domain, env, owner, workspace, Write::Push { message });
        }
        host::Request::Save { owner, workspace } => {
            workspace::save(domain, env, owner, workspace);
        }
        host::Request::Release { workspace } => workspace::release(domain, env, workspace),
        host::Request::StartTyped { .. }
        | host::Request::StartV2 { .. }
        | host::Request::Start { .. }
        | host::Request::Deliver { .. }
        | host::Request::Reply { .. }
        | host::Request::Grant { .. }
        | host::Request::TurnCredit { .. }
        | host::Request::Stop { .. } => unreachable!("agent capability was taken above"),
    }
}

fn from_host_agent(domain: &mut Domain, env: &Env<Limits>, request: host::ToAgent) {
    let event = match request {
        host::ToAgent::StartTyped { .. } => unreachable!("the current agent child uses the earlier wire vocabulary"),
        host::ToAgent::StartV2 { owner, workspace, charter, transcript, grants } => {
            return match workspace {
                Some(workspace) => workspace::start_v2(domain, env, owner, workspace, charter, transcript, grants),
                None => agent_step(
                    domain,
                    env,
                    agent::Event::SpawnV2 {
                        client: owner,
                        spawn: agent::SpawnV2 {
                            workspace: None,
                            charter,
                            transcript,
                            repositories: Box::new([]),
                            grants: grants_to_agent(grants),
                        },
                    },
                ),
            };
        }
        host::ToAgent::Start { owner, workspace, charter, snapshot, grants } => {
            return match workspace {
                Some(workspace) => workspace::start(domain, env, owner, workspace, charter, snapshot, grants),
                None => agent_step(
                    domain,
                    env,
                    agent::Event::Spawn {
                        client: owner,
                        spawn: agent::Spawn {
                            workspace: None,
                            charter,
                            snapshot,
                            repositories: Box::new([]),
                            grants: grants_to_agent(grants),
                        },
                    },
                ),
            };
        }
        host::ToAgent::Message { agent, name, event } => agent::Event::Deliver { agent, name, event },
        host::ToAgent::Answer { agent, call, reply } => {
            agent::Event::Answer { agent, call, reply: translate::reply(domain, reply) }
        }
        host::ToAgent::Grant { agent, grant } => agent::Event::Grant { agent, grant: channel_grant(grant) },
        host::ToAgent::Cancel { agent } => agent::Event::Stop { agent },
        host::ToAgent::ReadCredit { agent, read } => agent::Event::TurnCredit { agent, read },
    };
    agent_step(domain, env, event);
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
    let event = match request {
        agent::Request::Turn { client, turn } => host::FromAgent::Turn {
            owner: client,
            turn: wire::Turn { turn: turn.turn, spent: turn.spent, read: turn.read, body: turn.body },
        },
        agent::Request::FinishedV2 { client, turns, spent, finish } => {
            host::FromAgent::FinishedV2 { owner: client, turns, spent, finish: translate::finish_v2(finish) }
        }
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
        agent::Request::Rejected { client, account, generation } => {
            if let Some(hosting) = domain.host.hosting(client) {
                out.push(Request::Rejected { run: hosting.run, attempt: hosting.attempt, account, generation });
            }
            return;
        }
        agent::Request::Exhausted { client, account, retry_after } => {
            if let Some(hosting) = domain.host.hosting(client) {
                out.push(Request::Exhausted { run: hosting.run, attempt: hosting.attempt, account, retry_after });
            }
            return;
        }
        agent::Request::Told { client, fact } => host::FromAgent::Facts { owner: client, fact },
        agent::Request::Started { client, agent } => host::FromAgent::Started { owner: client, agent },
        agent::Request::Called { client, call, ask } => {
            host::FromAgent::Called { owner: client, call, ask: translate::ask(ask) }
        }
        agent::Request::Withdrawn { client, call } => host::FromAgent::Withdrawn { owner: client, call },
        agent::Request::Waiting { client } => host::FromAgent::Yielded { owner: client },
        agent::Request::Finished { client, finish } => {
            host::FromAgent::Finished { owner: client, finish: translate::finish(finish) }
        }
        agent::Request::Faulted { client, fault } => {
            host::FromAgent::Faulted { owner: client, fault: translate::fault(fault) }
        }
        agent::Request::Bounced { client, name, bounce } => {
            host::FromAgent::Bounced { owner: client, name, bounce: translate::bounce(bounce) }
        }
        // However it went (refused at the entrance, which the limits rule
        // out, unspawned, or stopped), the agent has gone.
        agent::Request::Gone { client, end: _, detail } => host::FromAgent::Gone { owner: client, detail },
    };
    host_step(domain, env, host::Event::from_agent(event));
}

pub(crate) const fn channel_grant(grant: wire::Grant) -> agent::channel::Grant {
    agent::channel::Grant { account: grant.account, generation: grant.generation, valid: grant.valid }
}

fn grants_to_agent(grants: Box<[wire::Grant]>) -> Box<[agent::channel::Grant]> {
    let mut names = skein_lib::List::with_capacity(u32::try_from(grants.len()).expect("validated grants"));
    for grant in grants {
        names.push(channel_grant(grant)).expect("room for every grant");
    }
    names.into_boxed()
}
