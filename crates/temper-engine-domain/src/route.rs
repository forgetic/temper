//! Exhaustive child routing and synchronous handoffs (jig's domain/root.md, 5).
pub(crate) mod escalation;
pub(crate) mod forge_route;
pub(crate) mod host_route;
pub use host_route::{HostDelivery, HostMessage, HostRequest};
pub(crate) mod inbox;
pub(crate) mod landing;
pub(crate) mod policy_translate;
pub(crate) mod proposals;
pub(crate) mod results;
use crate::assemble::{brief_outputs, finish_brief_plan, take_payload};
use crate::boundary::{
    Assignment, Call, EscalationChoice, Event, ForgeRepository, ForgeStart, ForgeWorkspace, InputCheck, MessageForm,
    Payload, PreparedWorkspace, ProposalChoice, ProposedAction, Read, Request, Tool, Work,
};
use crate::domain::{
    Domain, admits, close, emit, forge_release_ending, internal, journal_outputs, max_out, now, route, save,
    step_released,
};
use crate::limits::{
    Limits, core_limits, end_bytes, environment_core, hello_within, route_bound, route_decision, route_takes,
    tasks_saved_within,
};
use crate::restart::{
    accept_load, begin_dependency_read, connector_restart_done, load_decision, load_outputs, request_load,
    restart_request, take_read,
};
use crate::translate::{
    call_needs_input, checked_call, connector_answer, core_call, ending_words, environment_forge,
    keep_connector_answer, named_call, open_watch, relay_call, tool_kind, view_step, watch_subject,
};
use crate::{CallAnswer, CallKey, Decision, Delivery, Family, Key, Range, Record, Write, loads};
use alloc::boxed::Box;
pub use forge_route::adopt_repository_ask;
use jig_core::{Core, Delegate, ProcedureAction};
use jig_core_accounts as accounts;
use jig_core_authority as authority;
use jig_core_brief as brief;
use jig_core_fleet as fleet;
use jig_core_people as people;
use jig_core_tasks as tasks;
use jig_core_views as views;
pub use landing::{Approval, Freshness, Gate, LandingRule};
use skein_lib::{Env, Id, List, Queue, ReplyTo, Token};
use temper_engine_domain_forge as forge;
use temper_engine_domain_forge_change as forge_change;
use temper_engine_domain_forge_client as forge_client;
use temper_engine_domain_forge_issues as forge_issues;

#[expect(
    clippy::too_many_lines,
    reason = "one exhaustive admission match keeps every input before a single decision close"
)]
pub(crate) fn step_routed(domain: &mut Domain, env: &Env<Limits>, event: Event, out: &mut Queue<Request>) {
    assert!(domain.limits == env.limits, "root uses configured limits");
    assert!(out.room() >= max_out(&env.limits), "root output room reserved");
    if domain.journal.stopped() || domain.core.restart_failure().is_some() {
        discard_after_stop(domain, event);
        return;
    }
    match event {
        Event::Resume => {
            crate::domain::resume_routed(domain, env, out);
            return;
        }
        Event::Released(released) => {
            step_released(domain, env, released, out);
            return;
        }
        Event::Forge(event) => {
            if !domain.ready()
                && let forge::Event::Hint { .. } = &*event
            {
                return;
            }
            domain.work.push(Work::Forge(*event));
        }
        Event::Party(crate::PartyInput::Watch { watcher, sign_in, key, subject }) => {
            let project = jig_core::watch_project(&domain.core, subject);
            open_watch(domain, env, watcher, sign_in, key, project, watch_subject(subject), out);
            return;
        }
        Event::Party(crate::PartyInput::Unwatch { watcher }) => {
            if domain.core.watching.contains_key(&watcher) {
                view_step(domain, env, views::Event::Unwatch { watcher }, out);
            }
            return;
        }
        Event::Party(crate::PartyInput::ViewDelivered { watcher, done }) => {
            view_step(domain, env, views::Event::Delivered { watcher, done }, out);
            return;
        }
        Event::Party(crate::PartyInput::StartRecurring { project, authority, template }) => {
            if domain.ready() && admits(domain, &env.limits) {
                domain.work.push(Work::Core(jig_core::Event::StartRecurring { project, authority, template }));
            }
        }
        Event::Party(crate::PartyInput::Period { project, period, budget }) => {
            if domain.ready() && admits(domain, &env.limits) {
                domain.work.push(Work::Core(jig_core::Event::Period { project, period, budget }));
            }
        }
        Event::Party(crate::PartyInput::ProcedureStep { task, step, connector, code, action }) => {
            if !domain.ready() || !admits(domain, &env.limits) {
                return;
            }
            domain.work.push(Work::Core(jig_core::Event::ProcedureStep { task, step, connector, code, action }));
        }
        Event::Store(crate::StoreInput::Committed { number }) => {
            crate::committed(&mut domain.journal, number);
            return;
        }
        Event::Store(crate::StoreInput::Uncommitted { number }) => {
            let mut journal_out = Queue::with_capacity(1);
            crate::uncommitted(&mut domain.journal, number, &mut journal_out);
            journal_outputs(&mut journal_out, out);
            return;
        }
        Event::Store(crate::StoreInput::Loaded { owner, rows, next }) => {
            if domain.journal.stopped() {
                loads::abandon(&mut domain.loads, owner);
            }
            let mut decision = Some(load_decision(domain, &env.limits).expect("load terminal route room"));
            let mut load_out = Queue::with_capacity(1);
            loads::loaded(&mut domain.loads, owner, rows, next, &mut load_out);
            load_outputs(domain, env, &mut load_out, &mut decision, out);
            accept_load(domain, env, decision);
            return;
        }
        Event::Store(crate::StoreInput::Unloaded { owner }) => {
            if domain.journal.stopped() {
                loads::abandon(&mut domain.loads, owner);
            }
            let mut decision = Some(load_decision(domain, &env.limits).expect("load terminal route room"));
            let mut load_out = Queue::with_capacity(1);
            loads::unloaded(&mut domain.loads, owner, &mut load_out);
            load_outputs(domain, env, &mut load_out, &mut decision, out);
            accept_load(domain, env, decision);
            return;
        }
        Event::Restart => {
            let Some(mut decision) = route_decision(domain, &env.limits) else { return };
            let request = domain.core.restart_begin();
            if request != jig_core::RestartRequest::Idle {
                restart_request(domain, env, &mut decision, request);
                let account = jig_core::Event::Account(accounts::Event::Add {
                    account: domain.core.settings.account,
                    generation: domain.core.settings.account_generation,
                    valid: domain.core.settings.account_valid,
                });
                let routed = jig_core::step(&mut domain.core, &environment_core(env), account);
                route_core_requests(domain, env, &mut decision, routed);
            }
            route_into(domain, env, &mut decision);
            close(domain, env, decision, out);
            return;
        }
        Event::Account(event) => {
            account_event(domain, env, event, out);
            return;
        }
        Event::Worker(crate::WorkerInput::Hello { channel, hello }) => {
            if !hello_within(&hello, &env.limits.fleet) {
                out.push(crate::boundary::delivery_output(Delivery::Refuse { channel }));
                return;
            }
            if !admits(domain, &env.limits) {
                out.push(crate::boundary::delivery_output(Delivery::Refuse { channel }));
                return;
            }
            domain.work.push(Work::Fleet(fleet::Event::Hello { channel, hello }));
        }
        Event::Worker(crate::WorkerInput::Lost { channel }) => {
            lose_channel(domain, env, channel);
            return;
        }
        Event::Party(crate::PartyInput::SignedIn { reply_to, identity }) => {
            if !domain.ready() || !admits(domain, &env.limits) {
                out.push(crate::boundary::delivery_output(Delivery::WebReply {
                    to: reply_to,
                    sign_in: None,
                    reply: people::Reply::Refused(people::Refusal::Busy),
                }));
                return;
            }
            domain.work.push(Work::Core(jig_core::Event::SignIn { reply_to, identity }));
        }
        Event::Party(crate::PartyInput::Ask { reply_to, sign_in, key, ask }) => {
            if let people::Ask::Watch { project, subject } = &ask {
                open_watch(domain, env, reply_to.into_token(), sign_in, key, *project, *subject, out);
                return;
            }
            if !domain.ready() || !admits(domain, &env.limits) {
                out.push(crate::boundary::delivery_output(Delivery::WebReply {
                    to: reply_to,
                    sign_in: None,
                    reply: people::Reply::Refused(people::Refusal::Busy),
                }));
                return;
            }
            domain.work.push(Work::People(people::Event::Ask { reply_to, sign_in, key, ask }));
        }
        Event::Worker(crate::WorkerInput::HostCall { channel, task, attempt, call }) => {
            let bytes = match call.name.len().checked_add(call.tool.len()) {
                Some(size) => size.checked_add(call.input.len()),
                None => None,
            };
            if !domain.ready()
                || !admits(domain, &env.limits)
                || domain.core.fleet.calls() >= env.limits.fleet.calls
                || call.tool.is_empty()
                || !crate::decision::length_within(bytes, env.limits.journal.transcript_bytes)
            {
                out.push(Request::Worker(crate::WorkerRequest::Host(Box::new(HostRequest::Busy {
                    channel,
                    task,
                    attempt,
                    name: call.name,
                }))));
                return;
            }
            domain.work.push(Work::Fleet(fleet::Event::Relay {
                channel,
                run: Token::new(task),
                attempt: Token::new(attempt),
                call,
            }));
        }
        Event::Worker(crate::WorkerInput::DecodedHostCall { to, body }) => {
            domain.work.push(Work::TypedDecoded { to, body });
        }
        Event::Worker(crate::WorkerInput::RenderedHostCall { to, answer }) => host_route::rendered(domain, to, answer),
        Event::Worker(crate::WorkerInput::Inbound { task, attempt, message }) => {
            if domain.ready() && admits(domain, &env.limits) {
                if message.name == 0
                    || message.sender.is_empty()
                    || message.sender.len() > usize::try_from(env.limits.people.identity_bytes).expect("u32 fits usize")
                    || message.words.len() > usize::try_from(env.limits.people.words).expect("u32 fits usize")
                    || !crate::decision::length_within(
                        message.sender.len().checked_add(message.words.len()),
                        env.limits.journal.transcript_bytes,
                    )
                {
                    return;
                }
                let name = Token::new(message.name);
                let Ok(id) = domain.payloads.insert(Some(Payload::Message(message))) else { return };
                domain.work.push(Work::Fleet(fleet::Event::Inbound {
                    run: Token::new(task),
                    attempt: Token::new(attempt),
                    message: fleet::Message { name, sender: id.token(), words: id.token() },
                }));
            }
        }
        Event::Worker(crate::WorkerInput::Call { channel, task, attempt, call, mut body }) => {
            let key = CallKey { task, attempt, completion: body.completion, position: body.position };
            if !domain.ready()
                || !admits(domain, &env.limits)
                || task == 0
                || attempt == 0
                || body.completion == 0
                || domain.core.fleet.calls() >= env.limits.fleet.calls
                || domain.core.pending_calls.contains_key(&key)
                || (call_needs_input(&body.tool) && domain.result_reads.len() >= domain.result_reads.capacity())
                || (!domain.core.call_parts.contains_key(&key)
                    && domain.core.call_parts.len().saturating_add(domain.core.pending_calls.len())
                        >= env.limits.call_records)
            {
                out.push(Request::Worker(crate::WorkerRequest::CallBusy { channel, task, attempt, call }));
                return;
            }
            body = checked_call(body, &env.limits);
            let Ok(id) = domain.payloads.insert(Some(Payload::Call { key, body })) else {
                out.push(Request::Worker(crate::WorkerRequest::CallBusy { channel, task, attempt, call }));
                return;
            };
            domain.work.push(Work::Fleet(fleet::Event::Relay {
                channel,
                run: Token::new(task),
                attempt: Token::new(attempt),
                call: fleet::Call {
                    name: Box::from(call.raw().to_be_bytes()),
                    tool: Box::new([]),
                    writes: false,
                    input: Box::from(id.token().raw().to_be_bytes()),
                    deadline: skein_lib::Duration::ZERO,
                },
            }));
        }
        Event::Worker(crate::WorkerInput::Turn { channel, task, attempt, turn }) => {
            if !domain.ready()
                || !admits(domain, &env.limits)
                || turn.transcript.len() > usize::try_from(env.limits.journal.transcript_bytes).expect("u32 fits")
            {
                out.push(Request::Worker(crate::WorkerRequest::TurnBusy { channel, task, attempt, turn: turn.number }));
                return;
            }
            let number = turn.number;
            let Ok(id) = domain.payloads.insert(Some(Payload::Turn { task, attempt, body: turn })) else {
                out.push(Request::Worker(crate::WorkerRequest::TurnBusy { channel, task, attempt, turn: number }));
                return;
            };
            domain.work.push(Work::Fleet(fleet::Event::Turn {
                channel,
                run: Token::new(task),
                attempt: Token::new(attempt),
                turn: number,
                body: id.token(),
            }));
        }
        Event::Worker(crate::WorkerInput::Answer { channel, task, attempt, cumulative, end, saved, pushed }) => {
            if !domain.ready() || !admits(domain, &env.limits) {
                out.push(Request::Worker(crate::WorkerRequest::AnswerBusy { channel, task, attempt }));
                return;
            }
            if end_bytes(&end) > u64::from(env.limits.tasks.result_bytes).checked_mul(2).expect("bounded result bytes")
                || !tasks_saved_within(saved.as_deref(), env.limits.tasks.saved_resources)
                || pushed.len() > usize::try_from(env.limits.forge.resources_per_task).expect("u32 fits usize")
            {
                out.push(Request::Worker(crate::WorkerRequest::AnswerBusy { channel, task, attempt }));
                return;
            }
            let Ok(id) =
                domain.payloads.insert(Some(Payload::Answer { task, attempt, cumulative, end, saved, pushed }))
            else {
                out.push(Request::Worker(crate::WorkerRequest::AnswerBusy { channel, task, attempt }));
                return;
            };
            domain.work.push(Work::Fleet(fleet::Event::Answer {
                channel,
                run: Token::new(task),
                attempt: Token::new(attempt),
                answer: fleet::Answer::Ended,
                payload: id.token(),
            }));
        }
        Event::Party(crate::PartyInput::ReadEscalation { reply_to, sign_in, task }) => {
            escalation::read(domain, env, reply_to, sign_in, task, out);
            return;
        }
        Event::Party(crate::PartyInput::ReadResult { reply_to, sign_in, task }) => {
            results::begin(domain, env, reply_to, sign_in, results::Query::Named { task }, out);
            return;
        }
        Event::Party(crate::PartyInput::ReadInbox { reply_to, sign_in, most }) => {
            results::begin(domain, env, reply_to, sign_in, results::Query::Inbox { most }, out);
            return;
        }
        Event::Party(crate::PartyInput::ViewInbox { reply_to, sign_in, most, before }) => {
            inbox::begin(domain, env, reply_to, sign_in, most, before, out);
            return;
        }
    }
    // Connector terminals already belong to the root's bounded work queue.
    // Keep them there until the journal can admit the entire child route.
    if !route_takes(domain, &env.limits) {
        return;
    }
    let decision = route(domain, env);
    domain.core.signing_in = None;
    close(domain, env, decision, out);
}

pub(crate) fn lose_channel(domain: &mut Domain, env: &Env<Limits>, channel: Token) {
    let room = skein_lib::JournalRoom { writes: 0, held: 0 };
    let Some(decision) = domain.journal.decision(&room) else { return };
    domain.core.lost_channel(&environment_core(env), channel);
    domain.journal.accept(decision);
}

#[expect(
    clippy::too_many_lines,
    reason = "the root translates each closed core request variant for its connector and journal"
)]
pub(crate) fn route_core_requests(
    domain: &mut Domain,
    env: &Env<Limits>,
    decision: &mut Decision,
    requests: jig_core::Requests,
) {
    match requests {
        jig_core::Requests::Out(mut output) => {
            for _ in 0..output.len() {
                match output.pop().expect("core output count") {
                    jig_core::Request::Write(write) => match write {
                        jig_core::Write::Save(record) => match record {
                            jig_core::Record::Core(jig_core::CoreRecord::Projection(row)) => {
                                save(
                                    decision,
                                    &env.limits,
                                    Write::Save(Record::Core(jig_core::Record::Core(
                                        jig_core::CoreRecord::Projection(row),
                                    ))),
                                );
                            }
                            jig_core::Record::Core(jig_core::CoreRecord::Call(row)) => {
                                let answer = forge_route::effect_answer(domain, row.key, &row.part);
                                if let jig_core::CallPart::Effect { .. } = row.part {
                                    keep_connector_answer(domain, row.key, answer.clone())
                                        .expect("admitted effect call payload");
                                }
                                save_connector_answer(domain, &env.limits, decision, row.key);
                                save(
                                    decision,
                                    &env.limits,
                                    Write::Save(Record::Core(jig_core::Record::Core(jig_core::CoreRecord::Call(row)))),
                                );
                            }
                            jig_core::Record::Core(jig_core::CoreRecord::RunProof(proof)) => {
                                save(
                                    decision,
                                    &env.limits,
                                    Write::Save(Record::Core(jig_core::Record::Core(jig_core::CoreRecord::RunProof(
                                        proof,
                                    )))),
                                );
                            }
                            jig_core::Record::Core(jig_core::CoreRecord::Terminal(terminal)) => {
                                save(
                                    decision,
                                    &env.limits,
                                    Write::Save(Record::Core(jig_core::Record::Core(jig_core::CoreRecord::Terminal(
                                        terminal,
                                    )))),
                                );
                            }
                            jig_core::Record::Core(jig_core::CoreRecord::Turn(turn)) => {
                                save(
                                    decision,
                                    &env.limits,
                                    Write::Save(Record::Core(jig_core::Record::Core(jig_core::CoreRecord::Turn(turn)))),
                                );
                            }
                            jig_core::Record::Core(jig_core::CoreRecord::ProposalDecision(archive)) => {
                                save(
                                    decision,
                                    &env.limits,
                                    Write::Save(Record::Core(jig_core::Record::Core(
                                        jig_core::CoreRecord::ProposalDecision(archive),
                                    ))),
                                );
                            }
                            jig_core::Record::Core(jig_core::CoreRecord::EscalationDecision(archive)) => {
                                save(
                                    decision,
                                    &env.limits,
                                    Write::Save(Record::Core(jig_core::Record::Core(
                                        jig_core::CoreRecord::EscalationDecision(archive),
                                    ))),
                                );
                            }
                            jig_core::Record::People(row) => {
                                save(decision, &env.limits, Write::Save(Record::Core(jig_core::Record::People(row))));
                            }
                            jig_core::Record::Tasks(row) => {
                                save(decision, &env.limits, Write::Save(Record::Core(jig_core::Record::Tasks(row))));
                            }
                            jig_core::Record::Notes(row) => {
                                save(decision, &env.limits, Write::Save(Record::Core(jig_core::Record::Notes(row))));
                            }
                            jig_core::Record::Core(_) => {
                                unreachable!("the core route owns its write family")
                            }
                        },
                        jig_core::Write::Erase(key) => match key {
                            jig_core::Key::Core(jig_core::CoreKey::Projection(key)) => {
                                save(
                                    decision,
                                    &env.limits,
                                    Write::Erase(Key::Core(jig_core::Key::Core(jig_core::CoreKey::Projection(key)))),
                                );
                            }
                            jig_core::Key::People(key) => {
                                save(decision, &env.limits, Write::Erase(Key::Core(jig_core::Key::People(key))));
                            }
                            jig_core::Key::Tasks(key) => {
                                save(decision, &env.limits, Write::Erase(Key::Core(jig_core::Key::Tasks(key))));
                            }
                            jig_core::Key::Core(jig_core::CoreKey::Call(key)) => {
                                if domain.forge.forget_named_answer(named_call(key)).is_some() {
                                    save(
                                        decision,
                                        &env.limits,
                                        Write::Erase(Key::Forge(forge::Key::Call(named_call(key)))),
                                    );
                                }
                                save(
                                    decision,
                                    &env.limits,
                                    Write::Erase(Key::Core(jig_core::Key::Core(jig_core::CoreKey::Call(key)))),
                                );
                            }
                            jig_core::Key::Core(jig_core::CoreKey::RunProof(task)) => {
                                save(
                                    decision,
                                    &env.limits,
                                    Write::Erase(Key::Core(jig_core::Key::Core(jig_core::CoreKey::RunProof(task)))),
                                );
                            }
                            jig_core::Key::Core(
                                jig_core::CoreKey::Deployment
                                | jig_core::CoreKey::Turn { .. }
                                | jig_core::CoreKey::Terminal { .. }
                                | jig_core::CoreKey::ProposalDecision(_)
                                | jig_core::CoreKey::EscalationDecision { .. },
                            ) => {
                                unreachable!("the core route owns its erase family")
                            }
                            jig_core::Key::Notes(key) => {
                                save(decision, &env.limits, Write::Erase(Key::Core(jig_core::Key::Notes(key))));
                            }
                        },
                    },
                    jig_core::Request::Ask { connector, ask } => {
                        let request = match ask {
                            jig_core::Ask::Effect(ask) => {
                                forge_route::effect_ask(domain, env, connector, ask);
                                None
                            }
                            jig_core::Ask::Gather { section, budget } => {
                                Some(brief::GatherRequest::Gather { connector, section, budget })
                            }
                            jig_core::Ask::CutTo { section, size } => {
                                Some(brief::GatherRequest::CutTo { connector, section, size })
                            }
                            jig_core::Ask::Drop { section } => Some(brief::GatherRequest::Drop { connector, section }),
                            jig_core::Ask::Hold { task, resource } => {
                                if connector == domain.config.forge_connector
                                    && let Some(name) = forge_route::forge_name(
                                        domain.config.forge_connector,
                                        &resource,
                                        env.limits.forge.name_bytes,
                                    )
                                {
                                    let from = match domain.forge.hold(&name) {
                                        Some(row) if row.task != task => Some(row.task),
                                        Some(_) | None => None,
                                    };
                                    domain.work.push(Work::Forge(forge::Event::Hold { task, resource: name, from }));
                                }
                                None
                            }
                            jig_core::Ask::EndTopic { task, subscription } => {
                                if connector == domain.config.forge_connector
                                    && let Some(topic) = domain.forge.subscription(task, subscription)
                                {
                                    domain.work.push(Work::Forge(forge::Event::Unsubscribe { task, topic }));
                                }
                                None
                            }
                            jig_core::Ask::Adopt { request, project, adoption } => {
                                let parsed = if connector == domain.config.forge_connector {
                                    forge_route::parse_adoption(project, adoption, connector)
                                } else {
                                    None
                                };
                                match parsed {
                                    Some(adoption) if domain.adoption_restore.len() < env.limits.forge.adoptions => {
                                        let previous = domain.forge.repository(adoption.provider).cloned();
                                        assert!(
                                            domain.adoption_restore.insert(request, previous) == Ok(None),
                                            "one keyed adoption flight"
                                        );
                                        domain
                                            .work
                                            .push(Work::Forge(forge::Event::Adopt { reply_to: request, adoption }));
                                    }
                                    Some(_) => domain.work.push(Work::People(people::Event::Decided {
                                        request,
                                        outcome: people::Outcome::Refused(people::Refusal::Busy),
                                    })),
                                    None => domain.work.push(Work::People(people::Event::Decided {
                                        request,
                                        outcome: people::Outcome::Refused(people::Refusal::Unknown),
                                    })),
                                }
                                None
                            }
                            jig_core::Ask::Close { task, root, ending } => {
                                if connector == domain.config.forge_connector {
                                    domain.work.push(Work::Forge(forge::Event::SettleEffects {
                                        task,
                                        root,
                                        ending: forge_release_ending(ending),
                                    }));
                                }
                                None
                            }
                            jig_core::Ask::Release { task, root, ending, entry } => {
                                if connector == domain.config.forge_connector {
                                    domain.work.push(Work::Forge(forge::Event::Release {
                                        task,
                                        root,
                                        ending: forge_release_ending(ending),
                                        entry,
                                    }));
                                }
                                None
                            }
                            jig_core::Ask::Lost { task, attempt } => {
                                if connector == domain.config.forge_connector {
                                    match domain.forge_left.remove(&(task, attempt)) {
                                        Some(pushed) if !pushed.is_empty() => {
                                            forge_route::left(domain, env, task, attempt, pushed);
                                        }
                                        Some(_) | None => {
                                            domain.work.push(Work::Forge(forge::Event::Lost { task, attempt }));
                                        }
                                    }
                                }
                                None
                            }
                            jig_core::Ask::TaskHoldings { request, project, root, number, executor, spec } => {
                                let holdings = if connector == domain.config.forge_connector {
                                    forge_route::task_holdings(
                                        domain, env, project, root, number, executor, &spec, None,
                                    )
                                } else {
                                    None
                                };
                                domain.work.push(Work::Core(jig_core::Event::Holdings {
                                    request,
                                    connector,
                                    holdings,
                                }));
                                None
                            }
                            jig_core::Ask::DelegateHoldings { request, from, members } => {
                                let holdings = if connector == domain.config.forge_connector {
                                    let mut collected = List::with_capacity(
                                        u32::try_from(members.len()).expect("bounded delegate batch"),
                                    );
                                    let mut failed = false;
                                    for member in members {
                                        match forge_route::task_holdings(
                                            domain,
                                            env,
                                            member.project,
                                            member.root,
                                            member.number,
                                            member.executor,
                                            &member.spec,
                                            Some(from),
                                        ) {
                                            Some(holdings) => collected.push(holdings).expect("one per member"),
                                            None => failed = true,
                                        }
                                    }
                                    if failed { None } else { Some(collected.into_boxed()) }
                                } else {
                                    None
                                };
                                domain.work.push(Work::Core(jig_core::Event::DelegateHoldings {
                                    request,
                                    connector,
                                    holdings,
                                }));
                                None
                            }
                            jig_core::Ask::ProcedureHoldings { task, step, members } => {
                                let holdings = if connector == domain.config.forge_connector {
                                    let mut collected = List::with_capacity(
                                        u32::try_from(members.len()).expect("bounded procedure batch"),
                                    );
                                    let mut failed = false;
                                    for member in members {
                                        match forge_route::task_holdings(
                                            domain,
                                            env,
                                            member.project,
                                            member.root,
                                            member.number,
                                            member.executor,
                                            &member.spec,
                                            Some(task),
                                        ) {
                                            Some(holdings) => collected.push(holdings).expect("one per member"),
                                            None => failed = true,
                                        }
                                    }
                                    if failed { None } else { Some(collected.into_boxed()) }
                                } else {
                                    None
                                };
                                domain.work.push(Work::Core(jig_core::Event::ProcedureHoldings {
                                    task,
                                    step,
                                    connector,
                                    holdings,
                                }));
                                None
                            }
                            jig_core::Ask::ProjectGoal { feed } => {
                                if connector == domain.config.forge_connector {
                                    domain.work.push(Work::ProjectGoal(feed));
                                } else if feed.closing {
                                    domain.work.push(Work::Core(jig_core::Event::ProjectionSettled {
                                        goal: feed.goal.number,
                                        connector,
                                    }));
                                }
                                None
                            }
                            jig_core::Ask::ForgetProjection { goal } => {
                                if connector == domain.config.forge_connector {
                                    domain.work.push(Work::Forge(forge::Event::ForgetProjection { goal }));
                                }
                                None
                            }
                            jig_core::Ask::SubscriptionDone { request, key, subscription } => {
                                if connector == domain.config.forge_connector {
                                    if let Some(pending) = domain.forge_subscribing.remove(&request) {
                                        let names = forge_route::watch_names(domain, &env.limits, &pending)
                                            .expect("connector names preflighted at subscription");
                                        let owner = pending.task;
                                        domain
                                            .work
                                            .push(Work::Forge(forge::Event::Subscribe { subscription: pending }));
                                        domain
                                            .work
                                            .push(Work::Forge(forge::Event::Names { task: owner, resources: names }));
                                    }
                                    decide_call(
                                        domain,
                                        &env.limits,
                                        decision,
                                        ReplyTo::new(request),
                                        key,
                                        CallAnswer::Subscribed { subscription },
                                    );
                                }
                                None
                            }
                            jig_core::Ask::UnsubscriptionDone { request, key } => {
                                if connector == domain.config.forge_connector {
                                    if let Some((owner, topic)) = domain.forge_unsubscribing.remove(&request) {
                                        domain.work.push(Work::Forge(forge::Event::Unsubscribe { task: owner, topic }));
                                    }
                                    decide_call(
                                        domain,
                                        &env.limits,
                                        decision,
                                        ReplyTo::new(request),
                                        key,
                                        CallAnswer::Unsubscribed,
                                    );
                                }
                                None
                            }
                            jig_core::Ask::ClaimDone { task, attempt } => {
                                if connector == domain.config.forge_connector {
                                    let (writes, holders) = forge_route::claimed_writes(domain, env, task)
                                        .expect("the admitted assignment retains its bounded held forge writes");
                                    if !writes.is_empty() {
                                        domain.work.push(Work::Forge(forge::Event::Claim {
                                            task,
                                            attempt,
                                            writes,
                                            holders,
                                        }));
                                    }
                                    let key = &domain.assignments.get(&task).expect("claimed assignment").workspace.key;
                                    let workstream =
                                        u64::from_be_bytes(key.as_ref().try_into().expect("task-number workstream"));
                                    emit(
                                        decision,
                                        &env.limits,
                                        Delivery::Fleet(fleet::Event::Start {
                                            kinds: fleet::Kinds::Workers,
                                            reply_to: internal(task),
                                            run: Token::new(task),
                                            attempt: Token::new(attempt),
                                            workstream,
                                            assignment: fleet::Assignment {
                                                turns: Token::new(task),
                                                answered: Token::new(task),
                                            },
                                        }),
                                    );
                                }
                                None
                            }
                            jig_core::Ask::DropSubscription { request } => {
                                if connector == domain.config.forge_connector {
                                    drop(domain.forge_subscribing.remove(&request));
                                    drop(domain.forge_unsubscribing.remove(&request));
                                }
                                None
                            }
                            jig_core::Ask::RepairRefused { repair } => {
                                if connector == domain.config.forge_connector
                                    && let Some(repair) = repair
                                    && let Some(owner) = domain.forge.queue_repair_owner(repair)
                                {
                                    domain.work.push(Work::Tasks(tasks::Event::Hold {
                                        task: owner,
                                        why: tasks::Hold::Effects,
                                    }));
                                }
                                None
                            }
                            jig_core::Ask::DelegateRefused { task } => {
                                if connector == domain.config.forge_connector
                                    && let Some(task) = task
                                    && let Some(row) = domain.forge.change(task)
                                    && let Some((child, _)) = row.delegate
                                {
                                    domain.work.push(Work::Forge(forge::Event::DelegateRefused { task, child }));
                                    domain
                                        .work
                                        .push(Work::Tasks(tasks::Event::Hold { task, why: tasks::Hold::Procedure }));
                                }
                                None
                            }
                            jig_core::Ask::StartProcedure { context, step } => {
                                let task = context.task;
                                let code = match context.executor {
                                    tasks::Executor::Procedure { code, .. } => code,
                                    tasks::Executor::Agent { .. } | tasks::Executor::Person(_) => {
                                        unreachable!("procedure route owns a procedure context")
                                    }
                                };
                                if connector == domain.config.forge_connector && code == 2 {
                                    if !forge_route::start_change(domain, env, &context) {
                                        domain
                                            .work
                                            .push(Work::Tasks(tasks::Event::Hold { task, why: tasks::Hold::Effects }));
                                    }
                                } else {
                                    emit(decision, &env.limits, Delivery::Procedure { task, step, connector, code });
                                }
                                None
                            }
                        };
                        if let Some(request) = request {
                            let mut child = Queue::with_capacity(1);
                            child.push(request);
                            brief_outputs(domain, env, decision, &mut child);
                        }
                    }
                    jig_core::Request::Held(held) => match *held {
                        jig_core::Held::SettledCall { to, call, .. } => {
                            host_route::settled(domain, &env.limits, decision, to, call);
                        }
                        jig_core::Held::Relayed { channel, run, attempt, call: name, answer } => {
                            let delivery = host_route::relayed(domain, channel, run, attempt, name, answer);
                            emit(decision, &env.limits, delivery);
                        }
                        jig_core::Held::Inbound { channel, run, attempt, message } => {
                            match take_payload(domain, message.words).expect("fleet message payload") {
                                Payload::Message(message) => emit(
                                    decision,
                                    &env.limits,
                                    Delivery::Host(Box::new(HostDelivery::Inbound {
                                        channel,
                                        task: run.raw(),
                                        attempt: attempt.raw(),
                                        message,
                                    })),
                                ),
                                Payload::InboxWord(word) => emit(
                                    decision,
                                    &env.limits,
                                    Delivery::Inbound { channel, task: run.raw(), attempt: attempt.raw(), word },
                                ),
                                Payload::Call { .. }
                                | Payload::CallAnswer(_)
                                | Payload::SettledCall(_)
                                | Payload::Turn { .. }
                                | Payload::Answer { .. } => unreachable!("fleet message payload"),
                            }
                        }
                        jig_core::Held::MakeEffect { connector, entry } => {
                            assert!(connector == domain.config.forge_connector, "numbered forge make");
                            emit(decision, &env.limits, Delivery::ForgeCommitted { entry });
                        }
                        jig_core::Held::Relay { task, attempt, previous, word } => {
                            emit(decision, &env.limits, Delivery::Relay { task, attempt, previous, word });
                        }
                        jig_core::Held::PeopleReply { to, sign_in, reply } => {
                            emit(decision, &env.limits, Delivery::WebReply { to, sign_in, reply });
                        }
                        jig_core::Held::CallAnswer { to, key, part } => {
                            let answer = forge_route::effect_answer(domain, key, &part);
                            relay_call(domain, &env.limits, decision, to, answer);
                        }
                        jig_core::Held::ViewStart { run, attempt } => {
                            emit(
                                decision,
                                &env.limits,
                                Delivery::View(Box::new(views::Event::Started { task: run, attempt })),
                            );
                        }
                        jig_core::Held::ViewFinished { task } => {
                            emit(
                                decision,
                                &env.limits,
                                Delivery::View(Box::new(views::Event::Finished { task: Token::new(task) })),
                            );
                        }
                        jig_core::Held::ViewTaskPhase { task, trees, project, phase, priority } => {
                            emit(
                                decision,
                                &env.limits,
                                Delivery::View(Box::new(views::Event::TaskPhase {
                                    task,
                                    trees,
                                    project,
                                    phase,
                                    priority,
                                })),
                            );
                        }
                        jig_core::Held::Result { person, task, words } => {
                            emit(decision, &env.limits, Delivery::Result { person, task, words });
                        }
                        jig_core::Held::Assign { channel, run, attempt, .. } => {
                            let assignment =
                                domain.assignments.remove(&run.raw()).expect("durable claim has prepared assignment");
                            assert!(assignment.attempt == attempt.raw(), "assignment names current attempt");
                            emit(decision, &env.limits, Delivery::Assigned { channel, assignment });
                        }
                        jig_core::Held::Acknowledge { channel, run, attempt } => {
                            emit(
                                decision,
                                &env.limits,
                                Delivery::Acknowledge { channel, task: run.raw(), attempt: attempt.raw() },
                            );
                        }
                        jig_core::Held::TaskTerminalAcknowledged { task, attempt } => {
                            emit(
                                decision,
                                &env.limits,
                                Delivery::Fleet(fleet::Event::Acknowledge {
                                    run: Token::new(task),
                                    attempt: Token::new(attempt),
                                }),
                            );
                        }
                        jig_core::Held::TaskTurnKept { task, attempt, turn } => {
                            emit(
                                decision,
                                &env.limits,
                                Delivery::Fleet(fleet::Event::TurnKept {
                                    run: Token::new(task),
                                    attempt: Token::new(attempt),
                                    turn,
                                }),
                            );
                        }
                        jig_core::Held::ViewTurn { task, attempt, turn } => {
                            emit(
                                decision,
                                &env.limits,
                                Delivery::View(Box::new(views::Event::Turn {
                                    task: Token::new(task),
                                    attempt: Token::new(attempt),
                                    number: turn,
                                })),
                            );
                        }
                        jig_core::Held::AcknowledgeTurn { channel, run, attempt, turn } => {
                            emit(
                                decision,
                                &env.limits,
                                Delivery::AcknowledgeTurn { channel, task: run.raw(), attempt: attempt.raw(), turn },
                            );
                        }
                        jig_core::Held::Cancel { channel, run, attempt } => {
                            emit(
                                decision,
                                &env.limits,
                                Delivery::Cancel { channel, task: run.raw(), attempt: attempt.raw() },
                            );
                        }
                        jig_core::Held::Refuse { channel } => emit(decision, &env.limits, Delivery::Refuse { channel }),
                        jig_core::Held::TurnBusy { channel, run, attempt, turn } => {
                            emit(
                                decision,
                                &env.limits,
                                Delivery::TurnBusy { channel, task: run.raw(), attempt: attempt.raw(), turn },
                            );
                        }
                        jig_core::Held::StopRun { task, attempt } => {
                            emit(decision, &env.limits, Delivery::Fleet(Core::stop_run(task, attempt)));
                        }
                        jig_core::Held::NotesLoad { owner, range } => {
                            match domain.result_reads.insert(Some(Read::Notes { owner })) {
                                Ok(id) => emit(
                                    decision,
                                    &env.limits,
                                    Delivery::Load {
                                        waiter: id.token(),
                                        range: Range::Core(crate::CoreRange::Notes(range)),
                                        after: None,
                                    },
                                ),
                                Err(_) => domain.work.push(Work::Core(jig_core::Event::Notes(
                                    jig_core_notes::Event::LoadFailed { owner },
                                ))),
                            }
                        }
                        jig_core::Held::NotesWritten { .. } | jig_core::Held::NotesDeleted { .. } => {
                            unreachable!("the current application has no note caller")
                        }
                    },
                    jig_core::Request::Now(now) => match *now {
                        jig_core::Now::Relayed { channel, run, attempt, call: name, answer } => {
                            let delivery = host_route::relayed(domain, channel, run, attempt, name, answer);
                            assert!(
                                self::now(domain, crate::boundary::delivery_output(delivery)),
                                "read answer door reserved"
                            );
                        }
                        jig_core::Now::Call { to, run, attempt, call } => {
                            if call.tool.is_empty() {
                                let body = Token::new(
                                    skein_lib::Reader::new(&call.input).u64().expect("decoded call payload"),
                                );
                                relay_payload(domain, env, decision, to, run, attempt, body);
                            } else {
                                host_route::decode(domain, to, run.raw(), attempt.raw(), call);
                            }
                        }
                        jig_core::Now::DropCall { call } => {
                            if call.tool.is_empty() {
                                let body = Token::new(
                                    skein_lib::Reader::new(&call.input).u64().expect("decoded call payload"),
                                );
                                drop(take_payload(domain, body));
                            } else {
                                self::now(
                                    domain,
                                    Request::Worker(crate::WorkerRequest::Host(Box::new(HostRequest::Dropped {
                                        call,
                                    }))),
                                );
                            }
                        }
                        jig_core::Now::Undelivered { run, attempt, message } => {
                            match take_payload(domain, message.words).expect("undelivered message payload") {
                                Payload::Message(_) => {
                                    let _sent = self::now(
                                        domain,
                                        Request::Worker(crate::WorkerRequest::Host(Box::new(
                                            HostRequest::Undelivered {
                                                task: run.raw(),
                                                attempt: attempt.raw(),
                                                message,
                                            },
                                        ))),
                                    );
                                }
                                Payload::InboxWord(_) => {}
                                Payload::Call { .. }
                                | Payload::CallAnswer(_)
                                | Payload::SettledCall(_)
                                | Payload::Turn { .. }
                                | Payload::Answer { .. } => unreachable!("message payload family"),
                            }
                        }
                        jig_core::Now::SettledCallRefused { to, key: _, name, tool } => host_route::settled(
                            domain,
                            &env.limits,
                            decision,
                            to,
                            jig_core::SettledCall {
                                serial: 0,
                                name,
                                tool,
                                answer: jig_core::SettledAnswer::Host { error: true, body: Box::from(&b"busy"[..]) },
                            },
                        ),
                        jig_core::Now::SignInRefused { to } => {
                            emit(
                                decision,
                                &env.limits,
                                Delivery::WebReply {
                                    to,
                                    sign_in: None,
                                    reply: people::Reply::Refused(people::Refusal::Limit),
                                },
                            );
                        }
                        jig_core::Now::WatchRefused { .. } => {
                            unreachable!("volatile watch route handles its own refusal")
                        }
                        jig_core::Now::DropPayload { payload } => drop(take_payload(domain, payload)),
                        jig_core::Now::TurnPayload { run, attempt, turn, body } => {
                            let payload = domain.payloads.get(Id::from_token(body)).expect("fleet returns owned token");
                            let payload = match payload.as_ref().expect("fleet returns owned payload") {
                                Payload::Turn { body: payload, .. } => payload,
                                Payload::Answer { .. }
                                | Payload::Call { .. }
                                | Payload::CallAnswer(_)
                                | Payload::SettledCall(_)
                                | Payload::Message(_)
                                | Payload::InboxWord(_) => {
                                    unreachable!("fleet returns turn family")
                                }
                            };
                            domain.work.push(Work::Core(jig_core::Event::TurnPayload {
                                run,
                                attempt,
                                turn,
                                body,
                                read: payload.read,
                                cumulative: payload.cumulative,
                            }));
                        }
                        jig_core::Now::AnswerPayload { run, attempt, payload } => {
                            let body = domain.payloads.get(Id::from_token(payload)).expect("fleet returns owned token");
                            let (cumulative, end, saved, pushed) =
                                match body.as_ref().expect("fleet returns owned payload") {
                                    Payload::Answer { cumulative, end, saved, pushed, .. } => {
                                        (*cumulative, end.clone(), saved.clone(), pushed.clone())
                                    }
                                    Payload::Turn { .. }
                                    | Payload::Call { .. }
                                    | Payload::CallAnswer(_)
                                    | Payload::SettledCall(_)
                                    | Payload::Message(_)
                                    | Payload::InboxWord(_) => {
                                        unreachable!("fleet returns answer family")
                                    }
                                };
                            domain
                                .forge_left
                                .insert((run.raw(), attempt.raw()), pushed)
                                .expect("one current answer per run");
                            let (saved, invalid_saved) = match saved {
                                Some(tags) => {
                                    match forge_route::saved_resources(domain.config.forge_connector, &tags) {
                                        Some(resources) => (Some(resources), false),
                                        None => (None, true),
                                    }
                                }
                                None => (None, false),
                            };
                            domain.work.push(Work::Core(jig_core::Event::AnswerPayload {
                                run,
                                attempt,
                                payload,
                                cumulative,
                                end,
                                saved,
                                invalid_saved,
                            }));
                        }
                        jig_core::Now::AcceptedTurn { payload, task, attempt, turn, accepted } => {
                            let payload = take_payload(domain, payload).expect("charged turn owns payload");
                            let body = match payload {
                                Payload::Turn { body, .. } => body,
                                Payload::Answer { .. }
                                | Payload::Call { .. }
                                | Payload::CallAnswer(_)
                                | Payload::SettledCall(_)
                                | Payload::Message(_)
                                | Payload::InboxWord(_) => {
                                    unreachable!("turn family")
                                }
                            };
                            domain.work.push(Work::Core(jig_core::Event::AcceptedTurn {
                                task,
                                attempt,
                                turn,
                                accepted,
                                cumulative: body.cumulative,
                                read: body.read,
                                transcript: body.transcript,
                            }));
                        }
                        jig_core::Now::RefusedPayload { request, problem } => {
                            let payload = match take_payload(domain, request) {
                                Some(Payload::Turn { task, attempt, body }) => {
                                    Some(jig_core::PayloadRefusal::Turn { task, attempt, turn: body.number })
                                }
                                Some(Payload::Answer { task, attempt, .. }) => {
                                    domain.forge_left.remove(&(task, attempt));
                                    Some(jig_core::PayloadRefusal::Answer { task, attempt })
                                }
                                Some(
                                    Payload::Call { .. }
                                    | Payload::CallAnswer(_)
                                    | Payload::SettledCall(_)
                                    | Payload::Message(_)
                                    | Payload::InboxWord(_),
                                ) => {
                                    unreachable!("task refusal owns a task payload")
                                }
                                None => None,
                            };
                            domain.work.push(Work::Core(jig_core::Event::RefusedPayload { request, problem, payload }));
                        }
                        jig_core::Now::Activate { context } => {
                            let ready = domain.ready();
                            domain.work.push(Work::Core(jig_core::Event::Activate { context, ready }));
                        }
                        jig_core::Now::PrepareAgent { context } => {
                            let (transcript_waiter, busy) = if context.ever_turned {
                                match domain.result_reads.insert(Some(Read::Transcript { task: context.task })) {
                                    Ok(waiter) => (Some(waiter.token()), false),
                                    Err(_) => (None, true),
                                }
                            } else {
                                (None, false)
                            };
                            domain.work.push(Work::Core(jig_core::Event::PreparedAgent {
                                context,
                                transcript_waiter,
                                busy,
                            }));
                        }
                        jig_core::Now::StartPreparation { task, transcript_waiter } => {
                            domain.work.push(Work::Tasks(tasks::Event::Prepare { reply_to: internal(task), task }));
                            match transcript_waiter {
                                Some(waiter) => emit(
                                    decision,
                                    &env.limits,
                                    Delivery::Load {
                                        waiter,
                                        range: Range::Core(crate::CoreRange::TaskTranscript { task }),
                                        after: None,
                                    },
                                ),
                                None => {
                                    if let Some((waiter, first)) = begin_dependency_read(domain, task) {
                                        emit(
                                            decision,
                                            &env.limits,
                                            Delivery::Load {
                                                waiter,
                                                range: Range::Core(crate::CoreRange::TaskResult { task: first }),
                                                after: None,
                                            },
                                        );
                                    }
                                }
                            }
                        }
                        jig_core::Now::HistoricalProposal { request, person, project, proposer, proposal } => {
                            proposals::historical_begin(
                                domain, env, decision, request, person, project, proposer, proposal,
                            );
                        }
                        jig_core::Now::BriefCorePlanned { task, parent, sections } => {
                            finish_brief_plan(domain, env, task, parent, sections);
                        }
                        jig_core::Now::WorkspaceRequest { task, attempt, context } => {
                            let workspace = forge_route::run_workspace(domain, env, &context, attempt);
                            let writes = match workspace {
                                Some(workspace) => {
                                    match forge_route::claim_names(domain, &env.limits, task, &workspace.names) {
                                        Some(claim_names) => {
                                            let writes = workspace.writes.clone();
                                            let inserted = match domain
                                                .run_workspaces
                                                .insert(task, PreparedWorkspace { workspace, claim_names })
                                            {
                                                Ok(None) => true,
                                                Ok(Some(_)) | Err(_) => false,
                                            };
                                            assert!(inserted, "one workspace flight per task");
                                            Some(writes)
                                        }
                                        None => None,
                                    }
                                }
                                None => None,
                            };
                            domain.work.push(Work::Core(jig_core::Event::WorkspacePrepared { task, attempt, writes }));
                        }
                        jig_core::Now::RunPreparationFailed { task } => {
                            drop(domain.brief_sections.remove(&task));
                            drop(domain.run_workspaces.remove(&task));
                        }
                        jig_core::Now::RunPrepared {
                            task,
                            attempt,
                            charter,
                            run,
                            inbox,
                            saved,
                            transcript,
                            answered,
                            grant,
                        } => {
                            let sections = domain.brief_sections.remove(&task).expect("prepared brief sections");
                            let PreparedWorkspace { workspace, claim_names } =
                                domain.run_workspaces.remove(&task).expect("prepared connector workspace");
                            let mut records = List::with_capacity(domain.limits.call_records);
                            for key in answered {
                                let answer = call_answer(domain, key).expect("live call part has its connector answer");
                                records
                                    .push(crate::CallRecord {
                                        key,
                                        answer,
                                        settled: domain.core.call_settled.get(&key).cloned(),
                                    })
                                    .expect("retained call bound");
                            }
                            let assignment = Assignment {
                                task,
                                attempt,
                                charter,
                                run,
                                sections,
                                inbox,
                                saved: forge_route::saved_tags(&saved, domain.config.forge_connector)
                                    .expect("task saved names were admitted by the root"),
                                workspace: workspace.workspace.clone(),
                                transcript,
                                answered: records.into_boxed(),
                                settled: domain.core.settled_calls(task, attempt),
                                grant,
                            };
                            let budget = assignment.run.budget;
                            assert!(
                                domain.assignments.insert(task, assignment).is_ok(),
                                "assignment fits live task room"
                            );
                            if workspace.names.is_empty() {
                                domain.work.push(Work::Core(jig_core::Event::ClaimPrepared {
                                    task,
                                    attempt,
                                    budget,
                                    writes: Box::new([]),
                                }));
                            } else {
                                let mut hub_writes = List::with_capacity(env.limits.tasks.holdings);
                                for name in &workspace.names {
                                    let resource =
                                        forge_route::hub_name(domain.config.forge_connector, name, &env.limits.tasks)
                                            .expect("admitted forge branch name fits the hub's bound");
                                    hub_writes.push(resource).expect("workspace write bound fits hub");
                                }
                                domain.work.push(Work::Forge(forge::Event::Names { task, resources: claim_names }));
                                for name in workspace.own_holds {
                                    domain.work.push(Work::Forge(forge::Event::Hold {
                                        task,
                                        resource: name,
                                        from: None,
                                    }));
                                }
                                domain.work.push(Work::Core(jig_core::Event::ClaimPrepared {
                                    task,
                                    attempt,
                                    budget,
                                    writes: hub_writes.into_boxed(),
                                }));
                            }
                        }
                        jig_core::Now::CompleteBrief { brief, order } => {
                            let mut child = Queue::with_capacity(1);
                            child.push(brief::GatherRequest::Complete { brief, order });
                            brief_outputs(domain, env, decision, &mut child);
                        }
                        jig_core::Now::ProcedureDelegateOutcome { task, step, child } => {
                            if let Some(kind) = domain.forge_delegating.remove(&(task, step))
                                && let Some(child) = child
                            {
                                domain.work.push(Work::Forge(forge::Event::Delegated { task, child, kind }));
                            }
                        }
                        jig_core::Now::HistoricalEscalation { request, person, project, task, revision } => {
                            escalation::historical_begin(
                                domain, env, decision, request, person, project, task, revision,
                            );
                        }
                        jig_core::Now::EscalationInspection { waiter, context } => {
                            escalation::inspected(domain, waiter, context);
                        }
                        jig_core::Now::EscalationReply { to, person, context } => {
                            emit(decision, &env.limits, Delivery::EscalationReply { to, person, context });
                        }
                        jig_core::Now::EscalationRefused { to, why } => {
                            emit(
                                decision,
                                &env.limits,
                                Delivery::WebReply { to, sign_in: None, reply: people::Reply::Refused(why) },
                            );
                        }
                        jig_core::Now::DropAssignment { task } => {
                            drop(domain.assignments.remove(&task));
                        }
                        jig_core::Now::EffectAnswer { to, key: _, part } => {
                            let answer = CallAnswer::from_core(&part).expect("effect wait answer");
                            relay_call(domain, &env.limits, decision, to, answer);
                        }
                        jig_core::Now::NoteBusy { to, key: _ } => {
                            relay_call(
                                domain,
                                &env.limits,
                                decision,
                                to,
                                CallAnswer::NoteRefused(jig_core_notes::Refusal::Busy),
                            );
                        }
                        jig_core::Now::RestoreRefused => {
                            let _refused = domain.core.restart_refuse();
                            domain.stop_pending = true;
                        }
                        jig_core::Now::Account(request) => {
                            assert!(
                                self::now(domain, Request::Account(request)),
                                "admitted account output fits the door"
                            );
                        }
                        jig_core::Now::View(_)
                        | jig_core::Now::NotesIndexed { .. }
                        | jig_core::Now::NotesRecalled { .. }
                        | jig_core::Now::NotesRefused { .. } => {
                            unreachable!("the current application has no immediate core route")
                        }
                    },
                    jig_core::Request::Decided => {}
                }
            }
        }
    }
}

pub(crate) fn route_into(domain: &mut Domain, env: &Env<Limits>, decision: &mut Decision) {
    for _ in 0..route_bound(&env.limits).expect("valid route bound") {
        let Some(work) = domain.work.pop() else {
            break;
        };
        match work {
            Work::AdoptRestored => {
                if domain.core.restart_admit_claims() {
                    for _ in 0..domain.core.adopted.len() {
                        domain.work.push(Work::Fleet(domain.core.adopted.pop().expect("restored claims")));
                    }
                    domain.work.push(Work::Fleet(fleet::Event::Loaded));
                    domain.work.push(Work::AdoptDone);
                }
            }
            Work::AdoptDone => {
                let request = domain.core.restart_done(jig_core::RestartStep::AdoptRuns);
                domain.work.push(Work::Restart(request));
            }
            Work::Restart(request) => restart_request(domain, env, decision, request),
            Work::TypedDecoded { to, body } => host_route::decoded(domain, env, decision, to, body),
            Work::Core(event) => {
                let routed = jig_core::step(&mut domain.core, &environment_core(env), event);
                route_core_requests(domain, env, decision, routed);
            }
            Work::Tasks(event) => {
                let routed = jig_core::step(&mut domain.core, &environment_core(env), jig_core::Event::Tasks(event));
                route_core_requests(domain, env, decision, routed);
            }
            Work::People(event) => {
                let routed = jig_core::step(&mut domain.core, &environment_core(env), jig_core::Event::People(event));
                route_core_requests(domain, env, decision, routed);
            }
            Work::Fleet(event) => {
                let routed = jig_core::step(&mut domain.core, &environment_core(env), jig_core::Event::Fleet(event));
                route_core_requests(domain, env, decision, routed);
            }
            Work::Brief(event) => {
                let routed = jig_core::step(&mut domain.core, &environment_core(env), jig_core::Event::Brief(event));
                route_core_requests(domain, env, decision, routed);
            }
            Work::StartBrief { task } => domain.work.push(Work::Core(jig_core::Event::StartBrief { task })),
            Work::Forge(event) => {
                let mut out = Queue::with_capacity(forge::max_out(&env.limits.forge));
                forge::step(&mut domain.forge, &environment_forge(env), event, &mut out);
                forge_route::outputs(domain, env, decision, &mut out);
            }
            Work::ProjectGoal(goal) => forge_route::project_goal(domain, env, &goal),
            Work::GoalSubscribe(subscriber) => forge_route::goal_subscribed(domain, env, subscriber),
            Work::Activate(context) => {
                let ready = domain.ready();
                let routed = jig_core::step(
                    &mut domain.core,
                    &environment_core(env),
                    jig_core::Event::Activate { context, ready },
                );
                route_core_requests(domain, env, decision, routed);
            }
            Work::EscalationLoaded { waiter, rows } => escalation::loaded(domain, env, waiter, rows),
            Work::EscalationFailed { waiter } => escalation::failed(domain, waiter),
            Work::ProposalLoaded { waiter, rows } => proposals::historical_loaded(domain, waiter, rows),
            Work::ProposalFailed { waiter } => proposals::historical_failed(domain, waiter),
        }
    }
    assert!(
        domain.work.is_empty(),
        "finite synchronous root handoffs finish within the configured route bound: {:?}",
        domain.work
    );
}

pub(crate) fn call_answer(domain: &Domain, key: CallKey) -> Option<CallAnswer> {
    match domain.core.replay_call(key) {
        Some(jig_core::CallReplay::Core(part)) => Some(forge_route::effect_answer(domain, key, &part)),
        Some(jig_core::CallReplay::Connector { .. }) => connector_answer(domain, key),
        None => None,
    }
}

pub(crate) fn decide_call(
    domain: &mut Domain,
    limits: &Limits,
    decision: &mut Decision,
    to: ReplyTo,
    key: CallKey,
    answer: CallAnswer,
) {
    let part = answer.core_part(domain.config.forge_connector);
    let connector_owned = match &part {
        jig_core::CallPart::Connector { .. } | jig_core::CallPart::Effect { .. } => true,
        jig_core::CallPart::EffectDenied { .. }
        | jig_core::CallPart::ToolDenied { .. }
        | jig_core::CallPart::EscalationDecided { .. }
        | jig_core::CallPart::EscalationRefused(_)
        | jig_core::CallPart::Proposed { .. }
        | jig_core::CallPart::ProposalDecided { .. }
        | jig_core::CallPart::ProposalRefused(_)
        | jig_core::CallPart::Controlled
        | jig_core::CallPart::ControlRefused(_)
        | jig_core::CallPart::ControlDenied { .. }
        | jig_core::CallPart::Sent { .. }
        | jig_core::CallPart::Introduced
        | jig_core::CallPart::MessageRefused(_)
        | jig_core::CallPart::Subscribed { .. }
        | jig_core::CallPart::Unsubscribed
        | jig_core::CallPart::SubscriptionRefused(_)
        | jig_core::CallPart::Delegated(_)
        | jig_core::CallPart::DelegationDenied { .. }
        | jig_core::CallPart::DelegationRefused(_)
        | jig_core::CallPart::NoteWritten { .. }
        | jig_core::CallPart::NoteRecalled { .. }
        | jig_core::CallPart::NoteRefused(_)
        | jig_core::CallPart::Unavailable => false,
    };
    if !domain.core.decide_named_call(key, part) {
        relay_call(domain, limits, decision, to, answer);
        return;
    }
    if connector_owned {
        assert!(keep_connector_answer(domain, key, answer.clone()) == Ok(None), "connector answer room reserved");
    }
    save_connector_answer(domain, limits, decision, key);
    save(
        decision,
        limits,
        Write::Save(Record::Core(jig_core::Record::Core(jig_core::CoreRecord::Call(
            domain.core.call_record(key).expect("admitted core call"),
        )))),
    );
    relay_call(domain, limits, decision, to, answer);
}

#[expect(clippy::too_many_arguments, reason = "validated historical input pointers travel with the delegated call")]
pub(crate) fn delegate_call(
    domain: &mut Domain,
    env: &Env<Limits>,
    decision: &mut Decision,
    to: ReplyTo,
    key: CallKey,
    batch: Box<[Delegate]>,
    validated: bool,
    stubs: Box<[tasks::Stub]>,
) {
    if let Err(part) = domain.core.delegate_preflight(&env.limits.tasks, key, &batch) {
        domain.work.push(Work::Core(jig_core::Event::NamedAnswer { to, key, part }));
        return;
    }
    if !validated {
        let (project, ids) = match domain.core.delegate_inputs(&env.limits.tasks, key, &batch) {
            Ok(value) => value,
            Err(part) => {
                domain.work.push(Work::Core(jig_core::Event::NamedAnswer { to, key, part }));
                return;
            }
        };
        if let Some(&first) = ids.first() {
            let capacity = batch
                .len()
                .checked_mul(usize::try_from(env.limits.tasks.inputs).expect("u32 fits usize"))
                .expect("bounded batch input count");
            let to = to.into_token();
            let read = InputCheck {
                to,
                key,
                batch,
                ids,
                at: 0,
                project,
                stubs: List::with_capacity(u32::try_from(capacity).expect("bounded input IDs")),
            };
            let waiter =
                domain.result_reads.insert(Some(Read::InputCheck(read))).expect("preflighted input read slot").token();
            assert!(domain.core.reserve_connector_call(key), "reserved call record room");
            emit(
                decision,
                &env.limits,
                Delivery::Load {
                    waiter,
                    range: Range::Core(crate::CoreRange::TaskResult { task: first }),
                    after: None,
                },
            );
            return;
        }
    }
    domain.work.push(Work::Core(jig_core::Event::DelegateValidated { to, key, batch, stubs }));
}

#[expect(
    clippy::too_many_lines,
    reason = "the root translates every temper host tool into core or connector vocabulary"
)]
pub(crate) fn relay_payload(
    domain: &mut Domain,
    env: &Env<Limits>,
    decision: &mut Decision,
    reply_to: ReplyTo,
    run: Token,
    attempt: Token,
    body: Token,
) {
    let Some(Payload::Call { key, body }) = take_payload(domain, body) else {
        unreachable!("fleet returns admitted call payload")
    };
    assert!(key.task == run.raw() && key.attempt == attempt.raw(), "fleet call envelope is unchanged");
    assert!(current_proof(domain, key.task, key.attempt), "fleet only relays a current claim");
    if let Some(jig_core::CallPart::Effect {
        deadline,
        outcome:
            None | Some(jig_core::connector::OutboxOutcome::Uncertain | jig_core::connector::OutboxOutcome::Held { .. }),
        ..
    }) = domain.core.call_parts.get(&key)
        && env.wall < *deadline
    {
        let deadline = *deadline;
        domain.work.push(Work::Core(jig_core::Event::EffectStart {
            owner: Token::new(domain.next_effect_owner),
            connector: domain.config.forge_connector,
            origin: jig_core::EffectOrigin::Call { to: reply_to, key, deadline },
        }));
        domain.next_effect_owner = domain.next_effect_owner.checked_add(1).expect("transient effect owner");
        return;
    }
    match call_answer(domain, key) {
        Some(answer) => relay_call(domain, &env.limits, decision, reply_to, answer),
        None => {
            if let Some(kind) = tool_kind(&body.tool)
                && let Some(part) = domain.core.authorize_tool(key, kind)
            {
                domain.work.push(Work::Core(jig_core::Event::NamedAnswer { to: reply_to, key, part }));
                return;
            }
            match body.tool {
                Tool::Unavailable => {
                    domain.work.push(Work::Core(jig_core::Event::NamedAnswer {
                        to: reply_to,
                        key,
                        part: jig_core::CallPart::Unavailable,
                    }));
                }
                Tool::RejectedNote(why) => domain.work.push(Work::Core(jig_core::Event::NamedAnswer {
                    to: reply_to,
                    key,
                    part: jig_core::CallPart::NoteRefused(why),
                })),
                Tool::Note { entry, recalled } => domain.work.push(Work::Core(jig_core::Event::NamedAction {
                    to: reply_to,
                    key,
                    action: jig_core::NamedAction::Note { entry, recalled },
                })),
                Tool::Recall { by, page } => domain.work.push(Work::Core(jig_core::Event::NamedAction {
                    to: reply_to,
                    key,
                    action: jig_core::NamedAction::Recall { by, page },
                })),
                Tool::Rejected(why) => domain.work.push(Work::Core(jig_core::Event::NamedAnswer {
                    to: reply_to,
                    key,
                    part: jig_core::CallPart::DelegationRefused(tasks::Problem { task: None, why, blocked_by: None }),
                })),
                Tool::RejectedMessage(why) => domain.work.push(Work::Core(jig_core::Event::NamedAnswer {
                    to: reply_to,
                    key,
                    part: jig_core::CallPart::MessageRefused(tasks::Problem { task: None, why, blocked_by: None }),
                })),
                Tool::RejectedControl(why) => domain.work.push(Work::Core(jig_core::Event::NamedAnswer {
                    to: reply_to,
                    key,
                    part: jig_core::CallPart::ControlRefused(tasks::Problem { task: None, why, blocked_by: None }),
                })),
                Tool::RejectedProposal(why) => domain.work.push(Work::Core(jig_core::Event::NamedAnswer {
                    to: reply_to,
                    key,
                    part: jig_core::CallPart::ProposalRefused(tasks::Problem { task: None, why, blocked_by: None }),
                })),
                Tool::Delegate { batch } => {
                    delegate_call(domain, env, decision, reply_to, key, batch, false, Box::new([]));
                }
                Tool::Propose { action, reason, as_holder } => match action {
                    ProposedAction::Effect { repository, resource, write } => forge_route::effect_call(
                        domain,
                        env,
                        decision,
                        reply_to,
                        key,
                        repository,
                        resource,
                        *write,
                        Some((reason, as_holder)),
                    ),
                    action @ (ProposedAction::Batch(_)
                    | ProposedAction::Amend { .. }
                    | ProposedAction::Widen { .. }
                    | ProposedAction::Release { .. }) => {
                        proposals::propose_call(domain, reply_to, key, action, reason, as_holder);
                    }
                },
                Tool::Decide { proposer, proposal, decision: choice } => {
                    domain.work.push(Work::Core(jig_core::Event::NamedAction {
                        to: reply_to,
                        key,
                        action: jig_core::NamedAction::DecideProposal {
                            proposer,
                            proposal,
                            choice: match choice {
                                ProposalChoice::Accept => jig_core::ProposalChoice::Accept,
                                ProposalChoice::Reject { reason } => jig_core::ProposalChoice::Reject { reason },
                                ProposalChoice::Pass => jig_core::ProposalChoice::Pass,
                            },
                        },
                    }));
                }
                Tool::Withdraw { proposal } => {
                    domain.work.push(Work::Core(jig_core::Event::NamedAction {
                        to: reply_to,
                        key,
                        action: jig_core::NamedAction::WithdrawProposal { proposal },
                    }));
                }
                Tool::DecideEscalation { task, revision, decision: choice } => {
                    domain.work.push(Work::Core(jig_core::Event::NamedAction {
                        to: reply_to,
                        key,
                        action: jig_core::NamedAction::DecideEscalation {
                            task,
                            revision,
                            choice: match choice {
                                EscalationChoice::Release => jig_core::EscalationChoice::Release,
                                EscalationChoice::Reject { reason } => jig_core::EscalationChoice::Reject { reason },
                                EscalationChoice::Pass => jig_core::EscalationChoice::Pass,
                            },
                        },
                    }));
                }
                Tool::Amend { target, amendment } => {
                    domain.work.push(Work::Core(jig_core::Event::NamedAction {
                        to: reply_to,
                        key,
                        action: jig_core::NamedAction::Amend { target, amendment },
                    }));
                }
                Tool::Cancel { target, reason } => {
                    domain.work.push(Work::Core(jig_core::Event::NamedAction {
                        to: reply_to,
                        key,
                        action: jig_core::NamedAction::Control { target, control: tasks::Control::Cancel { reason } },
                    }));
                }
                Tool::Release { target } => {
                    domain.work.push(Work::Core(jig_core::Event::NamedAction {
                        to: reply_to,
                        key,
                        action: jig_core::NamedAction::Control { target, control: tasks::Control::Release },
                    }));
                }
                Tool::Message { target, form, words } => {
                    let kind = match form {
                        MessageForm::Words => tasks::MessageKind::Words,
                        MessageForm::Question => tasks::MessageKind::Question,
                        MessageForm::Answer { question } => tasks::MessageKind::Answer { question },
                    };
                    domain.work.push(Work::Core(jig_core::Event::NamedAction {
                        to: reply_to,
                        key,
                        action: jig_core::NamedAction::Message { target, kind, words },
                    }));
                }
                Tool::Introduce { left, right } => {
                    domain.work.push(Work::Core(jig_core::Event::NamedAction {
                        to: reply_to,
                        key,
                        action: jig_core::NamedAction::Introduce { left, right },
                    }));
                }
                Tool::Subscribe { kind } => domain.work.push(Work::Core(jig_core::Event::NamedAction {
                    to: reply_to,
                    key,
                    action: jig_core::NamedAction::Subscribe { kind },
                })),
                Tool::SubscribeForge { topic, own_change, paths } => {
                    forge_route::subscribe_call(domain, env, decision, reply_to, key, topic, own_change, paths);
                }
                Tool::ReadForge { repository, read } => {
                    forge_route::read_call(domain, env, decision, reply_to, key, repository, read);
                }
                Tool::EffectForge { repository, resource, write } => {
                    forge_route::effect_call(domain, env, decision, reply_to, key, repository, resource, *write, None);
                }
                Tool::Unsubscribe { subscription } => {
                    let token = reply_to.into_token();
                    if let Some(topic) = domain.forge.subscription(key.task, subscription) {
                        assert!(
                            domain.forge_unsubscribing.insert(token, (key.task, topic)) == Ok(None),
                            "one connector unsubscribe"
                        );
                    }
                    domain.work.push(Work::Core(jig_core::Event::NamedAction {
                        to: ReplyTo::new(token),
                        key,
                        action: jig_core::NamedAction::Unsubscribe { subscription },
                    }));
                }
            }
        }
    }
}

pub(crate) fn account_event(domain: &mut Domain, env: &Env<Limits>, event: accounts::Event, out: &mut Queue<Request>) {
    let room = skein_lib::JournalRoom { writes: 0, held: 0 };
    let Some(decision) = domain.journal.decision(&room) else { return };
    let routed = jig_core::step(&mut domain.core, &environment_core(env), jig_core::Event::Account(event));
    account_outputs(routed, out);
    domain.journal.accept(decision);
}

pub(crate) fn account_outputs(routed: jig_core::Requests, out: &mut Queue<Request>) {
    let jig_core::Requests::Out(mut marked) = routed;
    for _ in 0..marked.len() {
        match marked.pop().expect("account mark count") {
            jig_core::Request::Now(value) => match *value {
                jig_core::Now::Account(request) => out.push(Request::Account(request)),
                jig_core::Now::View(_)
                | jig_core::Now::SignInRefused { .. }
                | jig_core::Now::WatchRefused { .. }
                | jig_core::Now::NotesIndexed { .. }
                | jig_core::Now::NotesRecalled { .. }
                | jig_core::Now::NotesRefused { .. }
                | jig_core::Now::NoteBusy { .. }
                | jig_core::Now::Relayed { .. }
                | jig_core::Now::EffectAnswer { .. }
                | jig_core::Now::DropPayload { .. }
                | jig_core::Now::DropAssignment { .. }
                | jig_core::Now::TurnPayload { .. }
                | jig_core::Now::AnswerPayload { .. }
                | jig_core::Now::AcceptedTurn { .. }
                | jig_core::Now::RefusedPayload { .. }
                | jig_core::Now::Activate { .. }
                | jig_core::Now::PrepareAgent { .. }
                | jig_core::Now::StartPreparation { .. }
                | jig_core::Now::HistoricalProposal { .. }
                | jig_core::Now::BriefCorePlanned { .. }
                | jig_core::Now::WorkspaceRequest { .. }
                | jig_core::Now::RunPreparationFailed { .. }
                | jig_core::Now::RunPrepared { .. }
                | jig_core::Now::CompleteBrief { .. }
                | jig_core::Now::HistoricalEscalation { .. }
                | jig_core::Now::EscalationInspection { .. }
                | jig_core::Now::EscalationReply { .. }
                | jig_core::Now::EscalationRefused { .. }
                | jig_core::Now::ProcedureDelegateOutcome { .. }
                | jig_core::Now::SettledCallRefused { .. }
                | jig_core::Now::Call { .. }
                | jig_core::Now::DropCall { .. }
                | jig_core::Now::Undelivered { .. }
                | jig_core::Now::RestoreRefused => unreachable!("account route owns its now output"),
            },
            jig_core::Request::Decided => {}
            jig_core::Request::Write(_) | jig_core::Request::Ask { .. } | jig_core::Request::Held(_) => {
                unreachable!("account route changes no store decision")
            }
        }
    }
}

pub(crate) fn account_fire(domain: &mut Domain, env: &Env<Limits>, out: &mut Queue<Request>) {
    let routed = jig_core::fire(&mut domain.core, &environment_core(env), jig_core::Timer::Account);
    account_outputs(routed, out);
}

pub(crate) fn discard_after_stop(domain: &mut Domain, event: Event) {
    match event {
        Event::Store(crate::StoreInput::Loaded { owner, rows, next }) => {
            loads::abandon(&mut domain.loads, owner);
            let mut out = Queue::with_capacity(1);
            loads::loaded(&mut domain.loads, owner, rows, next, &mut out);
            assert!(out.is_empty(), "halted waiter emits no delivery");
        }
        Event::Store(crate::StoreInput::Unloaded { owner }) => {
            loads::abandon(&mut domain.loads, owner);
            let mut out = Queue::with_capacity(1);
            loads::unloaded(&mut domain.loads, owner, &mut out);
            assert!(out.is_empty(), "halted waiter emits no delivery");
        }
        Event::Released(_)
        | Event::Worker(
            crate::WorkerInput::HostCall { .. }
            | crate::WorkerInput::DecodedHostCall { .. }
            | crate::WorkerInput::RenderedHostCall { .. }
            | crate::WorkerInput::Inbound { .. }
            | crate::WorkerInput::Hello { .. }
            | crate::WorkerInput::Lost { .. }
            | crate::WorkerInput::Turn { .. }
            | crate::WorkerInput::Call { .. }
            | crate::WorkerInput::Answer { .. },
        )
        | Event::Party(
            crate::PartyInput::StartRecurring { .. }
            | crate::PartyInput::Period { .. }
            | crate::PartyInput::ProcedureStep { .. }
            | crate::PartyInput::SignedIn { .. }
            | crate::PartyInput::Ask { .. }
            | crate::PartyInput::ReadEscalation { .. }
            | crate::PartyInput::ReadResult { .. }
            | crate::PartyInput::ReadInbox { .. }
            | crate::PartyInput::ViewInbox { .. }
            | crate::PartyInput::Watch { .. }
            | crate::PartyInput::Unwatch { .. }
            | crate::PartyInput::ViewDelivered { .. },
        )
        | Event::Forge(_)
        | Event::Restart
        | Event::Resume
        | Event::Store(crate::StoreInput::Committed { .. } | crate::StoreInput::Uncommitted { .. })
        | Event::Account(_) => {}
    }
}

pub(crate) fn current_proof(domain: &Domain, task: u64, attempt: u64) -> bool {
    domain.core.current_proof(task, attempt)
}

pub(crate) fn save_connector_answer(domain: &Domain, limits: &Limits, decision: &mut Decision, key: CallKey) {
    if let Some(answer) = domain.forge.named_answer(named_call(key)) {
        save(
            decision,
            limits,
            Write::Save(Record::Forge { row: Box::new(forge::Stored::Call { key: named_call(key), answer }) }),
        );
    }
}
