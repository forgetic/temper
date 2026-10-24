//! Exhaustive child routing and synchronous handoffs (jig's domain/root.md, 5).
mod applications;
mod asks;
pub(crate) mod demo;
pub(crate) mod escalation;
pub(crate) mod forge_route;
mod held;
pub(crate) mod host_route;
pub use host_route::{HostDelivery, HostRequest};
pub(crate) mod landing;
mod now;
mod panels;
pub(crate) mod policy_translate;
pub(crate) mod proposals;
mod tools;
use crate::archive::take_read;
use crate::assemble::{brief_outputs, take_payload};
use crate::boundary::{
    Assignment, Call, EscalationChoice, Event, ForgeRepository, ForgeStart, ForgeWorkspace, InputCheck, MessageForm,
    Payload, PreparedWorkspace, ProposalChoice, ProposedAction, Read, Request, Tool, Work,
};
use crate::domain::{
    Domain, close, emit, forge_release_ending, internal, journal_outputs, max_out, now, route, save, step_released,
};
use crate::limits::{Limits, core_limits, environment_core, route_bound, route_takes};
use crate::restart::{accept_load, connector_restart_done, load_decision, load_outputs, restart_request};
use crate::translate::{
    call_needs_input, checked_call, connector_answer, core_call, environment_forge, keep_connector_answer, named_call,
    relay_call, tool_kind,
};
use crate::{CallAnswer, CallKey, Decision, Delivery, Family, Key, Range, Record, Write, loads};
use alloc::boxed::Box;
pub use forge_route::adopt_repository_ask;
use jig_domain_authority as authority;
use jig_domain_core as core;
use jig_domain_core::{Delegate, ProcedureStep, StepEnd};
use jig_domain_fleet as fleet;
use jig_domain_people as people;
use jig_domain_tasks as tasks;
pub use landing::{Approval, Freshness, Gate, LandingRule};
use skein_lib::{Env, Id, List, Queue, ReplyTo, Token};
use temper_engine_domain_forge as forge;
use temper_engine_domain_forge_change as forge_change;
use temper_engine_domain_forge_issues as forge_issues;
pub(crate) use tools::relay_payload;

/// Route one admitted input and its synchronous continuations in one decision.
/// Admission is `try_step`'s; the root never answers busy itself (jig's
/// `testing/jig-test-domain/src/admission.rs`).
#[expect(
    clippy::too_many_lines,
    reason = "one exhaustive input match keeps every input before a single decision close"
)]
pub(crate) fn step_routed(domain: &mut Domain, env: &Env<Limits>, event: Event, out: &mut Queue<Request>) {
    crate::domain::invalidate_input(domain, &event);
    assert!(domain.application.is_none(), "application value cleared in its admission step");
    assert!(domain.limits == env.limits, "root uses configured limits");
    assert!(out.room() >= max_out(&env.limits), "root output room reserved");
    if domain.journal.stopped() || domain.core.restart_failure().is_some() {
        discard_after_stop(domain, event);
        return;
    }
    let event = match crate::stopping::defer(domain, event) {
        Ok(event) => event,
        Err(forge) => {
            out.push(Request::Busy(forge));
            return;
        }
    };
    match event {
        Event::Setup { .. } => return, // temper-07 handles the requesting client.
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
        Event::Stop => domain.work.push(Work::Core(core::Event::Stopping)),
        Event::Credential { number, generation, valid } => {
            domain.work.push(Work::Core(core::Event::Account(jig_domain_accounts::Event::Credential {
                number,
                generation,
                valid,
            })));
        }
        Event::Account(event) => domain.work.push(Work::Core(core::Event::Account(event))),
        Event::Party(crate::PartyInput::StartRecurring { project, authority, template }) => {
            domain.work.push(Work::Core(core::Event::StartRecurring { project, authority, template }));
        }
        Event::Party(crate::PartyInput::Period { project, period, budget, next }) => {
            domain.work.push(Work::Core(core::Event::Period { project, period, budget, next }));
        }
        Event::Party(crate::PartyInput::ProcedureStep { task, step, connector, code, decision }) => {
            domain.work.push(Work::Core(core::Event::ProcedureStep { task, step, connector, code, decision }));
        }
        Event::Party(crate::PartyInput::Whose { reply_to, sign_in }) => {
            domain.work.push(Work::People(people::Event::Whose { reply_to, sign_in }));
        }
        Event::Party(crate::PartyInput::ChangeSettings { reply_to, sign_in, key, digest, change }) => {
            domain.work.push(Work::Core(core::Event::ChangeSettings { reply_to, sign_in, key, digest, change }));
        }
        Event::Party(crate::PartyInput::SignOut { reply_to, sign_in }) => {
            domain.work.push(Work::People(people::Event::SignOut { reply_to, sign_in }));
        }
        Event::Party(crate::PartyInput::SignedIn { reply_to, identity, held }) => {
            domain.work.push(Work::Core(core::Event::SignIn { reply_to, identity, held }));
        }
        Event::Party(crate::PartyInput::Application { reply_to, sign_in, key, digest, scope, request, carried }) => {
            let admission = applications::Admission { reply_to, sign_in, key, digest, scope, carried };
            applications::start(domain, admission, request);
        }
        Event::Party(crate::PartyInput::Ask { reply_to, sign_in, key, ask, models }) => {
            domain.work.push(Work::People(people::Event::Ask { reply_to, sign_in, key, ask, models }));
        }
        Event::Party(crate::PartyInput::Key { reply_to, sign_in, key, ask, carried }) => {
            domain.work.push(Work::Core(core::Event::Key { reply_to, sign_in, key, ask, carried }));
        }
        Event::Party(crate::PartyInput::Read { reply_to, sign_in, read }) => {
            domain.work.push(Work::Core(core::Event::Read { reply: reply_to, sign_in, read }));
        }
        Event::Party(crate::PartyInput::Watch { watcher, sign_in, subject, window }) => {
            domain.work.push(Work::Core(core::Event::Watch { watcher, sign_in, subject, window }));
        }
        Event::Party(crate::PartyInput::WatchRun { watcher, sign_in, task, turn, offset }) => {
            domain.work.push(Work::Core(core::Event::WatchRun { watcher, sign_in, task, turn, offset }));
        }
        Event::Party(crate::PartyInput::Unwatch { watcher }) => {
            domain.work.push(Work::Core(core::Event::Unwatch { watcher }));
        }
        Event::Party(crate::PartyInput::Delivered { watcher }) => {
            domain.work.push(Work::Core(core::Event::Delivered { watcher }));
        }
        Event::Party(crate::PartyInput::StreamDelivered { watcher }) => {
            domain.work.push(Work::Core(core::Event::StreamDelivered { watcher }));
        }
        Event::Store(crate::StoreInput::Committed { number, fill }) => {
            crate::committed(&mut domain.journal, number);
            let _filled = core::step(&mut domain.core, &environment_core(env), core::Event::Fill { bytes: fill });
            return;
        }
        Event::Store(crate::StoreInput::Uncommitted { number }) => {
            let mut journal_out = Queue::with_capacity(1);
            crate::uncommitted(&mut domain.journal, number, &mut journal_out);
            journal_outputs(&mut journal_out, out);
            return;
        }
        Event::Store(crate::StoreInput::Loaded { owner, rows, next, texts }) => {
            let mut decision = Some(load_decision(domain, &env.limits).expect("load terminal route room"));
            let mut load_out = Queue::with_capacity(1);
            loads::loaded(&mut domain.loads, owner, rows, next, &mut load_out);
            load_outputs(domain, env, &mut load_out, texts, &mut decision, out);
            accept_load(domain, env, decision);
            return;
        }
        Event::Store(crate::StoreInput::Unloaded { owner }) => {
            let mut decision = Some(load_decision(domain, &env.limits).expect("load terminal route room"));
            let mut load_out = Queue::with_capacity(1);
            loads::unloaded(&mut domain.loads, owner, &mut load_out);
            load_outputs(domain, env, &mut load_out, None, &mut decision, out);
            accept_load(domain, env, decision);
            return;
        }
        Event::Store(crate::StoreInput::ReadLoaded { load, result }) => {
            domain.work.push(Work::Core(core::Event::ReadLoaded { load, result }));
        }
        Event::Restart => {
            let Some(mut decision) = crate::limits::route_decision(domain, &env.limits) else { return };
            let request = domain.core.restart_begin();
            restart_request(domain, env, &mut decision, request);
            route_into(domain, env, &mut decision);
            close(domain, env, decision, out);
            return;
        }
        Event::Worker(crate::WorkerInput::Link { channel, link }) => {
            // Admission keeps room for the record's bytes; past it the
            // record is dropped and its host sends it again after a hello.
            if let Some(event) = crate::link::receive(domain, channel, link) {
                domain.work.push(Work::Core(event));
            }
        }
        Event::Worker(crate::WorkerInput::DecodedHostCall { to, body }) => {
            domain.work.push(Work::Decoded { to, body });
        }
        Event::Worker(crate::WorkerInput::RenderedHostCall { to, answer }) => host_route::rendered(domain, to, answer),
        Event::Worker(crate::WorkerInput::ResultRead { payload, end }) => {
            if let Some(event) = crate::link::result_event(domain, payload, end) {
                domain.work.push(Work::Core(event));
            }
        }
    }
    // Connector terminals already belong to the root's bounded work queue.
    // Keep them there until the journal can admit the entire child route.
    if !route_takes(domain, &env.limits) {
        return;
    }
    let decision = route(domain, env);
    close(domain, env, decision, out);
}

/// Route one core batch: writes join the decision, asks and outputs go to their owners.
pub(crate) fn route_core_requests(
    domain: &mut Domain,
    env: &Env<Limits>,
    decision: &mut Decision,
    requests: core::Requests,
) {
    let core::Requests::Out(mut output) = requests;
    for _ in 0..output.len() {
        match output.pop().expect("core output count") {
            core::Request::Write(core::Write::Save(record)) => {
                if let core::Record::Core(core::CoreRecord::Call(row)) = &record {
                    let answer = forge_route::effect_answer(domain, row.key, &row.part);
                    if let core::CallPart::Effect { .. } = row.part {
                        keep_connector_answer(domain, row.key, answer).expect("admitted effect call payload");
                    }
                    save_connector_answer(domain, &env.limits, decision, row.key);
                }
                save(decision, &env.limits, Write::Save(Record::Core(record)));
            }
            core::Request::Write(core::Write::CopySettingsRevision(number)) => {
                save(decision, &env.limits, Write::CopySettingsRevision(number));
            }
            core::Request::Write(core::Write::Erase(key)) => {
                if let core::Key::Core(core::CoreKey::Call(call)) = key
                    && domain.forge.forget_named_answer(named_call(call)).is_some()
                {
                    save(decision, &env.limits, Write::Erase(Key::Forge(forge::Key::Call(named_call(call)))));
                }
                save(decision, &env.limits, Write::Erase(Key::Core(key)));
            }
            core::Request::Ask { connector, ask } => asks::ask(domain, env, decision, connector, ask),
            core::Request::Held(output) => held::held(domain, env, decision, *output),
            core::Request::Now(output) => now::now(domain, env, decision, *output),
            core::Request::ProjectMade { project } => emit(decision, &env.limits, Delivery::ProjectMade { project }),
            core::Request::Decided => {}
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
                let request = domain.core.restart_done(core::RestartStep::AdoptRuns);
                domain.work.push(Work::Restart(request));
            }
            Work::Restart(request) => restart_request(domain, env, decision, request),
            Work::Decoded { to, body } => host_route::decoded(domain, env, decision, to, body),
            Work::Core(event) => {
                let routed = core::step(&mut domain.core, &environment_core(env), event);
                route_core_requests(domain, env, decision, routed);
            }
            Work::Tasks(event) => {
                let routed = core::step(&mut domain.core, &environment_core(env), core::Event::Tasks(event));
                route_core_requests(domain, env, decision, routed);
            }
            Work::People(event) => {
                let routed = core::step(&mut domain.core, &environment_core(env), core::Event::People(event));
                route_core_requests(domain, env, decision, routed);
            }
            Work::Fleet(event) => {
                let routed = core::step(&mut domain.core, &environment_core(env), core::Event::Fleet(event));
                route_core_requests(domain, env, decision, routed);
            }
            Work::Brief(event) => {
                let routed = core::step(&mut domain.core, &environment_core(env), core::Event::Brief(event));
                route_core_requests(domain, env, decision, routed);
            }
            Work::StartBrief { task } => domain.work.push(Work::Core(core::Event::StartBrief { task })),
            Work::Forge(event) => {
                if let forge::Event::Connection { forge, state: forge::Connection::SetUpFailed { why } } = &event {
                    demo::connection(domain, *forge, why);
                }
                if event == forge::Event::BoxClosed
                    && let Some(config) = domain.forge.demo()
                {
                    demo::connection(domain, config.adoption.forge, b"the box closed the pipe");
                }
                let mut out = Queue::with_capacity(forge::max_out(&env.limits.forge));
                forge::step(&mut domain.forge, &environment_forge(env), event, &mut out);
                forge_route::outputs(domain, env, decision, &mut out);
                demo::progress(domain, env, decision);
            }
            Work::ProjectGoal(goal) => forge_route::project_goal(domain, env, &goal),
            Work::GoalSubscribe(subscriber) => forge_route::goal_subscribed(domain, env, subscriber),
            Work::Activate(context) => {
                let ready = domain.ready();
                let routed =
                    core::step(&mut domain.core, &environment_core(env), core::Event::Activate { context, ready });
                route_core_requests(domain, env, decision, routed);
            }
            Work::EscalationLoaded { waiter, rows } => escalation::loaded(domain, env, waiter, rows),
            Work::EscalationFailed { waiter } => escalation::failed(domain, waiter),
            Work::ProposalLoaded { waiter, rows } => proposals::historical_loaded(domain, waiter, rows),
            Work::ProposalFailed { waiter } => proposals::historical_failed(domain, waiter),
            Work::TaskProposalLoaded { waiter, rows } => proposals::task_loaded(domain, waiter, rows),
            Work::EndedLoaded { waiter, rows } => crate::archive::ended_loaded(domain, waiter, rows),
            Work::EndedFailed { waiter } => crate::archive::ended_failed(domain, waiter),
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
        Some(core::CallReplay::Core(part)) => Some(forge_route::effect_answer(domain, key, &part)),
        Some(core::CallReplay::Connector { .. }) => connector_answer(domain, key),
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
        core::CallPart::Connector { .. } | core::CallPart::Effect { .. } => true,
        core::CallPart::EffectDenied { .. }
        | core::CallPart::ToolDenied { .. }
        | core::CallPart::EscalationDecided { .. }
        | core::CallPart::EscalationRefused(_)
        | core::CallPart::Proposed { .. }
        | core::CallPart::ProposalDecided { .. }
        | core::CallPart::ProposalRefused(_)
        | core::CallPart::Again { .. }
        | core::CallPart::PassedUp { .. }
        | core::CallPart::Revised { .. }
        | core::CallPart::Moved { .. }
        | core::CallPart::Controlled
        | core::CallPart::EffectRefused(_)
        | core::CallPart::ControlRefused(_)
        | core::CallPart::ControlDenied { .. }
        | core::CallPart::Sent { .. }
        | core::CallPart::Introduced
        | core::CallPart::MessageRefused(_)
        | core::CallPart::Subscribed { .. }
        | core::CallPart::Unsubscribed
        | core::CallPart::SubscriptionRefused(_)
        | core::CallPart::Delegated(_)
        | core::CallPart::DelegationDenied { .. }
        | core::CallPart::DelegationRefused(_)
        | core::CallPart::NoteWritten { .. }
        | core::CallPart::NoteRecalled { .. }
        | core::CallPart::NoteRefused(_)
        | core::CallPart::Unavailable => false,
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
        Write::Save(Record::Core(core::Record::Core(core::CoreRecord::Call(
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
        domain.work.push(Work::Core(core::Event::NamedAnswer { to, key, part }));
        return;
    }
    if !validated {
        let (project, ids) = match domain.core.delegate_inputs(&env.limits.tasks, key, &batch) {
            Ok(value) => value,
            Err(part) => {
                domain.work.push(Work::Core(core::Event::NamedAnswer { to, key, part }));
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
                domain.archive_reads.insert(Some(Read::InputCheck(read))).expect("preflighted input read slot").token();
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
    domain.work.push(Work::Core(core::Event::DelegateValidated { to, key, batch, stubs }));
}

pub(crate) fn discard_after_stop(domain: &mut Domain, event: Event) {
    match event {
        Event::Store(crate::StoreInput::Loaded { owner, rows, next, .. }) => {
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
        | Event::Worker(_)
        | Event::Party(_)
        | Event::Forge(_)
        | Event::Credential { .. }
        | Event::Setup { .. }
        | Event::Restart
        | Event::Resume
        | Event::Stop
        | Event::Store(
            crate::StoreInput::Committed { .. }
            | crate::StoreInput::Uncommitted { .. }
            | crate::StoreInput::ReadLoaded { .. },
        )
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
