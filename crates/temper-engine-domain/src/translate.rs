//! Total vocabulary and configuration translations (jig's domain/root.md, 6).
use crate::boundary::{Call, EscalationChoice, MessageForm, Payload, ProposalChoice, ProposedAction, Request, Tool};
use crate::domain::{Domain, emit};
use crate::limits::{Limits, environment_core};
use crate::route::{LandingRule, host_route};
use crate::{CallAnswer, Decision, Delivery};
use alloc::boxed::Box;
use jig_core::RunPolicy;
use jig_core_authority as authority;
use jig_core_fleet as fleet;
use jig_core_notes as notes;
use jig_core_people as people;
use jig_core_tasks as tasks;
use jig_core_views as views;
use skein_lib::{Env, List, Map, Queue, ReplyTo, Token};
use temper_engine_domain_forge as forge;
use temper_engine_domain_forge_client as forge_client;

/// Startup configuration owned by the root, bounded by the corresponding
/// child limits; the shell supplies it once (domain/engine.md, 4).
#[derive(Debug)]
pub struct Config {
    /// Identity committed at the first start; ignored when a header exists.
    pub deployment: [u8; 16],
    pub seed: u64,
    /// Bootstrap project/identity owners supplied by deployment configuration, at most people
    /// `initial_owners`; sign-in authentication matches their identity keys rather than trusting a
    /// request's role.
    pub owners: Box<[people::InitialOwner]>,
    /// Provider number reserved for services made by this deployment; forge sign-in uses 0.
    pub deployment_provider: u16,
    /// Validated deployment and project policy; no duplicated policy values.
    pub authority: authority::Domain,
    /// Forge landing requirements and their connector-owned judge parameters.
    pub landing: LandingPolicy,
    /// Project policy mappings from connector permissions to roles at adoption.
    pub permission_roles: Map<u32, Box<[people::PermissionRole]>>,
    /// Connector number assigned to this application's forge adapter.
    pub forge_connector: u16,
    /// Procedure namespace assigned to the root's recurring goal executor.
    pub recurring_connector: u16,
    /// Charter selected for chats; admitted by tasks as the configured executor.
    pub charter: u32,
    /// Smith-neutral run policy selected for this charter. The root owns the
    /// policy; its typed adapter supplies Smith's vocabulary at the boundary.
    pub run: RunPolicy,
    /// Maximum committed conversation bytes across a task's attempts that may be resumed whole.
    /// A longer transcript starts the next run fresh with its bounded tail in the brief.
    pub resume_bytes: u32,
    /// Finite period number used to address project/person funding ledgers; restoring an existing
    /// ledger never resets it.
    pub period: u64,
    /// Initial project period budget, checked against current project policy before its first
    /// opening; restored funding wins.
    pub period_budget: u64,
    /// Initial person pool budget, checked against the authenticated role's period ceiling before
    /// carving; restored funding wins.
    pub person_budget: u64,
    /// Exact task authority checked for each authenticated chat.
    pub chat_authority: authority::Authority,
    /// One-bit family assignments for the engine tools in this charter.
    pub tools: jig_core::ToolFamilies,
    /// Account required by the chat charter, configured through the accounts child.
    pub account: u32,
    /// Existing secret-free generation at startup.
    pub account_generation: u64,
    /// Startup credential lifetime; a refresh is required when absent.
    pub account_valid: Option<skein_lib::Duration>,
    /// Forge writer identities and this deployment's branch namespace.
    pub forge: forge_client::Config,
}

#[derive(Debug)]
pub(crate) struct RootConfig {
    pub(crate) forge_connector: u16,
}

pub(crate) fn split_config(config: Config, projects: List<u32>) -> (jig_core::Config, RootConfig) {
    let Config {
        deployment,
        seed,
        owners,
        deployment_provider,
        authority,
        landing: _,
        permission_roles,
        forge_connector,
        recurring_connector,
        charter,
        run,
        resume_bytes,
        period,
        period_budget,
        person_budget,
        chat_authority,
        tools,
        account,
        account_generation,
        account_valid,
        forge: _,
    } = config;
    (
        jig_core::Config {
            deployment,
            seed,
            owners,
            authority,
            projects,
            permission_roles,
            connectors: Box::new([forge_connector]),
            settings: jig_core::Settings {
                deployment_provider,
                recurring_connector,
                charter,
                run,
                resume_bytes,
                period,
                period_budget,
                person_budget,
                chat_authority,
                tools,
                account,
                account_generation,
                account_valid,
            },
        },
        RootConfig { forge_connector },
    )
}

/// Typed forge landing policy retained by the application for policy snapshots.
#[derive(Debug)]
pub struct LandingPolicy {
    pub deployment: Box<[LandingRule]>,
    pub projects: Map<u32, Box<[LandingRule]>>,
}

pub(crate) fn view_outputs(
    domain: &mut Domain,
    env: &Env<Limits>,
    output: &mut Queue<views::Request>,
    out: &mut Queue<Request>,
) {
    for _ in 0..output.len() {
        let request = output.pop().expect("view output count");
        match request {
            views::Request::Ended { watcher, .. } | views::Request::Refused { watcher, .. } => {
                domain.core.watch_closed(&environment_core(env), watcher);
            }
            views::Request::Watching { .. } | views::Request::Deliver { .. } => {}
        }
        out.push(Request::Party(crate::PartyRequest::View(request)));
    }
}

pub(crate) fn view_requests(routed: jig_core::Requests, room: u32) -> Queue<views::Request> {
    let mut child = Queue::with_capacity(room);
    let jig_core::Requests::Out(mut marked) = routed;
    for _ in 0..marked.len() {
        match marked.pop().expect("view mark count") {
            jig_core::Request::Now(value) => match *value {
                jig_core::Now::View(request) => child.push(request),
                jig_core::Now::Account(_) => unreachable!("view route does not own account output"),
                jig_core::Now::SignInRefused { .. }
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
                | jig_core::Now::RestoreRefused => unreachable!("view route owns its now output"),
            },
            jig_core::Request::Decided => {}
            jig_core::Request::Write(_) | jig_core::Request::Ask { .. } | jig_core::Request::Held(_) => {
                unreachable!("view route changes no decision")
            }
        }
    }
    child
}

pub(crate) fn view_step(domain: &mut Domain, env: &Env<Limits>, event: views::Event, out: &mut Queue<Request>) {
    let room = skein_lib::JournalRoom { writes: 0, held: 0 };
    let Some(decision) = domain.journal.decision(&room) else { return };
    view_into(domain, env, event, out);
    domain.journal.accept(decision);
}

/// Route inside an already reserved decision, including a released projection.
pub(crate) fn view_into(domain: &mut Domain, env: &Env<Limits>, event: views::Event, out: &mut Queue<Request>) {
    let routed = jig_core::step(&mut domain.core, &environment_core(env), jig_core::Event::View(event));
    let mut child = view_requests(routed, views::max_out(&env.limits.views));
    view_outputs(domain, env, &mut child, out);
}

pub(crate) fn watch_subject(subject: views::Subject) -> people::WatchSubject {
    match subject {
        views::Subject::Run { task, attempt } => people::WatchSubject::Run { task: task.raw(), attempt: attempt.raw() },
        views::Subject::Tree { task } => people::WatchSubject::Tree { task: task.raw() },
        views::Subject::Goals { .. } => people::WatchSubject::Goals,
        views::Subject::Inbox { party } => people::WatchSubject::Inbox { party },
    }
}

/// A volatile watch has no durable root decision; the core admits its party and opens its view.
#[expect(clippy::too_many_arguments, reason = "watch admission carries the signed-in caller, key and subject")]
pub(crate) fn open_watch(
    domain: &mut Domain,
    env: &Env<Limits>,
    watcher: Token,
    sign_in: u64,
    key: [u8; 16],
    project: u32,
    subject: people::WatchSubject,
    out: &mut Queue<Request>,
) {
    let ready = domain.ready() && domain.core.counters.quiescent(domain.journal.idle()) && domain.work.is_empty();
    let room = skein_lib::JournalRoom { writes: 0, held: 0 };
    let Some(decision) = domain.journal.decision(&room) else { return };
    let routed = jig_core::step(
        &mut domain.core,
        &environment_core(env),
        jig_core::Event::Watch { watcher, sign_in, key, project, subject, ready },
    );
    let jig_core::Requests::Out(mut marked) = routed;
    for _ in 0..marked.len() {
        match marked.pop().expect("watch output count") {
            jig_core::Request::Now(value) => match *value {
                jig_core::Now::View(request) => out.push(Request::Party(crate::PartyRequest::View(request))),
                jig_core::Now::WatchRefused { watcher, refusal } => {
                    out.push(Request::Party(crate::PartyRequest::WatchRefused { watcher, refusal }));
                }
                jig_core::Now::SignInRefused { .. }
                | jig_core::Now::Account(_)
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
                | jig_core::Now::RestoreRefused => unreachable!("watch route has only volatile view outputs"),
            },
            jig_core::Request::Decided => {}
            jig_core::Request::Write(_) | jig_core::Request::Ask { .. } | jig_core::Request::Held(_) => {
                unreachable!("watch route changes no store decision")
            }
        }
    }
    domain.journal.accept(decision);
}

pub(crate) fn environment_forge(env: &Env<Limits>) -> Env<forge::Limits> {
    Env { now: env.now, wall: env.wall, limits: env.limits.forge }
}

pub(crate) fn checked_call(mut body: Call, limits: &Limits) -> Call {
    let note_valid = match &body.tool {
        Tool::Note { entry, .. } => notes::valid_new(entry, &limits.notes),
        Tool::Recall { by, .. } => notes::valid_recall(by, &limits.notes),
        Tool::Delegate { .. }
        | Tool::Message { .. }
        | Tool::Amend { .. }
        | Tool::Cancel { .. }
        | Tool::Release { .. }
        | Tool::Decide { .. }
        | Tool::DecideEscalation { .. }
        | Tool::Withdraw { .. }
        | Tool::Propose { .. }
        | Tool::Introduce { .. }
        | Tool::Subscribe { .. }
        | Tool::SubscribeForge { .. }
        | Tool::ReadForge { .. }
        | Tool::EffectForge { .. }
        | Tool::Unsubscribe { .. }
        | Tool::Unavailable
        | Tool::Rejected(_)
        | Tool::RejectedMessage(_)
        | Tool::RejectedControl(_)
        | Tool::RejectedProposal(_)
        | Tool::RejectedNote(_) => true,
    };
    if !note_valid {
        body.tool = Tool::RejectedNote(notes::Refusal::Oversized);
    }
    if let Some(why) = call_shape(&body.tool, &limits.tasks) {
        body.tool = match body.tool {
            Tool::Message { .. } => Tool::RejectedMessage(why),
            Tool::Amend { .. } | Tool::Cancel { .. } | Tool::Release { .. } | Tool::DecideEscalation { .. } => {
                Tool::RejectedControl(why)
            }
            Tool::Propose { .. } | Tool::Decide { .. } | Tool::Withdraw { .. } => Tool::RejectedProposal(why),
            Tool::Note { .. } | Tool::Recall { .. } => Tool::RejectedNote(jig_core_notes::Refusal::Oversized),
            Tool::Delegate { .. }
            | Tool::Introduce { .. }
            | Tool::Subscribe { .. }
            | Tool::SubscribeForge { .. }
            | Tool::ReadForge { .. }
            | Tool::EffectForge { .. }
            | Tool::Unsubscribe { .. }
            | Tool::Rejected(_)
            | Tool::RejectedMessage(_)
            | Tool::RejectedControl(_)
            | Tool::RejectedProposal(_)
            | Tool::RejectedNote(_)
            | Tool::Unavailable => Tool::Rejected(why),
        };
    }
    body
}

pub(crate) fn relay_call(
    domain: &mut Domain,
    limits: &Limits,
    decision: &mut Decision,
    to: ReplyTo,
    answer: CallAnswer,
) {
    let Some((to, answer)) = host_route::render(domain, limits, decision, to, answer) else { return };
    let id = domain.payloads.insert(Some(Payload::CallAnswer(answer))).expect("answer payload room reserved");
    emit(decision, limits, Delivery::Fleet(fleet::Event::Relayed { to, answer: id.token() }));
}

pub(crate) fn call_needs_input(tool: &Tool) -> bool {
    match tool {
        Tool::Unavailable
        | Tool::Rejected(_)
        | Tool::RejectedMessage(_)
        | Tool::RejectedControl(_)
        | Tool::RejectedProposal(_)
        | Tool::RejectedNote(_)
        | Tool::Note { .. }
        | Tool::Recall { .. }
        | Tool::Message { .. }
        | Tool::Introduce { .. }
        | Tool::Subscribe { .. }
        | Tool::SubscribeForge { .. }
        | Tool::ReadForge { .. }
        | Tool::EffectForge { .. }
        | Tool::Unsubscribe { .. }
        | Tool::Cancel { .. }
        | Tool::Release { .. }
        | Tool::Amend { .. }
        | Tool::Propose { .. }
        | Tool::Decide { .. }
        | Tool::Withdraw { .. }
        | Tool::DecideEscalation { .. } => false,
        Tool::Delegate { batch } => {
            for member in batch {
                if !member.spec.inputs.is_empty() {
                    return true;
                }
            }
            false
        }
    }
}

#[expect(clippy::too_many_lines, reason = "one tool shape entrance preflights every bounded call")]
pub(crate) fn call_shape(tool: &Tool, limits: &tasks::Limits) -> Option<tasks::Refusal> {
    match tool {
        Tool::Propose { action, reason, .. } => {
            if reason.len() > usize::try_from(limits.message_bytes).expect("u32 fits usize") {
                return Some(tasks::Refusal::Read);
            }
            match action {
                ProposedAction::Batch(batch) => {
                    if batch.is_empty() || batch.len() > usize::try_from(limits.batch).expect("u32 fits usize") {
                        return Some(tasks::Refusal::Batch);
                    }
                    for member in batch {
                        if !tasks::valid_spec(limits, &member.spec) || !member.spec.inputs.is_empty() {
                            return Some(tasks::Refusal::Spec);
                        }
                        if !tasks::valid_contract(limits, &member.contract)
                            || !tasks::valid_authority(limits, &member.authority)
                            || member.dependencies.len() > usize::try_from(limits.dependencies).expect("u32 fits usize")
                        {
                            return Some(tasks::Refusal::AuthorityShape);
                        }
                    }
                    None
                }
                ProposedAction::Amend { amendment, .. } => {
                    if amendment.reason.len() > usize::try_from(limits.message_bytes).expect("u32 fits usize") {
                        Some(tasks::Refusal::Read)
                    } else if match &amendment.authority {
                        Some(authority) => !tasks::valid_authority(limits, authority),
                        None => true,
                    } {
                        Some(tasks::Refusal::AuthorityShape)
                    } else {
                        None
                    }
                }
                ProposedAction::Widen { authority, .. } => {
                    if tasks::valid_authority(limits, authority) {
                        None
                    } else {
                        Some(tasks::Refusal::AuthorityShape)
                    }
                }
                ProposedAction::Effect { .. } | ProposedAction::Release { .. } => None,
            }
        }
        Tool::Decide { decision, .. } => match decision {
            ProposalChoice::Reject { reason }
                if reason.len() > usize::try_from(limits.message_bytes).expect("u32 fits usize") =>
            {
                Some(tasks::Refusal::Read)
            }
            ProposalChoice::Accept | ProposalChoice::Reject { .. } | ProposalChoice::Pass => None,
        },
        Tool::DecideEscalation { decision, .. } => match decision {
            EscalationChoice::Reject { reason }
                if reason.len() > usize::try_from(limits.result_bytes).expect("u32 fits usize") =>
            {
                Some(tasks::Refusal::Read)
            }
            EscalationChoice::Release | EscalationChoice::Reject { .. } | EscalationChoice::Pass => None,
        },
        Tool::Withdraw { .. }
        | Tool::Unavailable
        | Tool::Rejected(_)
        | Tool::RejectedMessage(_)
        | Tool::RejectedControl(_)
        | Tool::RejectedProposal(_)
        | Tool::RejectedNote(_)
        | Tool::Note { .. }
        | Tool::Recall { .. }
        | Tool::Introduce { .. }
        | Tool::Subscribe { .. }
        | Tool::SubscribeForge { .. }
        | Tool::ReadForge { .. }
        | Tool::EffectForge { .. }
        | Tool::Unsubscribe { .. }
        | Tool::Release { .. } => None,
        Tool::Cancel { reason, .. } => {
            if reason.len() > usize::try_from(limits.result_bytes).expect("u32 fits usize") {
                Some(tasks::Refusal::Read)
            } else {
                None
            }
        }
        Tool::Amend { amendment, .. } => {
            if amendment.reason.len() > usize::try_from(limits.message_bytes).expect("u32 fits usize") {
                return Some(tasks::Refusal::Read);
            }
            if let Some(spec) = &amendment.spec
                && (!tasks::valid_spec(limits, spec) || !spec.inputs.is_empty())
            {
                return Some(tasks::Refusal::Spec);
            }
            if let Some(authority) = &amendment.authority
                && !tasks::valid_authority(limits, authority)
            {
                return Some(tasks::Refusal::AuthorityShape);
            }
            if let Some(dependencies) = &amendment.dependencies
                && dependencies.len() > usize::try_from(limits.dependencies).expect("u32 fits usize")
            {
                return Some(tasks::Refusal::Dependencies);
            }
            None
        }
        Tool::Message { form, words, .. } => {
            if words.is_empty() || words.len() > usize::try_from(limits.message_bytes).expect("u32 fits usize") {
                return Some(tasks::Refusal::Read);
            }
            match form {
                MessageForm::Answer { question } if *question == 0 => return Some(tasks::Refusal::Read),
                MessageForm::Words | MessageForm::Question | MessageForm::Answer { .. } => {}
            }
            None
        }
        Tool::Delegate { batch } => {
            if batch.len() > usize::try_from(limits.batch).expect("u32 fits usize") {
                return Some(tasks::Refusal::Batch);
            }
            for member in batch {
                if !tasks::valid_spec(limits, &member.spec) {
                    return Some(tasks::Refusal::Spec);
                }
                if !tasks::valid_contract(limits, &member.contract) {
                    return Some(tasks::Refusal::Contract);
                }
                if !tasks::valid_authority(limits, &member.authority) {
                    return Some(tasks::Refusal::AuthorityShape);
                }
                if member.dependencies.len() > usize::try_from(limits.dependencies).expect("u32 fits usize") {
                    return Some(tasks::Refusal::Dependencies);
                }
            }
            None
        }
    }
}

pub(crate) fn tool_kind(tool: &Tool) -> Option<jig_core::ToolKind> {
    Some(match tool {
        Tool::Delegate { .. } => jig_core::ToolKind::Delegate,
        Tool::Message { target, .. } => jig_core::ToolKind::Message { target: *target },
        Tool::Introduce { left, .. } => jig_core::ToolKind::Message { target: *left },
        Tool::Amend { .. } | Tool::Cancel { .. } | Tool::Release { .. } => jig_core::ToolKind::Control,
        Tool::Decide { .. } | Tool::Withdraw { .. } | Tool::DecideEscalation { .. } => jig_core::ToolKind::Decide,
        Tool::Propose { .. } => jig_core::ToolKind::Propose,
        Tool::Subscribe { .. } | Tool::SubscribeForge { .. } | Tool::Unsubscribe { .. } => {
            jig_core::ToolKind::Subscribe
        }
        Tool::EffectForge { .. } => jig_core::ToolKind::Effect,
        Tool::Note { entry, .. } => jig_core::ToolKind::Note { scope: entry.scope.clone() },
        Tool::Recall { by, .. } => jig_core::ToolKind::Recall { by: by.clone() },
        Tool::ReadForge { .. } => jig_core::ToolKind::Read,
        Tool::Unavailable
        | Tool::Rejected(_)
        | Tool::RejectedMessage(_)
        | Tool::RejectedControl(_)
        | Tool::RejectedProposal(_)
        | Tool::RejectedNote(_) => return None,
    })
}

pub(crate) fn result_words(result: tasks::TaskResult) -> Box<[u8]> {
    match result {
        tasks::TaskResult::Report { words }
        | tasks::TaskResult::Verdict { words, .. }
        | tasks::TaskResult::Change { words, .. } => words,
        tasks::TaskResult::Failure { reason } => reason,
    }
}

pub(crate) fn ending_words(ending: tasks::Ending) -> Box<[u8]> {
    match ending {
        tasks::Ending::Done(result) => result_words(result),
        tasks::Ending::Failed { reason } => reason,
        tasks::Ending::Cancelled { reason, result } => match result {
            Some(result) => result_words(result),
            None => reason,
        },
    }
}

pub(crate) const fn named_call(key: crate::CallKey) -> forge::NamedCall {
    forge::NamedCall { task: key.task, attempt: key.attempt, completion: key.completion, position: key.position }
}

pub(crate) const fn core_call(key: forge::NamedCall) -> crate::CallKey {
    crate::CallKey { task: key.task, attempt: key.attempt, completion: key.completion, position: key.position }
}

pub(crate) fn connector_answer(domain: &Domain, key: crate::CallKey) -> Option<CallAnswer> {
    let payload = domain.forge.named_answer(named_call(key))?;
    Some(call_payload_answer(payload))
}

pub(crate) fn call_payload_answer(payload: forge::CallPayload) -> CallAnswer {
    match payload {
        forge::CallPayload::Effect { entry, deadline, outcome } => CallAnswer::ForgeEffect { entry, deadline, outcome },
        forge::CallPayload::Refused(error) => CallAnswer::ForgeEffectRefused(error),
        forge::CallPayload::Read(answer) => CallAnswer::ForgeRead(answer),
    }
}

pub(crate) fn keep_connector_answer(
    domain: &mut Domain,
    key: crate::CallKey,
    answer: CallAnswer,
) -> Result<Option<CallAnswer>, CallAnswer> {
    let payload = match answer {
        CallAnswer::ForgeEffect { entry, deadline, outcome } => forge::CallPayload::Effect { entry, deadline, outcome },
        CallAnswer::ForgeEffectRefused(error) => forge::CallPayload::Refused(error),
        CallAnswer::ForgeRead(answer) => forge::CallPayload::Read(answer),
        answer @ (CallAnswer::ForgeEffectDenied { .. }
        | CallAnswer::ToolDenied { .. }
        | CallAnswer::EscalationDecided { .. }
        | CallAnswer::EscalationRefused(_)
        | CallAnswer::Proposed { .. }
        | CallAnswer::ProposalDecided { .. }
        | CallAnswer::ProposalRefused(_)
        | CallAnswer::Controlled
        | CallAnswer::ControlRefused(_)
        | CallAnswer::ControlDenied { .. }
        | CallAnswer::Sent { .. }
        | CallAnswer::Introduced
        | CallAnswer::MessageRefused(_)
        | CallAnswer::Subscribed { .. }
        | CallAnswer::Unsubscribed
        | CallAnswer::SubscriptionRefused(_)
        | CallAnswer::Delegated(_)
        | CallAnswer::DelegationDenied { .. }
        | CallAnswer::DelegationRefused(_)
        | CallAnswer::NoteWritten { .. }
        | CallAnswer::NoteRecalled { .. }
        | CallAnswer::NoteRefused(_)
        | CallAnswer::Unavailable) => return Err(answer),
    };
    match domain.forge.keep_named_answer(&domain.limits.forge, named_call(key), payload) {
        Ok(Some(payload)) => Ok(Some(call_payload_answer(payload))),
        Ok(None) => Ok(None),
        Err(payload) => Err(call_payload_answer(payload)),
    }
}
