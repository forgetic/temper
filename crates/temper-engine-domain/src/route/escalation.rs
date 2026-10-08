//! Root transport and store translation for held-chat reads and archives.

use super::{Decision, Delivery, Domain, Env, Id, Limits, Read, ReplyTo, Request, Token, Work, emit, people, tasks};
use crate::Record;
use alloc::boxed::Box;

#[derive(Debug)]
pub(crate) enum Query {
    Read { to: ReplyTo, person: u64, task: u64 },
    Historical { request: Token, person: u64, project: u32, task: u64, revision: u64 },
}

pub(crate) fn role_number(role: people::Role) -> u32 {
    role.number()
}

pub(crate) fn supported(domain: &Domain, task: &tasks::TaskRecord) -> bool {
    domain.core.escalation_supported(task)
}

pub(crate) fn read(
    domain: &mut Domain,
    env: &Env<Limits>,
    to: ReplyTo,
    sign_in: u64,
    task: u64,
    out: &mut skein_lib::Queue<Request>,
) {
    let Some(person) = domain.core.people.person(sign_in, env.now, env.wall) else {
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
            | Read::Notes { .. }
            | Read::Escalation(Query::Historical { .. }) => {
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
pub(crate) fn historical_begin(
    domain: &mut Domain,
    env: &Env<Limits>,
    barrier: &mut Decision,
    request: Token,
    person: u64,
    project: u32,
    task: u64,
    revision: u64,
) {
    let query = Query::Historical { request, person, project, task, revision };
    match domain.result_reads.insert(Some(Read::Escalation(query))) {
        Ok(waiter) => emit(barrier, &env.limits, Delivery::ReadEscalationDecision { waiter: waiter.token() }),
        Err(_) => domain.work.push(Work::Core(jig_core::Event::HistoricalEscalation {
            request,
            person,
            project,
            task,
            revision,
            row: None,
            busy: true,
        })),
    }
}

pub(crate) fn inspected(domain: &mut Domain, waiter: Token, context: Option<Box<tasks::EscalationContext>>) {
    let Some(Read::Escalation(Query::Read { to, person, task })) = super::take_read(domain, waiter) else {
        unreachable!("held-chat inspection belongs to an authenticated reader")
    };
    domain.result_reads.retire(Id::from_token(waiter));
    domain.work.push(Work::Core(jig_core::Event::EscalationRead { to, person, task, context }));
}

pub(crate) fn loaded(domain: &mut Domain, _env: &Env<Limits>, waiter: Token, rows: Box<[Record]>) {
    let Some(Read::Escalation(query)) = super::take_read(domain, waiter) else {
        unreachable!("escalation archive owns its read")
    };
    domain.result_reads.retire(Id::from_token(waiter));
    let (request, person, project, task, revision) = match query {
        Query::Historical { request, person, project, task, revision } => (request, person, project, task, revision),
        Query::Read { .. } => unreachable!("only historical decision queries load history"),
    };
    let row = if rows.len() == 1 {
        match rows.into_iter().next().expect("one loaded row") {
            Record::Core(jig_core::Record::Core(jig_core::CoreRecord::EscalationDecision(row))) => Some(row),
            Record::Core(
                jig_core::Record::Core(
                    jig_core::CoreRecord::Call(_)
                    | jig_core::CoreRecord::Deployment(_)
                    | jig_core::CoreRecord::Turn(_)
                    | jig_core::CoreRecord::RunProof(_)
                    | jig_core::CoreRecord::Terminal(_)
                    | jig_core::CoreRecord::Projection(_)
                    | jig_core::CoreRecord::ProposalDecision(_),
                )
                | jig_core::Record::Tasks(_)
                | jig_core::Record::People(_)
                | jig_core::Record::Notes(_),
            )
            | Record::Forge { .. } => None,
        }
    } else {
        None
    };
    domain.work.push(Work::Core(jig_core::Event::HistoricalEscalation {
        request,
        person,
        project,
        task,
        revision,
        row,
        busy: false,
    }));
}

pub(crate) fn failed(domain: &mut Domain, waiter: Token) {
    let Some(read) = super::take_read(domain, waiter) else { return };
    domain.result_reads.retire(Id::from_token(waiter));
    match read {
        Read::Escalation(Query::Historical { request, person, project, task, revision }) => {
            domain.work.push(Work::Core(jig_core::Event::HistoricalEscalation {
                request,
                person,
                project,
                task,
                revision,
                row: None,
                busy: true,
            }));
        }
        Read::Proposal(_)
        | Read::Result(_)
        | Read::Inbox(_)
        | Read::Transcript { .. }
        | Read::Dependency(_)
        | Read::InputCheck(_)
        | Read::Notes { .. }
        | Read::Escalation(Query::Read { .. }) => {
            unreachable!("only decision history loads here")
        }
    }
}

fn refusal(to: ReplyTo, why: people::Refusal) -> Delivery {
    Delivery::WebReply { to, sign_in: None, reply: people::Reply::Refused(why) }
}

fn refuse_direct(to: ReplyTo, why: people::Refusal, out: &mut skein_lib::Queue<Request>) {
    out.push(crate::boundary::delivery_output(refusal(to, why)));
}
