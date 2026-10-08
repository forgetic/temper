//! Serialized authenticated role administration and held-recipient preflight.
//! Candidate rosters and waiting contexts are temporary root-owned snapshots;
//! all membership, semantic rerouting and keyed completion share one decision

//! Owner permission checks also admit current policy and pool changes.

use super::{Decision, Domain, Env, Limits, ReplyTo, Token, Work, escalation, people, proposals, save, tasks};
use crate::{Record, Write};
use alloc::boxed::Box;
use skein_lib::Queue;

pub(super) fn allowed(domain: &Domain, person: u64, project: u32) -> Result<(), people::Refusal> {
    domain.core.role_allowed(person, project, domain.limits.tasks.tree_tasks)
}

fn inspected(
    domain: &mut Domain,
    env: &Env<Limits>,
    request: Token,
    project: u32,
) -> Result<Box<[tasks::EscalationContext]>, people::Refusal> {
    let mut output = Queue::with_capacity(tasks::max_out(&env.limits.tasks));
    tasks::step(
        &mut domain.core.tasks,
        &super::environment_tasks(env),
        tasks::Event::InspectEscalations { reply_to: ReplyTo::new(request), project },
        &mut output,
    );
    let mut terminal = None;
    for _ in 0..output.len() {
        match output.pop().expect("inspection output count") {
            tasks::Request::EscalationsInspected { reply_to, result } => {
                assert!(reply_to.into_token() == request && terminal.is_none(), "one correlated inspection");
                terminal = Some(result);
            }
            tasks::Request::Taken { .. }
            | tasks::Request::Waiting { .. }
            | tasks::Request::EscalationsRechecked { .. }
            | tasks::Request::EscalationNeeded { .. }
            | tasks::Request::EscalationInspected { .. }
            | tasks::Request::EscalationDecided { .. }
            | tasks::Request::ProposalDecided { .. }
            | tasks::Request::ProposalRerouteNeeded { .. }
            | tasks::Request::ProposalStalled { .. }
            | tasks::Request::EscalationStalled { .. }
            | tasks::Request::Made { .. }
            | tasks::Request::Refused { .. }
            | tasks::Request::Done { .. }
            | tasks::Request::Acknowledged { .. }
            | tasks::Request::TurnAcknowledged { .. }
            | tasks::Request::Activate { .. }
            | tasks::Request::Stop { .. }
            | tasks::Request::Adopt { .. }
            | tasks::Request::Close { .. }
            | tasks::Request::Release { .. }
            | tasks::Request::Ended { .. }
            | tasks::Request::Save { .. }
            | tasks::Request::Erase { .. }
            | tasks::Request::RestoreRefused { .. }
            | tasks::Request::Sent { .. }
            | tasks::Request::Relay { .. }
            | tasks::Request::Notify { .. }
            | tasks::Request::Timer { .. }
            | tasks::Request::EndTopic { .. }
            | tasks::Request::RecurringDue { .. }
            | tasks::Request::WriterWaiting { .. }
            | tasks::Request::PersonProposed { .. }
            | tasks::Request::PersonProposalDecided { .. } => unreachable!("inspection is read-only"),
        }
    }
    match terminal.expect("inspection has a terminal") {
        Ok(contexts) => Ok(contexts),
        Err(problem) => {
            assert!(problem == tasks::Refusal::NotReady, "inspection refusal");
            Err(people::Refusal::NotReady)
        }
    }
}

fn preflight(
    domain: &Domain,
    contexts: &[tasks::EscalationContext],
    holdings: &[people::Holding],
) -> Result<u32, people::Refusal> {
    domain.core.roles_preflight(&super::core_limits(&domain.limits), contexts, holdings)
}

/// Consume one actual people route, preflight bounded Waiting contexts and
/// commit membership/rerouting/keyed completion together.
pub(super) fn begin(
    domain: &mut Domain,
    env: &Env<Limits>,
    decision: &mut Decision,
    request: Token,
    person: u64,
    project: u32,
    holdings: Box<[people::Holding]>,
) {
    if let Err(problem) = allowed(domain, person, project) {
        refused(domain, request, problem);
        return;
    }
    for holding in &holdings {
        match holding.role {
            people::Role::Policy { role } => {
                if domain.core.authority.role(project, role).is_none() {
                    refused(domain, request, people::Refusal::Unknown);
                    return;
                }
            }
            people::Role::Owner | people::Role::Maintainer | people::Role::Member | people::Role::Observer => {}
        }
    }
    let contexts = match inspected(domain, env, request, project) {
        Ok(contexts) => contexts,
        Err(problem) => {
            refused(domain, request, problem);
            return;
        }
    };
    let changed = match preflight(domain, &contexts, &holdings) {
        Ok(changed) => changed,
        Err(problem) => {
            refused(domain, request, problem);
            return;
        }
    };
    let handoffs = u32::try_from(contexts.len())
        .expect("bounded Waiting count")
        .checked_add(1)
        .expect("validated recheck and keyed completion room");
    // The original pending ask stays in people; root drops both snapshots before
    // application and the subsequent fresh semantic recheck. No IO or outside
    // mutation can intervene between these stages.
    drop(contexts);
    drop(holdings);
    if domain.work.room() < handoffs.checked_add(env.limits.tasks.tasks).expect("validated recheck bound")
        || !decision.room_for(changed.checked_add(2).expect("validated cohort bound"), env.limits.people.waiters)
    {
        refused(domain, request, people::Refusal::Busy);
        return;
    }
    if let Err(problem) = allowed(domain, person, project) {
        refused(domain, request, problem);
        return;
    }
    let mut output = Queue::with_capacity(people::max_out(&env.limits.people));
    people::step(
        &mut domain.core.people,
        &super::environment_people(env),
        people::Event::ApplyRoles { reply_to: ReplyTo::new(request), request },
        &mut output,
    );
    let mut terminal = None;
    for _ in 0..output.len() {
        match output.pop().expect("application output count") {
            people::Request::Save { record } => save(decision, &env.limits, Write::Save(Record::People(record))),
            people::Request::RolesApplied { reply_to, request: actual, result } => {
                assert!(
                    reply_to.into_token() == request && actual == request && terminal.is_none(),
                    "one correlated application"
                );
                terminal = Some(result);
            }
            people::Request::Erase { .. }
            | people::Request::Route { .. }
            | people::Request::Reply { .. }
            | people::Request::RolesRefused { .. }
            | people::Request::ServiceMade { .. }
            | people::Request::RestoreRefused { .. } => unreachable!("role application has one save and terminal"),
        }
    }
    if let Err(problem) = terminal.expect("application has a terminal") {
        refused(domain, request, problem);
        return;
    }
    recheck(domain, env, request, project);
    proposals::recheck_project(domain, project);
    domain.work.push(Work::People(people::Event::Decided { request, outcome: people::Outcome::RolesSet { project } }));
}

fn recheck(domain: &mut Domain, env: &Env<Limits>, request: Token, project: u32) {
    let mut output = Queue::with_capacity(tasks::max_out(&env.limits.tasks));
    tasks::step(
        &mut domain.core.tasks,
        &super::environment_tasks(env),
        tasks::Event::RecheckEscalations { reply_to: ReplyTo::new(request), project },
        &mut output,
    );
    let mut completed = false;
    for _ in 0..output.len() {
        match output.pop().expect("recheck output count") {
            tasks::Request::EscalationNeeded { context } => escalation::needed(domain, context),
            tasks::Request::EscalationsRechecked { reply_to, result } => {
                assert!(
                    reply_to.into_token() == request && !completed && result.is_ok(),
                    "one preflighted recheck terminal"
                );
                completed = true;
            }
            tasks::Request::Taken { .. }
            | tasks::Request::Waiting { .. }
            | tasks::Request::EscalationsInspected { .. }
            | tasks::Request::EscalationInspected { .. }
            | tasks::Request::EscalationDecided { .. }
            | tasks::Request::ProposalDecided { .. }
            | tasks::Request::ProposalRerouteNeeded { .. }
            | tasks::Request::ProposalStalled { .. }
            | tasks::Request::EscalationStalled { .. }
            | tasks::Request::Made { .. }
            | tasks::Request::Refused { .. }
            | tasks::Request::Done { .. }
            | tasks::Request::Acknowledged { .. }
            | tasks::Request::TurnAcknowledged { .. }
            | tasks::Request::Activate { .. }
            | tasks::Request::Stop { .. }
            | tasks::Request::Adopt { .. }
            | tasks::Request::Close { .. }
            | tasks::Request::Release { .. }
            | tasks::Request::Ended { .. }
            | tasks::Request::Save { .. }
            | tasks::Request::Erase { .. }
            | tasks::Request::RestoreRefused { .. }
            | tasks::Request::Sent { .. }
            | tasks::Request::Relay { .. }
            | tasks::Request::PersonProposed { .. }
            | tasks::Request::PersonProposalDecided { .. }
            | tasks::Request::Notify { .. }
            | tasks::Request::Timer { .. }
            | tasks::Request::EndTopic { .. }
            | tasks::Request::RecurringDue { .. }
            | tasks::Request::WriterWaiting { .. } => {
                unreachable!("recheck emits waiting contexts and terminal")
            }
        }
    }
    assert!(completed, "recheck has a terminal");
}

fn refused(domain: &mut Domain, request: Token, problem: people::Refusal) {
    domain.work.push(Work::People(people::Event::Decided { request, outcome: people::Outcome::Refused(problem) }));
}
