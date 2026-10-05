//! Actual held-chat reads, routing and authenticated decisions. Root owns
//! transport/query slots and immutable decision evidence; tasks owns the held
//! lifecycle (domain/engine.md, 7.7; domain/tasks.md, 15).

use super::{
    Decision, Delivery, Domain, Env, Id, Limits, Read, ReplyTo, Request, Token, Work, authority, emit, people, save,
    tasks,
};
use crate::{EscalationDecisionRecord, Record, Write};
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
        tasks::Escalation::Waiting { holder: tasks::EscalationHolder::Role { .. }, .. } => {
            // An old selector can be rechecked, but its project must remain the chat's.
            match fallback {
                tasks::EscalationHolder::Role { project, .. } => project == task.project,
                tasks::EscalationHolder::Person(_) => false,
            }
        }
        tasks::Escalation::Rejected { by, .. } => *by <= domain.journal.deployment().people,
        tasks::Escalation::Unheld { .. } | tasks::Escalation::Routing { .. } | tasks::Escalation::Waiting { .. } => {
            true
        }
    }
}

pub(super) fn needed(domain: &mut Domain, context: Box<tasks::EscalationContext>) {
    let holder = if covers(domain, &context, context.requester, domain.people.role(context.requester, context.project))
    {
        tasks::EscalationHolder::Person(context.requester)
    } else {
        let Some(holder) = fallback(domain, context.project) else {
            domain.startup = super::Startup::Failed;
            return;
        };
        holder
    };
    // A final role never moves down to a requester on recheck/restart.
    let holder = match context.escalation {
        tasks::Escalation::Waiting { holder: tasks::EscalationHolder::Role { .. }, .. } => {
            fallback(domain, context.project).expect("current root requires fallback")
        }
        tasks::Escalation::Unheld { .. }
        | tasks::Escalation::Routing { .. }
        | tasks::Escalation::Waiting { .. }
        | tasks::Escalation::Rejected { .. } => holder,
    };
    match context.escalation {
        tasks::Escalation::Waiting { revision, holder: old } if old != holder && revision == u64::MAX => {
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
    }));
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
            Read::Result(_) | Read::Escalation(Query::Decide { .. }) => unreachable!("inserted named read"),
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
            domain.work.push(Work::Tasks(tasks::Event::InspectEscalation { reply_to: ReplyTo::new(id.token()), task }))
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
        Some(tasks::EscalationHolder::Person(_)) | None => false,
    }
}

fn standing(context: &tasks::EscalationContext, person: u64, role: Option<people::Role>) -> bool {
    match &context.escalation {
        tasks::Escalation::Waiting { holder, .. } => match holder {
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
        Read::Result(_) => unreachable!("escalation query terminal"),
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
                    domain.work.push(Work::Tasks(tasks::Event::DecideEscalation {
                        reply_to: ReplyTo::new(waiter),
                        task,
                        revision,
                        by: person,
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
        Read::Result(_) => unreachable!("escalation query terminal"),
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
        tasks::EscalationOutcome::NeedsAmend => {
            decided(domain, request, people::Outcome::Refused(people::Refusal::NeedsAmend));
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
        Read::Result(_) => unreachable!("escalation query terminal"),
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
                                Some(tasks::EscalationHolder::Person(_)) | None => false,
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
                Record::Deployment(_)
                | Record::Turn(_)
                | Record::RunProof(_)
                | Record::Terminal(_)
                | Record::Tasks(_)
                | Record::People(_)
                | Record::EscalationDecision(_) => {}
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
            decided(domain, request, people::Outcome::Refused(people::Refusal::Busy))
        }
        Read::Result(_) | Read::Escalation(Query::Read { .. }) => unreachable!("only decision history loads here"),
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
