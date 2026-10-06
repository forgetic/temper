//! Actual held-chat reads, routing and authenticated decisions. Root owns
//! transport/query slots and immutable decision evidence; tasks owns the held
//! lifecycle.

use super::{
    CallKey, Decision, Delivery, Domain, Env, EscalationChoice, Family, Id, Limits, Read, ReplyTo, Request, RoutedCall,
    Token, Work, authority, emit, people, save, tasks,
};
use crate::{CallAnswer, EscalationDecisionRecord, Record, Write};
use alloc::boxed::Box;

#[derive(Debug)]
pub(super) enum Query {
    Read {
        to: ReplyTo,
        person: u64,
        task: u64,
    },
    Decide {
        request: Token,
        requester: u64,
        person: u64,
        role: Option<people::Role>,
        project: u32,
        task: u64,
        revision: u64,
        decision: people::EscalationDecision,
    },
}

pub(super) fn role_number(role: people::Role) -> u32 {
    match role {
        people::Role::Owner => 0,
        people::Role::Maintainer => 1,
        people::Role::Member => 2,
        people::Role::Observer => 3,
    }
}

fn fallback(domain: &Domain, project: u32) -> Option<tasks::EscalationHolder> {
    // The policy names the escalation role; this slice resolves that role here.
    let role = domain.config.authority.policy(project)?.escalation_role?;
    if role > 3 {
        return None;
    }
    Some(tasks::EscalationHolder::Role { project, role })
}

fn covers(domain: &Domain, context: &tasks::EscalationContext, person: u64, role: Option<people::Role>) -> bool {
    let Some(role) = role else { return false };
    let pool = tasks::Funder::Pool { project: context.project, person, period: domain.config.period };
    let numbers = match domain.tasks.funding(pool) {
        Some(record) => record.numbers,
        None => tasks::Numbers { budget: 0, spent: 0, spent_below: 0, reserved: 0 },
    };
    let Some(needed) = authority::needs(&authority::Action::Escalate { release: None }) else { return false };
    authority::covers(
        &domain.config.authority,
        &needed,
        &authority::Holder::Person {
            project: context.project,
            role: role_number(role),
            proposal: authority::ProposalKind::Escalation,
            pool: super::authority_numbers(numbers),
            tasks_left: domain.limits.tasks.tree_tasks,
        },
        0,
    )
}

pub(super) fn supported(domain: &Domain, task: &tasks::TaskRecord) -> bool {
    let Some(fallback) = fallback(domain, task.project) else { return false };
    match &task.escalation {
        tasks::Escalation::Waiting { holder: tasks::EscalationHolder::Role { .. }, entry, .. } => {
            // An old selector can be rechecked, but its project must remain the chat's.
            *entry <= domain.journal.deployment().messages
                && match fallback {
                    tasks::EscalationHolder::Role { project, .. } => project == task.project,
                    tasks::EscalationHolder::Task(_) | tasks::EscalationHolder::Person(_) => false,
                }
        }
        tasks::Escalation::Rejected { by, .. } => {
            *by <= domain.journal.deployment().people.max(domain.journal.deployment().tasks)
        }
        tasks::Escalation::Waiting { entry, .. } => *entry <= domain.journal.deployment().messages,
        tasks::Escalation::Unheld { .. } | tasks::Escalation::Routing { .. } => true,
    }
}

/// Pure recipient selection for actual startup/live routing and candidate-roster
/// preflight; preserves a final-role holder.
pub(super) fn recipient(
    domain: &Domain,
    context: &tasks::EscalationContext,
    requester_role: Option<people::Role>,
) -> Option<tasks::EscalationHolder> {
    recipient_after(domain, context, requester_role, None)
}

fn recipient_after(
    domain: &Domain,
    context: &tasks::EscalationContext,
    requester_role: Option<people::Role>,
    after: Option<tasks::EscalationHolder>,
) -> Option<tasks::EscalationHolder> {
    match context.escalation {
        tasks::Escalation::Waiting { holder: tasks::EscalationHolder::Role { .. }, .. } => {
            fallback(domain, context.project)
        }
        tasks::Escalation::Unheld { .. }
        | tasks::Escalation::Routing { .. }
        | tasks::Escalation::Waiting { .. }
        | tasks::Escalation::Rejected { .. } => {
            let needed = authority::needs(&authority::Action::Escalate { release: None })?;
            let mut above = context.immediate;
            let mut distance = 1_u32;
            let mut passed = after.is_none();
            for _ in 0..domain.limits.tasks.depth.saturating_add(1) {
                let tasks::Party::Task(parent) = above else { break };
                let Some(holder) = domain.tasks.delegation(parent) else { return fallback(domain, context.project) };
                if passed
                    && holder.deciding
                    && holder.project == context.project
                    && authority::covers(
                        &domain.config.authority,
                        &needed,
                        &authority::Holder::Task {
                            project: context.project,
                            authority: super::authority_value(&holder.authority),
                            numbers: super::authority_numbers(holder.numbers),
                            tasks_left: holder.tasks_left,
                        },
                        distance,
                    )
                {
                    return Some(tasks::EscalationHolder::Task(parent));
                }
                if after == Some(tasks::EscalationHolder::Task(parent)) {
                    passed = true;
                }
                above = holder.requester;
                distance = distance.checked_add(1)?;
            }
            if passed && context.requester != 0 && covers(domain, context, context.requester, requester_role) {
                Some(tasks::EscalationHolder::Person(context.requester))
            } else {
                fallback(domain, context.project)
            }
        }
    }
}

pub(super) fn needed(domain: &mut Domain, context: Box<tasks::EscalationContext>) {
    let Some(holder) = recipient(domain, &context, domain.people.role(context.requester, context.project)) else {
        domain.startup = super::Startup::Failed;
        return;
    };
    match context.escalation {
        tasks::Escalation::Waiting { holder: old, .. } if old == holder => return,
        tasks::Escalation::Waiting { revision, holder: old, .. } if old != holder && revision == u64::MAX => {
            // A changed restored recipient cannot reuse an exhausted semantic revision.
            domain.startup = super::Startup::Failed;
            return;
        }
        tasks::Escalation::Unheld { .. }
        | tasks::Escalation::Routing { .. }
        | tasks::Escalation::Waiting { .. }
        | tasks::Escalation::Rejected { .. } => {}
    }
    domain.work.push(Work::Tasks(tasks::Event::RoutedEscalation {
        task: context.task,
        revision: context.escalation.revision(),
        holder,
        entry: match crate::fresh(&mut domain.journal, Family::Message) {
            Some(entry) => entry,
            None => {
                domain.startup = super::Startup::Failed;
                return;
            }
        },
    }));
}

pub(super) fn stalled(domain: &mut Domain, task: u64, revision: u64, old: tasks::EscalationHolder) {
    let Some(context) = domain.tasks.escalation(task) else { return };
    let tasks::Escalation::Waiting { revision: current, holder, .. } = context.escalation else { return };
    if current != revision || holder != old {
        return;
    }
    let role = domain.people.role(context.requester, context.project);
    if let Some(next) = recipient_after(domain, &context, role, Some(old)) {
        let Some(entry) = crate::fresh(&mut domain.journal, Family::Message) else {
            domain.startup = super::Startup::Failed;
            return;
        };
        domain.work.push(Work::Tasks(tasks::Event::RoutedEscalation { task, revision, holder: next, entry }));
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "one named task holder decision carries its current claim and held revision"
)]
pub(super) fn task_decide(
    domain: &mut Domain,
    env: &Env<Limits>,
    barrier: &mut Decision,
    to: ReplyTo,
    key: CallKey,
    task: u64,
    revision: u64,
    choice: EscalationChoice,
) {
    if !super::current_proof(domain, key.task, key.attempt) {
        return refuse_task(domain, env, barrier, to, key, task, tasks::Refusal::Attempt);
    }
    let Some(context) = domain.tasks.escalation(task) else {
        return refuse_task(domain, env, barrier, to, key, task, tasks::Refusal::Unknown);
    };
    let tasks::Escalation::Waiting { revision: current, holder, .. } = context.escalation else {
        return refuse_task(domain, env, barrier, to, key, task, tasks::Refusal::State);
    };
    if current != revision || holder != tasks::EscalationHolder::Task(key.task) {
        return refuse_task(domain, env, barrier, to, key, task, tasks::Refusal::Reference);
    }
    let role = domain.people.role(context.requester, context.project);
    let semantic = match choice {
        EscalationChoice::Release => {
            if recipient(domain, &context, role) != Some(holder) {
                return refuse_task(domain, env, barrier, to, key, task, tasks::Refusal::Funding);
            }
            tasks::EscalationDecision::Release
        }
        EscalationChoice::Reject { reason } => tasks::EscalationDecision::Reject { reason },
        EscalationChoice::Pass => {
            let Some(next) = recipient_after(domain, &context, role, Some(holder)) else {
                return refuse_task(domain, env, barrier, to, key, task, tasks::Refusal::Reference);
            };
            tasks::EscalationDecision::Pass { holder: next }
        }
    };
    let entry = match semantic {
        tasks::EscalationDecision::Pass { .. } => match crate::fresh(&mut domain.journal, Family::Message) {
            Some(entry) => Some(entry),
            None => return refuse_task(domain, env, barrier, to, key, task, tasks::Refusal::Busy),
        },
        tasks::EscalationDecision::Release | tasks::EscalationDecision::Reject { .. } => None,
    };
    let token = to.into_token();
    assert!(domain.pending_calls.insert(key, true).is_ok(), "task escalation call room reserved");
    assert!(
        domain.routing_calls.insert(token, RoutedCall::Escalation { key, task, revision }) == Ok(None),
        "one route"
    );
    domain.work.push(Work::TaskEscalation(tasks::Event::DecideEscalation {
        reply_to: ReplyTo::new(token),
        task,
        revision,
        by: key.task,
        entry,
        decision: semantic,
    }));
}

fn refuse_task(
    domain: &mut Domain,
    env: &Env<Limits>,
    barrier: &mut Decision,
    to: ReplyTo,
    key: CallKey,
    task: u64,
    why: tasks::Refusal,
) {
    super::decide_call(
        domain,
        &env.limits,
        barrier,
        to,
        key,
        CallAnswer::EscalationRefused(tasks::Problem { task: Some(task), why }),
    );
}

pub(super) fn read(
    domain: &mut Domain,
    env: &Env<Limits>,
    to: ReplyTo,
    sign_in: u64,
    task: u64,
    out: &mut skein_lib::Queue<Request>,
) {
    let Some(person) = domain.people.person(sign_in, env.now, env.wall) else {
        refuse_direct(to, people::Refusal::SignIn, out);
        return;
    };
    if !domain.ready() || !super::admits(domain, &env.limits) {
        refuse_direct(to, people::Refusal::Busy, out);
        return;
    }
    let query = Query::Read { to, person, task };
    let id = match domain.result_reads.insert(Some(Read::Escalation(query))) {
        Ok(id) => id,
        Err(read) => match read.expect("unadmitted query owns reply") {
            Read::Escalation(Query::Read { to, .. }) => {
                refuse_direct(to, people::Refusal::Busy, out);
                return;
            }
            Read::Proposal(_)
            | Read::Result(_)
            | Read::Inbox(_)
            | Read::Transcript { .. }
            | Read::Dependency(_)
            | Read::InputCheck(_)
            | Read::Escalation(Query::Decide { .. }) => {
                unreachable!("inserted named read")
            }
        },
    };
    domain.work.push(Work::Tasks(tasks::Event::InspectEscalation { reply_to: ReplyTo::new(id.token()), task }));
    let decision = super::route(domain, env);
    super::close(domain, env, decision, out);
}

#[expect(
    clippy::too_many_arguments,
    reason = "explicit bounded keyed route carries authenticated identity and its one typed choice"
)]
pub(super) fn begin(
    domain: &mut Domain,
    request: Token,
    person: u64,
    role: Option<people::Role>,
    project: u32,
    task: u64,
    revision: u64,
    decision: people::EscalationDecision,
) {
    let bounded = match &decision {
        people::EscalationDecision::Release | people::EscalationDecision::Pass => true,
        people::EscalationDecision::Reject { reason } => {
            reason.len()
                <= usize::try_from(
                    domain
                        .limits
                        .tasks
                        .result_bytes
                        .min(domain.limits.journal.result_bytes)
                        .min(domain.limits.journal.transcript_bytes),
                )
                .expect("u32 fits usize")
        }
    };
    if !bounded {
        decided(domain, request, people::Outcome::Refused(people::Refusal::Limit));
        return;
    }
    let query = Query::Decide { request, requester: 0, person, role, project, task, revision, decision };
    match domain.result_reads.insert(Some(Read::Escalation(query))) {
        Ok(id) => {
            domain.work.push(Work::Tasks(tasks::Event::InspectEscalation { reply_to: ReplyTo::new(id.token()), task }));
        }
        Err(_) => decided(domain, request, people::Outcome::Refused(people::Refusal::Busy)),
    }
}

fn visible(domain: &Domain, context: &tasks::EscalationContext, person: u64, role: Option<people::Role>) -> bool {
    if person == context.requester {
        return true;
    }
    let Some(role) = role else { return false };
    match fallback(domain, context.project) {
        Some(tasks::EscalationHolder::Role { role: selected, .. }) => role_number(role) == selected,
        Some(tasks::EscalationHolder::Task(_) | tasks::EscalationHolder::Person(_)) | None => false,
    }
}

fn standing(context: &tasks::EscalationContext, person: u64, role: Option<people::Role>) -> bool {
    match &context.escalation {
        tasks::Escalation::Waiting { holder, .. } => match holder {
            tasks::EscalationHolder::Task(_) => false,
            tasks::EscalationHolder::Person(number) => person == *number,
            tasks::EscalationHolder::Role { project, role: selected } => {
                *project == context.project
                    && match role {
                        Some(role) => role_number(role) == *selected,
                        None => false,
                    }
            }
        },
        tasks::Escalation::Unheld { .. } | tasks::Escalation::Routing { .. } | tasks::Escalation::Rejected { .. } => {
            false
        }
    }
}

#[expect(clippy::too_many_lines, reason = "bounded escalation inspection and all terminal cases")]
pub(super) fn inspected(
    domain: &mut Domain,
    env: &Env<Limits>,
    barrier: &mut Decision,
    waiter: Token,
    context: Option<Box<tasks::EscalationContext>>,
) {
    let Some(read) = super::take_read(domain, waiter) else { return };
    let query = match read {
        Read::Escalation(query) => query,
        Read::Result(_)
        | Read::Inbox(_)
        | Read::Proposal(_)
        | Read::Transcript { .. }
        | Read::Dependency(_)
        | Read::InputCheck(_) => {
            unreachable!("escalation query terminal")
        }
    };
    match query {
        Query::Read { to, person, task, .. } => {
            domain.result_reads.retire(Id::from_token(waiter));
            let Some(context) = context else {
                emit(barrier, &env.limits, refusal(to, people::Refusal::Unknown));
                return;
            };
            let role = domain.people.role(person, context.project);
            if context.task != task || !visible(domain, &context, person, role) {
                emit(barrier, &env.limits, refusal(to, people::Refusal::Standing));
                return;
            }
            emit(barrier, &env.limits, Delivery::EscalationReply { to, person, context });
        }
        Query::Decide { request, requester, person, role, project, task, revision, decision } => {
            let mut query = Query::Decide { request, requester, person, role, project, task, revision, decision };
            match context {
                Some(context) => {
                    if context.project != project || !visible(domain, &context, person, role) {
                        finish_refused(domain, waiter, request, people::Refusal::Standing);
                        return;
                    }
                    let pending = match context.escalation {
                        tasks::Escalation::Waiting { revision: current, .. } => current == revision,
                        tasks::Escalation::Unheld { .. }
                        | tasks::Escalation::Routing { .. }
                        | tasks::Escalation::Rejected { .. } => false,
                    };
                    if !pending {
                        archive(domain, env, barrier, waiter, query);
                        return;
                    }
                    if !standing(&context, person, role) {
                        finish_refused(domain, waiter, request, people::Refusal::Standing);
                        return;
                    }
                    let decision = match &query {
                        Query::Decide { decision, .. } => decision,
                        Query::Read { .. } => unreachable!("decision query"),
                    };
                    let semantic = match decision {
                        people::EscalationDecision::Release => {
                            if !covers(domain, &context, person, role)
                                || !release_allowed(domain, &context, person, role)
                            {
                                finish_refused(domain, waiter, request, people::Refusal::Authority);
                                return;
                            }
                            tasks::EscalationDecision::Release
                        }
                        people::EscalationDecision::Reject { reason } => {
                            tasks::EscalationDecision::Reject { reason: reason.clone() }
                        }
                        people::EscalationDecision::Pass => {
                            let Some(holder) = fallback(domain, project) else {
                                finish_refused(domain, waiter, request, people::Refusal::Authority);
                                return;
                            };
                            tasks::EscalationDecision::Pass { holder }
                        }
                    };
                    match &mut query {
                        Query::Decide { requester, .. } => *requester = context.requester,
                        Query::Read { .. } => unreachable!("deciding context"),
                    }
                    *domain.result_reads.get_mut(Id::from_token(waiter)).expect("reserved query") =
                        Some(Read::Escalation(query));
                    let entry = match semantic {
                        tasks::EscalationDecision::Pass { .. } => {
                            match crate::fresh(&mut domain.journal, Family::Message) {
                                Some(entry) => Some(entry),
                                None => {
                                    finish_refused(domain, waiter, request, people::Refusal::Limit);
                                    return;
                                }
                            }
                        }
                        tasks::EscalationDecision::Release | tasks::EscalationDecision::Reject { .. } => None,
                    };
                    domain.work.push(Work::Tasks(tasks::Event::DecideEscalation {
                        reply_to: ReplyTo::new(waiter),
                        task,
                        revision,
                        by: person,
                        entry,
                        decision: semantic,
                    }));
                }
                None => archive(domain, env, barrier, waiter, query),
            }
        }
    }
}

fn release_allowed(
    domain: &Domain,
    context: &tasks::EscalationContext,
    person: u64,
    role: Option<people::Role>,
) -> bool {
    let Some(role) = role else { return false };
    let pool = tasks::Funder::Pool { project: context.project, person, period: domain.config.period };
    let numbers = match domain.tasks.funding(pool) {
        Some(record) => record.numbers,
        None => tasks::Numbers { budget: 0, spent: 0, spent_below: 0, reserved: 0 },
    };
    let mut findings = skein_lib::Queue::with_capacity(
        authority::max_out(domain.config.authority.limits()).expect("checked authority output"),
    );
    authority::check_request(
        &domain.config.authority,
        &authority::PersonAsk {
            project: context.project,
            role: role_number(role),
            pool: super::authority_numbers(numbers),
            tasks_left: domain.limits.tasks.tree_tasks,
            request: authority::PersonRequest::Accept(authority::Action::Escalate { release: None }),
        },
        &mut findings,
    )
    .answer
        == authority::Answer::Allow
}

fn archive(domain: &mut Domain, env: &Env<Limits>, barrier: &mut Decision, waiter: Token, query: Query) {
    *domain.result_reads.get_mut(Id::from_token(waiter)).expect("reserved archive query") =
        Some(Read::Escalation(query));
    emit(barrier, &env.limits, Delivery::ReadEscalationDecision { waiter });
}

pub(super) fn completed(
    domain: &mut Domain,
    env: &Env<Limits>,
    barrier: &mut Decision,
    waiter: Token,
    task: u64,
    revision: u64,
    outcome: tasks::EscalationOutcome,
) {
    let Some(read) = super::take_read(domain, waiter) else { return };
    let query = match read {
        Read::Escalation(query) => query,
        Read::Result(_)
        | Read::Inbox(_)
        | Read::Proposal(_)
        | Read::Transcript { .. }
        | Read::Dependency(_)
        | Read::InputCheck(_) => {
            unreachable!("escalation query terminal")
        }
    };
    let (request, requester, person, project, expected, current, decision) = match query {
        Query::Decide { request, requester, person, project, task, revision, decision, .. } => {
            (request, requester, person, project, task, revision, decision)
        }
        Query::Read { .. } => unreachable!("semantic completion belongs to decision"),
    };
    assert!(task == expected && revision == current, "exact semantic decision correlation");
    domain.result_reads.retire(Id::from_token(waiter));
    let choice = match outcome {
        tasks::EscalationOutcome::Released => people::EscalationChoice::Released,
        tasks::EscalationOutcome::Rejected => people::EscalationChoice::Rejected,
        tasks::EscalationOutcome::Passed { .. } => people::EscalationChoice::Passed,
        tasks::EscalationOutcome::NoFurther => {
            decided(domain, request, people::Outcome::Refused(people::Refusal::NoFurther));
            return;
        }
        tasks::EscalationOutcome::Limit => {
            decided(domain, request, people::Outcome::Refused(people::Refusal::Limit));
            return;
        }
        tasks::EscalationOutcome::Stale => {
            decided(domain, request, people::Outcome::Refused(people::Refusal::Unknown));
            return;
        }
    };
    save(
        barrier,
        &env.limits,
        Write::Save(Record::EscalationDecision(EscalationDecisionRecord {
            project,
            requester,
            task,
            revision,
            by: person,
            decision,
        })),
    );
    decided(domain, request, people::Outcome::EscalationDecided { task, revision, by: person, choice });
}

pub(super) fn loaded(domain: &mut Domain, env: &Env<Limits>, waiter: Token, rows: Box<[Record]>) {
    let Some(read) = super::take_read(domain, waiter) else { return };
    let query = match read {
        Read::Escalation(query) => query,
        Read::Result(_)
        | Read::Inbox(_)
        | Read::Proposal(_)
        | Read::Transcript { .. }
        | Read::Dependency(_)
        | Read::InputCheck(_) => {
            unreachable!("escalation query terminal")
        }
    };
    let (request, person, project, task, revision) = match query {
        Query::Decide { request, person, project, task, revision, .. } => (request, person, project, task, revision),
        Query::Read { .. } => unreachable!("only stale decision queries load history"),
    };
    domain.result_reads.retire(Id::from_token(waiter));
    let mut outcome = people::Outcome::Refused(people::Refusal::Unknown);
    if rows.len() == 1 {
        for row in rows {
            match row {
                Record::EscalationDecision(row)
                    if row.task == task
                        && row.revision == revision
                        && row.task <= domain.journal.deployment().tasks
                        && row.by != 0
                        && row.by <= domain.journal.deployment().people =>
                {
                    let bounded = match &row.decision {
                        people::EscalationDecision::Release | people::EscalationDecision::Pass => true,
                        people::EscalationDecision::Reject { reason } => {
                            reason.len()
                                <= usize::try_from(
                                    env.limits
                                        .people
                                        .words
                                        .min(env.limits.tasks.result_bytes)
                                        .min(env.limits.journal.result_bytes)
                                        .min(env.limits.journal.transcript_bytes),
                                )
                                .expect("u32 fits usize")
                        }
                    };
                    let private = row.project == project
                        && row.requester != 0
                        && row.requester <= domain.journal.deployment().people
                        && (row.requester == person
                            || match fallback(domain, project) {
                                Some(tasks::EscalationHolder::Role { role: selected, .. }) => {
                                    match domain.people.role(person, project) {
                                        Some(role) => role_number(role) == selected,
                                        None => false,
                                    }
                                }
                                Some(tasks::EscalationHolder::Task(_) | tasks::EscalationHolder::Person(_)) | None => {
                                    false
                                }
                            });
                    if !private {
                        outcome = people::Outcome::Refused(people::Refusal::Standing);
                    } else if bounded {
                        let choice = match row.decision {
                            people::EscalationDecision::Release => people::EscalationChoice::Released,
                            people::EscalationDecision::Reject { .. } => people::EscalationChoice::Rejected,
                            people::EscalationDecision::Pass => people::EscalationChoice::Passed,
                        };
                        outcome = people::Outcome::EscalationDecided { task, revision, by: row.by, choice };
                    }
                }
                Record::Call(_)
                | Record::Deployment(_)
                | Record::Turn(_)
                | Record::RunProof(_)
                | Record::Terminal(_)
                | Record::Tasks(_)
                | Record::People(_)
                | Record::EscalationDecision(_)
                | Record::ProposalDecision(_) => {}
            }
        }
    }
    decided(domain, request, outcome);
}

pub(super) fn failed(domain: &mut Domain, waiter: Token) {
    let Some(read) = super::take_read(domain, waiter) else { return };
    domain.result_reads.retire(Id::from_token(waiter));
    match read {
        Read::Escalation(Query::Decide { request, .. }) => {
            decided(domain, request, people::Outcome::Refused(people::Refusal::Busy));
        }
        Read::Proposal(_)
        | Read::Result(_)
        | Read::Inbox(_)
        | Read::Transcript { .. }
        | Read::Dependency(_)
        | Read::InputCheck(_)
        | Read::Escalation(Query::Read { .. }) => {
            unreachable!("only decision history loads here")
        }
    }
}

fn finish_refused(domain: &mut Domain, waiter: Token, request: Token, why: people::Refusal) {
    domain.result_reads.retire(Id::from_token(waiter));
    decided(domain, request, people::Outcome::Refused(why));
}

fn decided(domain: &mut Domain, request: Token, outcome: people::Outcome) {
    domain.work.push(Work::People(people::Event::Decided { request, outcome }));
}

fn refusal(to: ReplyTo, why: people::Refusal) -> Delivery {
    Delivery::WebReply { to, sign_in: None, reply: people::Reply::Refused(why) }
}

fn refuse_direct(to: ReplyTo, why: people::Refusal, out: &mut skein_lib::Queue<Request>) {
    out.push(Request::Deliver(refusal(to, why)));
}
