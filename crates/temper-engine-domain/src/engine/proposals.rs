//! Store translation for archived proposals and host-call translation for
//! named proposals. The core owns authorization and holder routing.
use super::{CallKey, Decision, Domain, Env, Limits, ProposedAction, ReplyTo, Token, Work, tasks};
use alloc::boxed::Box;

/// One bounded named historical lookup after a proposal left live memory.
#[derive(Debug)]
pub(super) struct Query {
    request: Token,
    person: u64,
    project: u32,
    proposer: u64,
    proposal: u64,
}

#[expect(clippy::too_many_arguments, reason = "one keyed proposal decision has one named archive identity")]
pub(super) fn historical_begin(
    domain: &mut Domain,
    env: &Env<Limits>,
    barrier: &mut Decision,
    request: Token,
    person: u64,
    project: u32,
    proposer: u64,
    proposal: u64,
) {
    let read = Query { request, person, project, proposer, proposal };
    let Ok(id) = domain.result_reads.insert(Some(super::Read::Proposal(read))) else {
        domain.work.push(Work::Core(jig_core::Event::HistoricalProposal {
            request,
            person,
            project,
            proposer,
            proposal,
            row: None,
            busy: true,
        }));
        return;
    };
    super::emit(
        barrier,
        &env.limits,
        super::Delivery::Load { waiter: id.token(), range: super::Range::ProposalDecision { proposal }, after: None },
    );
}

pub(super) fn historical_loaded(domain: &mut Domain, waiter: Token, rows: Box<[super::Record]>) {
    let Some(super::Read::Proposal(query)) = super::take_read(domain, waiter) else {
        unreachable!("proposal archive owns its read")
    };
    domain.result_reads.retire(super::Id::from_token(waiter));
    let row = if rows.len() == 1 {
        match rows.into_iter().next().expect("one loaded row") {
            super::Record::ProposalDecision(row) => Some(row),
            super::Record::Call(_)
            | super::Record::Deployment(_)
            | super::Record::Turn(_)
            | super::Record::RunProof(_)
            | super::Record::Terminal(_)
            | super::Record::Tasks(_)
            | super::Record::People(_)
            | super::Record::Notes(_)
            | super::Record::Forge { .. }
            | super::Record::Projection(_)
            | super::Record::EscalationDecision(_) => None,
        }
    } else {
        None
    };
    domain.work.push(Work::Core(jig_core::Event::HistoricalProposal {
        request: query.request,
        person: query.person,
        project: query.project,
        proposer: query.proposer,
        proposal: query.proposal,
        row,
        busy: false,
    }));
}

pub(super) fn historical_failed(domain: &mut Domain, waiter: Token) {
    let Some(super::Read::Proposal(query)) = super::take_read(domain, waiter) else {
        unreachable!("proposal archive owns its read")
    };
    domain.result_reads.retire(super::Id::from_token(waiter));
    domain.work.push(Work::Core(jig_core::Event::HistoricalProposal {
        request: query.request,
        person: query.person,
        project: query.project,
        proposer: query.proposer,
        proposal: query.proposal,
        row: None,
        busy: true,
    }));
}

pub(super) fn propose_call(
    domain: &mut Domain,
    to: ReplyTo,
    key: CallKey,
    action: ProposedAction,
    reason: Box<[u8]>,
    as_holder: bool,
) {
    let action = match action {
        ProposedAction::Effect { .. } => unreachable!("connector effect proposals retain their own payload"),
        ProposedAction::Batch(batch) => {
            domain.work.push(Work::Core(jig_core::Event::NamedAction {
                to,
                key,
                action: jig_core::NamedAction::ProposeBatch { batch, reason, as_holder },
            }));
            return;
        }
        ProposedAction::Amend { task, amendment } => tasks::ProposalAction::Amend { task, amendment },
        ProposedAction::Widen { task, authority } => tasks::ProposalAction::Widen { task, authority },
        ProposedAction::Release { task } => tasks::ProposalAction::Release { task },
    };
    domain.work.push(Work::Core(jig_core::Event::NamedAction {
        to,
        key,
        action: jig_core::NamedAction::Propose { action, reason, as_holder },
    }));
}
