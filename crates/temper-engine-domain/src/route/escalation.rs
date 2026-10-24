//! Root store translation for held-chat decisions that left live memory.
//! Reading a held chat is the core's (jig's `client/domain/contract.md`, 6).

use super::{Decision, Delivery, Domain, Env, Id, Limits, Read, Token, Work, emit, people, tasks};
use crate::Record;
use alloc::boxed::Box;

/// One historical held-chat decision being loaded for its keyed request.
#[derive(Debug)]
pub(crate) struct Query {
    pub(crate) request: Token,
    pub(crate) person: u64,
    pub(crate) project: u32,
    pub(crate) task: u64,
    pub(crate) revision: u64,
}

pub(crate) fn role_number(role: people::Role) -> u32 {
    role.number()
}

pub(crate) fn supported(domain: &Domain, task: &tasks::TaskRecord) -> bool {
    domain.core.escalation_supported(task)
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
    let query = Query { request, person, project, task, revision };
    match domain.archive_reads.insert(Some(Read::Escalation(query))) {
        Ok(waiter) => emit(barrier, &env.limits, Delivery::ReadEscalationDecision { waiter: waiter.token() }),
        Err(_) => domain.work.push(Work::Core(jig_domain_core::Event::HistoricalEscalation {
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

pub(crate) fn loaded(domain: &mut Domain, _env: &Env<Limits>, waiter: Token, rows: Box<[Record]>) {
    let Some(read) = super::take_read(domain, waiter) else { unreachable!("escalation archive owns its read") };
    domain.archive_reads.retire(Id::from_token(waiter));
    let Query { request, person, project, task, revision } = query(read);
    let row = if rows.len() == 1 {
        match rows.into_iter().next().expect("one loaded row") {
            Record::Core(jig_domain_core::Record::Core(jig_domain_core::CoreRecord::EscalationDecision(row))) => {
                Some(row)
            }
            Record::Core(
                jig_domain_core::Record::Core(
                    jig_domain_core::CoreRecord::Settings(_)
                    | jig_domain_core::CoreRecord::Call(_)
                    | jig_domain_core::CoreRecord::Deployment(_)
                    | jig_domain_core::CoreRecord::Turn(_)
                    | jig_domain_core::CoreRecord::RunProof(_)
                    | jig_domain_core::CoreRecord::Terminal(_)
                    | jig_domain_core::CoreRecord::Projection(_)
                    | jig_domain_core::CoreRecord::ProposalDecision(_),
                )
                | jig_domain_core::Record::Tasks(_)
                | jig_domain_core::Record::People(_)
                | jig_domain_core::Record::Accounts(_)
                | jig_domain_core::Record::Notes(_),
            )
            | Record::Forge { .. } => None,
        }
    } else {
        None
    };
    domain.work.push(Work::Core(jig_domain_core::Event::HistoricalEscalation {
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
    domain.archive_reads.retire(Id::from_token(waiter));
    let Query { request, person, project, task, revision } = query(read);
    domain.work.push(Work::Core(jig_domain_core::Event::HistoricalEscalation {
        request,
        person,
        project,
        task,
        revision,
        row: None,
        busy: true,
    }));
}

fn query(read: Read) -> Query {
    match read {
        Read::Escalation(query) => query,
        Read::Proposal(_)
        | Read::TaskProposal { .. }
        | Read::Ended(_)
        | Read::Transcript { .. }
        | Read::InputCheck(_)
        | Read::Notes { .. } => unreachable!("only decision history loads here"),
    }
}
