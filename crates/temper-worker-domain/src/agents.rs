//! Smith process and call bindings at the worker root (jig's domain/hosts.md,
//! sections 4, 5.1 and 6.4). Keeps callback names and exact turn ACK bindings;
//! the hub owns run policy and Smith owns process supervision.

use alloc::boxed::Box;
use jig_host as host;
use skein_lib::{Env, List, Map, Queue, Token};
use smith_host_domain as smith;

use crate::{Domain, Limits, Request, route, translate, workspace};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(crate) struct Callback {
    pub(crate) agent: Token,
    pub(crate) call: Token,
}

#[derive(Debug)]
pub(crate) struct Bindings {
    pub(crate) agents: Map<Token, Token>,
    pub(crate) calls: Map<Callback, [u8; 16]>,
}

impl Bindings {
    pub(crate) fn new(limits: &Limits) -> Self {
        Self {
            agents: Map::with_capacity(limits.agent.agents),
            calls: Map::with_capacity(limits.agent.agents.checked_mul(limits.agent.calls).expect("checked call count")),
        }
    }
}

pub(crate) fn to_agent(domain: &mut Domain, env: &Env<Limits>, request: host::ToAgent) {
    let event = match request {
        host::ToAgent::StartTyped { owner, workspace, charter, activation, turns, answered, grants } => {
            let logical_run = domain.host.hosting(owner).expect("a hosted start").run;
            let (workspace, directories, grants) = match workspace {
                Some(workspace) => {
                    let (directory, directories, grants) = workspace::agent_workspace(domain, workspace, grants);
                    (Some(directory), directories, grants)
                }
                None => {
                    let directories: Box<[smith::Directory]> = Box::new([]);
                    (None, directories, grants_to_smith(grants))
                }
            };
            let mut calls = List::with_capacity(u32::try_from(answered.len()).expect("bounded answered calls"));
            for call in answered {
                let reply = match call.answer {
                    host::SettledAnswer::Host { error, body } => smith::SavedReply::Host { error, body },
                    host::SettledAnswer::Delivery { outcome: _, evidence } => smith::SavedReply::Delivery(Box::new(
                        crate::delivery_evidence::decode(&evidence).expect("admitted delivery evidence"),
                    )),
                };
                calls
                    .push(smith::AnsweredCall {
                        name: translate::named(&call.name).expect("admitted call name"),
                        tool: call.tool,
                        reply,
                    })
                    .expect("room for each answered call");
            }
            smith::Event::Spawn {
                client: owner,
                start: smith::Start {
                    logical_run,
                    activation,
                    workspace,
                    charter,
                    transcript: if turns.is_empty() { None } else { Some(turns) },
                    answered: calls.into_boxed(),
                    directories,
                    grants,
                },
            }
        }
        host::ToAgent::MessageTyped { agent, name, sender, words } => {
            smith::Event::Message { agent, name, label: sender, text: words }
        }
        host::ToAgent::AnswerTyped { agent, call, reply } => {
            let mut found = None;
            for (callback, name) in &domain.bindings.calls {
                if callback.agent == agent && name.as_slice() == call.as_ref() {
                    found = Some(*callback);
                    break;
                }
            }
            let callback = found.expect("every hub call retains its Smith callback");
            domain.bindings.calls.remove(&callback);
            let reply = crate::smith_delivery::reply(domain, reply);
            smith::Event::Answer { agent, call: callback.call, reply }
        }
        host::ToAgent::Grant { agent, grant } => smith::Event::Grant { agent, grant: route::channel_grant(grant) },
        host::ToAgent::Cancel { agent } => smith::Event::Stop { agent },
        host::ToAgent::Acknowledge { agent, turn } => smith::Event::Acknowledge { agent, turn },
    };
    route::agent_step(domain, env, event);
}

fn grants_to_smith(grants: Box<[host::Grant]>) -> Box<[smith::Grant]> {
    let mut names = List::with_capacity(u32::try_from(grants.len()).expect("bounded grants"));
    for grant in grants {
        names.push(route::channel_grant(grant)).expect("room for every grant");
    }
    names.into_boxed()
}

pub(crate) fn from_agent(domain: &mut Domain, env: &Env<Limits>, request: smith::Request, out: &mut Queue<Request>) {
    let event = match request {
        smith::Request::Started { client, agent } => {
            domain.bindings.agents.insert(client, agent).expect("one process per hosted run");
            host::FromAgent::Started { owner: client, agent }
        }
        smith::Request::Admitted { .. } => return,
        smith::Request::Called { client, logical_run: _, call, name, deadline, ask } => {
            let agent = *domain.bindings.agents.get(&client).expect("a started process calls");
            let name = translate::call_name(name);
            domain.bindings.calls.insert(Callback { agent, call }, name).expect("Smith bounded its calls");
            let ask = match ask {
                smith::Ask::Host { tool, effect, body } => host::Ask::RelayTyped {
                    tool,
                    writes: match effect {
                        smith::Effect::Read => false,
                        smith::Effect::Write => true,
                    },
                    input: body,
                    deadline: deadline.saturating_since(env.now),
                },
                smith::Ask::Deliver { fields } => {
                    match crate::smith_delivery::fields(&fields, env.limits.checkout.message_bytes) {
                        Some(crate::smith_delivery::Fields { title, body }) => host::Ask::DeliverV2 { title, body },
                        None => {
                            domain.bindings.calls.remove(&Callback { agent, call });
                            route::agent_step(
                                domain,
                                env,
                                smith::Event::Answer {
                                    agent,
                                    call,
                                    reply: smith::Reply::Delivery(smith::Delivery::Refused(
                                        smith::DeliveryRefusal::new(
                                            None,
                                            Box::from(&b"delivery needs bounded title and body"[..]),
                                        )
                                        .expect("bounded refusal"),
                                    )),
                                },
                            );
                            return;
                        }
                    }
                }
            };
            host::FromAgent::CalledTyped { owner: client, call: Box::from(name), ask }
        }
        smith::Request::Withdrawn { client, call } => {
            let agent = *domain.bindings.agents.get(&client).expect("a started process withdraws");
            let name = *domain.bindings.calls.get(&Callback { agent, call }).expect("outstanding callback");
            host::FromAgent::WithdrawnTyped { owner: client, call: Box::from(name) }
        }
        smith::Request::Turn { client, turn } => host::FromAgent::Turn {
            owner: client,
            turn: host::Turn { turn: turn.number, spent: turn.spent, read: turn.read, body: turn.body },
        },
        smith::Request::Waiting { client, read: _ } => host::FromAgent::Yielded { owner: client },
        smith::Request::Told { client, body } => host::FromAgent::Facts { owner: client, fact: body },
        smith::Request::Answered { client, answer } => host::FromAgent::FinishedV2 {
            owner: client,
            turns: answer.turns,
            spent: answer.spent,
            finish: crate::smith_delivery::finish(answer.result),
        },
        smith::Request::Faulted { client, fault } => {
            host::FromAgent::Faulted { owner: client, fault: translate::fault(fault) }
        }
        smith::Request::Bounced { client, name, bounce } => {
            host::FromAgent::Bounced { owner: client, name, bounce: translate::bounce(bounce) }
        }
        smith::Request::Gone { client, end: _, detail } => {
            domain.bindings.agents.remove(&client);
            host::FromAgent::Gone { owner: client, detail }
        }
        smith::Request::Rejected { client, account, generation } => {
            if let Some(hosting) = domain.host.hosting(client) {
                out.push(Request::Rejected { run: hosting.run, attempt: hosting.attempt, account, generation });
            }
            return;
        }
        smith::Request::Exhausted { client, account, retry_after } => {
            if let Some(hosting) = domain.host.hosting(client) {
                out.push(Request::Exhausted { run: hosting.run, attempt: hosting.attempt, account, retry_after });
            }
            return;
        }
        smith::Request::Spawn { owner, workspace, deadline } => {
            return out.push(Request::Spawn { owner, workspace, deadline });
        }
        smith::Request::Send { owner, process, message } => return out.push(Request::Send { owner, process, message }),
        smith::Request::Read { owner, process } => return out.push(Request::Read { owner, process }),
        smith::Request::Signal { owner, process, signal } => {
            return out.push(Request::Signal { owner, process, signal });
        }
        smith::Request::Wait { owner, process } => return out.push(Request::Wait { owner, process }),
        smith::Request::Reap { owner, process } => return out.push(Request::Reap { owner, process }),
    };
    route::host_step(domain, env, host::Event::from_agent(event));
}
