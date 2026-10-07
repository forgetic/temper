//! Proposal authorization and nearest-holder routing (domain/tasks.md, section 8;
//! domain/authority.md, section 9). Tasks keeps durable actions and recipients;
//! root uses current policy, roles and funding. No authority or people policy is
//! copied into the task child.
use super::{
    CallKey, Decision, Dependency, Domain, Env, Family, Limits, PersonProposalRoute, ProposalChoice, ProposedAction,
    ReplyTo, RoutedCall, Token, Work, authority, authority_numbers, authority_value, current_proof, people, tasks,
};
use crate::{CallAnswer, ProposalDecisionRecord};
use alloc::boxed::Box;
use skein_lib::List;

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
        return person_refused(domain, request, people::Refusal::Busy);
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
    let mut answer = people::Outcome::Refused(people::Refusal::Unknown);
    if rows.len() == 1 {
        for row in rows {
            match row {
                super::Record::ProposalDecision(row)
                    if row.proposal == query.proposal
                        && row.project == query.project
                        && row.proposal <= domain.journal.deployment().messages
                        && row.by != 0
                        && row.by <= domain.journal.deployment().people
                        && match row.proposer {
                            tasks::Party::Person(number) | tasks::Party::Task(number) => number == query.proposer,
                            tasks::Party::Deployment { .. } => false,
                        } =>
                {
                    let allowed = query.person == row.by
                        || policy_standing(
                            domain,
                            query.person,
                            query.project,
                            match row.kind {
                                tasks::ProposalKind::Batch => authority::ProposalKind::Batch,
                                tasks::ProposalKind::Amend => authority::ProposalKind::Amend,
                                tasks::ProposalKind::Widen => authority::ProposalKind::Widen,
                                tasks::ProposalKind::Release => authority::ProposalKind::Escalation,
                            },
                        );
                    answer = if allowed {
                        people::Outcome::ProposalDecided {
                            proposer: query.proposer,
                            proposal: query.proposal,
                            by: row.by,
                            choice: row.choice,
                        }
                    } else {
                        people::Outcome::Refused(people::Refusal::Standing)
                    };
                }
                super::Record::ProposalDecision(_)
                | super::Record::Call(_)
                | super::Record::Deployment(_)
                | super::Record::Turn(_)
                | super::Record::RunProof(_)
                | super::Record::Terminal(_)
                | super::Record::Tasks(_)
                | super::Record::People(_)
                | super::Record::Forge { .. }
                | super::Record::EscalationDecision(_) => {}
            }
        }
    }
    domain.work.push(Work::People(people::Event::Decided { request: query.request, outcome: answer }));
}

pub(super) fn historical_failed(domain: &mut Domain, waiter: Token) {
    let Some(super::Read::Proposal(query)) = super::take_read(domain, waiter) else {
        unreachable!("proposal archive owns its read")
    };
    domain.result_reads.retire(super::Id::from_token(waiter));
    person_refused(domain, query.request, people::Refusal::Busy);
}

/// Extract one immutable person-facing final decision from the child's durable row.
pub(super) fn decision_record(row: &tasks::Stored) -> Option<ProposalDecisionRecord> {
    match row {
        tasks::Stored::PersonProposal(row) => {
            let (by, choice) = match &row.state {
                tasks::PersonProposalState::Accepted { by: tasks::Party::Person(by) } => {
                    (*by, people::ProposalChoice::Accepted)
                }
                tasks::PersonProposalState::Rejected { by: tasks::Party::Person(by), .. } => {
                    (*by, people::ProposalChoice::Rejected)
                }
                tasks::PersonProposalState::Pending { .. }
                | tasks::PersonProposalState::Accepted {
                    by: tasks::Party::Task(_) | tasks::Party::Deployment { .. },
                }
                | tasks::PersonProposalState::Rejected {
                    by: tasks::Party::Task(_) | tasks::Party::Deployment { .. },
                    ..
                } => return None,
            };
            Some(ProposalDecisionRecord {
                project: row.project,
                proposer: tasks::Party::Person(row.proposer),
                proposal: row.number,
                kind: tasks::ProposalKind::Batch,
                by,
                choice,
            })
        }
        tasks::Stored::History(history) => {
            let proposal = history.proposal.as_ref()?;
            let (by, choice) = match &proposal.state {
                tasks::ProposalState::Accepted { by: tasks::Party::Person(by) } => {
                    (*by, people::ProposalChoice::Accepted)
                }
                tasks::ProposalState::Rejected { by: tasks::Party::Person(by), .. } => {
                    (*by, people::ProposalChoice::Rejected)
                }
                tasks::ProposalState::Pending { .. }
                | tasks::ProposalState::Withdrawn
                | tasks::ProposalState::Accepted { by: tasks::Party::Task(_) | tasks::Party::Deployment { .. } }
                | tasks::ProposalState::Rejected {
                    by: tasks::Party::Task(_) | tasks::Party::Deployment { .. }, ..
                } => return None,
            };
            Some(ProposalDecisionRecord {
                project: proposal.project,
                proposer: tasks::Party::Task(proposal.proposer),
                proposal: proposal.number,
                kind: kind(&proposal.action).0,
                by,
                choice,
            })
        }
        tasks::Stored::Live(_) | tasks::Stored::Ended(_) | tasks::Stored::Ledger(_) => None,
    }
}

fn refused(
    domain: &mut Domain,
    env: &Env<Limits>,
    decision: &mut Decision,
    to: ReplyTo,
    key: CallKey,
    why: tasks::Refusal,
) {
    super::decide_call(
        domain,
        &env.limits,
        decision,
        to,
        key,
        CallAnswer::ProposalRefused(tasks::Problem { task: Some(key.task), why }),
    );
}

fn kind(action: &tasks::ProposalAction) -> (tasks::ProposalKind, authority::ProposalKind) {
    match action {
        tasks::ProposalAction::Batch(_) => (tasks::ProposalKind::Batch, authority::ProposalKind::Batch),
        tasks::ProposalAction::Amend { .. } => (tasks::ProposalKind::Amend, authority::ProposalKind::Amend),
        tasks::ProposalAction::Widen { .. } => (tasks::ProposalKind::Widen, authority::ProposalKind::Widen),
        tasks::ProposalAction::Release { .. } => (tasks::ProposalKind::Release, authority::ProposalKind::Escalation),
    }
}

fn action_for_check(action: &tasks::ProposalAction) -> Option<authority::Action> {
    match action {
        tasks::ProposalAction::Batch(batch) => {
            let mut members = List::with_capacity(u32::try_from(batch.len()).expect("bounded proposed batch"));
            for member in batch {
                let executor = match member.executor {
                    tasks::Executor::Agent { charter } => authority::Executor::Charter(charter),
                    tasks::Executor::Procedure { code, .. } => authority::Executor::Procedure(code),
                    tasks::Executor::Person(_) => return None,
                };
                members
                    .push(authority::Delegate { executor, authority: authority_value(&member.authority) })
                    .expect("bounded proposed batch");
            }
            Some(authority::Action::Batch(members.into_boxed()))
        }
        tasks::ProposalAction::Amend { amendment, .. } => Some(authority::Action::Amend(authority_value(
            amendment.authority.as_ref().expect("proposal amendment widens"),
        ))),
        tasks::ProposalAction::Widen { authority, .. } => Some(authority::Action::Widen(authority_value(authority))),
        tasks::ProposalAction::Release { .. } => Some(authority::Action::Escalate { release: None }),
    }
}

fn covers_task(domain: &Domain, needed: &authority::Authority, ancestor: u64, distance: u32, project: u32) -> bool {
    let Some(holder) = domain.tasks.delegation(ancestor) else { return false };
    holder.project == project
        && holder.deciding
        && authority::covers(
            &domain.config.authority,
            needed,
            &authority::Holder::Task {
                project,
                authority: authority_value(&holder.authority),
                numbers: authority_numbers(holder.numbers),
                tasks_left: holder.tasks_left,
            },
            distance,
        )
}

fn distance(domain: &Domain, proposer: u64, holder: u64) -> Option<u32> {
    let mut next = domain.tasks.delegation(proposer)?.requester;
    for depth in 1..=domain.limits.tasks.depth.saturating_add(1) {
        match next {
            tasks::Party::Task(task) if task == holder => return Some(depth),
            tasks::Party::Task(task) => next = domain.tasks.delegation(task)?.requester,
            tasks::Party::Person(_) | tasks::Party::Deployment { .. } => return None,
        }
    }
    None
}

fn covers_person(
    domain: &Domain,
    needed: &authority::Authority,
    person: u64,
    project: u32,
    kind: authority::ProposalKind,
) -> bool {
    let Some(role) = domain.people.role(person, project) else { return false };
    let funder = tasks::Funder::Pool { project, person, period: domain.config.period };
    let numbers = match domain.tasks.funding(funder) {
        Some(pool) => pool.numbers,
        None => tasks::Numbers { budget: domain.config.person_budget, spent: 0, spent_below: 0, reserved: 0 },
    };
    authority::covers(
        &domain.config.authority,
        needed,
        &authority::Holder::Person {
            project,
            role: super::escalation::role_number(role),
            proposal: kind,
            pool: authority_numbers(numbers),
            tasks_left: domain.limits.tasks.tree_tasks,
        },
        0,
    )
}

/// Select the nearest live covering ancestor, then the root requester, then
/// the policy's eligible people. `after` skips holders through one pass/stall.
pub(super) fn holder(
    domain: &Domain,
    proposer: u64,
    action: &tasks::ProposalAction,
    after: Option<tasks::ProposalHolder>,
) -> Option<tasks::ProposalHolder> {
    let context = domain.tasks.delegation(proposer)?;
    let checked = action_for_check(action)?;
    let needed = authority::needs(&checked)?;
    let (kind, policy_kind) = kind(action);
    let mut next = context.requester;
    let mut distance = 1_u32;
    let mut passed = after.is_none();
    for _ in 0..domain.limits.tasks.depth.saturating_add(1) {
        match next {
            tasks::Party::Task(task) => {
                let holder = tasks::ProposalHolder::Task(task);
                if passed && covers_task(domain, &needed, task, distance, context.project) {
                    return Some(holder);
                }
                if after == Some(holder) {
                    passed = true;
                }
                next = match domain.tasks.delegation(task) {
                    Some(context) => context.requester,
                    None => return Some(tasks::ProposalHolder::Policy { project: context.project, kind }),
                };
                distance = distance.checked_add(1)?;
            }
            tasks::Party::Person(person) => {
                let holder = tasks::ProposalHolder::Person(person);
                if passed && covers_person(domain, &needed, person, context.project, policy_kind) {
                    return Some(holder);
                }
                if after == Some(holder) {
                    // The next recipient is the final policy group.
                }
                return Some(tasks::ProposalHolder::Policy { project: context.project, kind });
            }
            tasks::Party::Deployment { .. } => {
                return Some(tasks::ProposalHolder::Policy { project: context.project, kind });
            }
        }
    }
    None
}

fn materialize(
    domain: &mut Domain,
    context: &tasks::DelegationContext,
    proposer: u64,
    batch: Box<[super::Delegate]>,
    limits: &tasks::Limits,
) -> Result<Box<[tasks::New]>, tasks::Refusal> {
    if batch.is_empty() || batch.len() > usize::try_from(limits.batch).expect("u32 fits usize") {
        return Err(tasks::Refusal::Batch);
    }
    let mut ids = List::with_capacity(limits.batch);
    for _ in &batch {
        let Some(number) = crate::fresh(&mut domain.journal, Family::Task) else { return Err(tasks::Refusal::Live) };
        ids.push(number).expect("bounded batch IDs");
    }
    let mut created = List::with_capacity(limits.batch);
    for (index, member) in batch.into_iter().enumerate() {
        let mut dependencies = List::with_capacity(limits.dependencies);
        for dependency in member.dependencies {
            let number = match dependency {
                Dependency::Batch(at) => *ids.get(at).ok_or(tasks::Refusal::Dependencies)?,
                Dependency::Existing(number) => number,
            };
            if dependencies.push(number).is_err() {
                return Err(tasks::Refusal::Dependencies);
            }
        }
        created
            .push(tasks::New {
                number: *ids.get(u32::try_from(index).expect("batch index")).expect("one ID per member"),
                project: context.project,
                executor: member.executor,
                spec: member.spec,
                contract: member.contract,
                numbers: tasks::Numbers {
                    budget: member.authority.budget.spend,
                    spent: 0,
                    spent_below: 0,
                    reserved: 0,
                },
                authority: member.authority,
                funder: tasks::Funder::Task(proposer),
                dependencies: dependencies.into_boxed(),
                wake: member.wake,
                recurring: None,
                tracked: None,
            })
            .expect("bounded proposed batch");
    }
    Ok(created.into_boxed())
}

#[expect(clippy::too_many_arguments, reason = "named proposal call owns its action, reason and call correlation")]
pub(super) fn propose_call(
    domain: &mut Domain,
    env: &Env<Limits>,
    decision: &mut Decision,
    to: ReplyTo,
    key: CallKey,
    action: ProposedAction,
    reason: Box<[u8]>,
    as_holder: bool,
) {
    if !current_proof(domain, key.task, key.attempt)
        || reason.len() > usize::try_from(env.limits.tasks.message_bytes).expect("u32 fits usize")
    {
        return refused(domain, env, decision, to, key, tasks::Refusal::Read);
    }
    let Some(context) = domain.tasks.delegation(key.task) else {
        return refused(domain, env, decision, to, key, tasks::Refusal::Unknown);
    };
    let action = match action {
        ProposedAction::Batch(batch) => match materialize(domain, &context, key.task, batch, &env.limits.tasks) {
            Ok(batch) => tasks::ProposalAction::Batch(batch),
            Err(why) => return refused(domain, env, decision, to, key, why),
        },
        ProposedAction::Amend { task, amendment } => {
            if amendment.authority.is_none() {
                return refused(domain, env, decision, to, key, tasks::Refusal::AuthorityShape);
            }
            tasks::ProposalAction::Amend { task, amendment }
        }
        ProposedAction::Widen { task, authority } => tasks::ProposalAction::Widen { task, authority },
        ProposedAction::Release { task } => tasks::ProposalAction::Release { task },
    };
    let Some(checked) = action_for_check(&action) else {
        return refused(domain, env, decision, to, key, tasks::Refusal::Executor);
    };
    let Some(needed) = authority::needs(&checked) else {
        return refused(domain, env, decision, to, key, tasks::Refusal::AuthorityShape);
    };
    let Some(policy) = domain.config.authority.policy(context.project) else {
        return refused(domain, env, decision, to, key, tasks::Refusal::Project);
    };
    let implies = &domain.config.authority.rules().implies;
    if !authority::at_most(&needed, &policy.ceiling, implies)
        || !authority::at_most(&needed, &domain.config.authority.rules().ceiling, implies)
    {
        return refused(domain, env, decision, to, key, tasks::Refusal::AuthorityShape);
    }
    let Some(holder) = holder(domain, key.task, &action, None) else {
        return refused(domain, env, decision, to, key, tasks::Refusal::Reference);
    };
    let Some(number) = crate::fresh(&mut domain.journal, Family::Message) else {
        return refused(domain, env, decision, to, key, tasks::Refusal::Busy);
    };
    let token = to.into_token();
    assert!(domain.pending_calls.insert(key, true).is_ok(), "proposal call room reserved");
    assert!(domain.routing_calls.insert(token, RoutedCall::Propose { key, proposal: number }) == Ok(None), "one route");
    domain.work.push(Work::Tasks(tasks::Event::Propose {
        reply_to: ReplyTo::new(token),
        proposal: tasks::Proposal {
            number,
            proposer: key.task,
            project: context.project,
            action,
            reason,
            as_holder,
            state: tasks::ProposalState::Pending { holder, since: env.wall },
        },
    }));
}

pub(super) fn withdraw_call(
    domain: &mut Domain,
    env: &Env<Limits>,
    decision: &mut Decision,
    to: ReplyTo,
    key: CallKey,
    proposal: u64,
) {
    if domain.tasks.proposal(key.task, proposal).is_none() {
        return refused(domain, env, decision, to, key, tasks::Refusal::Unknown);
    }
    let token = to.into_token();
    assert!(domain.pending_calls.insert(key, true).is_ok(), "proposal call room reserved");
    assert!(domain.routing_calls.insert(token, RoutedCall::Withdraw { key, proposal }) == Ok(None), "one route");
    domain.work.push(Work::Tasks(tasks::Event::WithdrawProposal {
        reply_to: ReplyTo::new(token),
        proposer: key.task,
        proposal,
    }));
}

#[expect(clippy::too_many_arguments, reason = "named holder decision carries proposal identity and choice")]
pub(super) fn decide_call(
    domain: &mut Domain,
    env: &Env<Limits>,
    decision: &mut Decision,
    to: ReplyTo,
    key: CallKey,
    proposer: u64,
    proposal: u64,
    choice: ProposalChoice,
) {
    let Some(pending) = domain.tasks.proposal(proposer, proposal) else {
        return refused(domain, env, decision, to, key, tasks::Refusal::Unknown);
    };
    let current = match pending.state {
        tasks::ProposalState::Pending { holder, .. } => holder,
        tasks::ProposalState::Accepted { .. }
        | tasks::ProposalState::Rejected { .. }
        | tasks::ProposalState::Withdrawn => {
            return refused(domain, env, decision, to, key, tasks::Refusal::State);
        }
    };
    if current != tasks::ProposalHolder::Task(key.task) {
        return refused(domain, env, decision, to, key, tasks::Refusal::Reference);
    }
    let next = match choice {
        ProposalChoice::Accept => {
            return accept(domain, env, decision, to, key, pending);
        }
        ProposalChoice::Reject { reason } => tasks::ProposalDecision::Reject { reason },
        ProposalChoice::Pass => {
            let Some(holder) = holder(domain, proposer, &pending.action, Some(current)) else {
                return refused(domain, env, decision, to, key, tasks::Refusal::Reference);
            };
            tasks::ProposalDecision::Pass { holder }
        }
    };
    let message = match next {
        tasks::ProposalDecision::Reject { .. } => match crate::fresh(&mut domain.journal, Family::Message) {
            Some(message) => Some(message),
            None => return refused(domain, env, decision, to, key, tasks::Refusal::Busy),
        },
        tasks::ProposalDecision::Pass { .. } => None,
        tasks::ProposalDecision::Accept => unreachable!("accept routed through action"),
    };
    let token = to.into_token();
    assert!(domain.pending_calls.insert(key, true).is_ok(), "proposal decision room reserved");
    assert!(domain.routing_calls.insert(token, RoutedCall::Decide { key, proposal }) == Ok(None), "one route");
    domain.work.push(Work::Tasks(tasks::Event::DecideProposal {
        reply_to: ReplyTo::new(token),
        proposer,
        proposal,
        message,
        by: tasks::Party::Task(key.task),
        decision: next,
    }));
}

fn accept(
    domain: &mut Domain,
    env: &Env<Limits>,
    decision: &mut Decision,
    to: ReplyTo,
    key: CallKey,
    proposal: tasks::Proposal,
) {
    let Some(action) = action_for_check(&proposal.action) else {
        return refused(domain, env, decision, to, key, tasks::Refusal::Executor);
    };
    let Some(needed) = authority::needs(&action) else {
        return refused(domain, env, decision, to, key, tasks::Refusal::AuthorityShape);
    };
    let Some(depth) = distance(domain, proposal.proposer, key.task) else {
        return refused(domain, env, decision, to, key, tasks::Refusal::Reference);
    };
    if !covers_task(domain, &needed, key.task, depth, proposal.project) {
        return refused(domain, env, decision, to, key, tasks::Refusal::Funding);
    }
    let Some(message) = crate::fresh(&mut domain.journal, Family::Message) else {
        return refused(domain, env, decision, to, key, tasks::Refusal::Busy);
    };
    let token = to.into_token();
    let event = match proposal.action {
        tasks::ProposalAction::Batch(mut batch) => {
            let creator =
                if proposal.as_holder { tasks::Party::Task(key.task) } else { tasks::Party::Task(proposal.proposer) };
            for member in &mut batch {
                member.funder = tasks::Funder::Task(key.task);
            }
            tasks::Event::Make { reply_to: ReplyTo::new(token), creator, batch }
        }
        tasks::ProposalAction::Amend { task, amendment } => {
            let Some(message) = crate::fresh(&mut domain.journal, Family::Message) else {
                return refused(domain, env, decision, ReplyTo::new(token), key, tasks::Refusal::Busy);
            };
            let Some(current) = domain.tasks.delegation(task) else {
                return refused(domain, env, decision, ReplyTo::new(token), key, tasks::Refusal::Unknown);
            };
            let after = authority_value(amendment.authority.as_ref().expect("proposal amendment authority"));
            let before = authority_value(&current.authority);
            let stop_run = !authority::at_most(&before, &after, &domain.config.authority.rules().implies);
            tasks::Event::Amend {
                reply_to: ReplyTo::new(token),
                by: tasks::Party::Task(key.task),
                task,
                message,
                stop_run,
                amendment,
            }
        }
        tasks::ProposalAction::Widen { task, authority } => {
            let Some(message) = crate::fresh(&mut domain.journal, Family::Message) else {
                return refused(domain, env, decision, ReplyTo::new(token), key, tasks::Refusal::Busy);
            };
            tasks::Event::Amend {
                reply_to: ReplyTo::new(token),
                by: tasks::Party::Task(key.task),
                task,
                message,
                stop_run: false,
                amendment: tasks::Amendment {
                    spec: None,
                    wake: None,
                    dependencies: None,
                    authority: Some(authority),
                    reason: proposal.reason,
                },
            }
        }
        tasks::ProposalAction::Release { task } => tasks::Event::Control {
            reply_to: ReplyTo::new(token),
            by: tasks::Party::Task(key.task),
            task,
            control: tasks::Control::Release,
        },
    };
    assert!(domain.pending_calls.insert(key, true).is_ok(), "proposal acceptance room reserved");
    assert!(
        domain.routing_calls.insert(
            token,
            RoutedCall::Accepting { key, proposer: proposal.proposer, proposal: proposal.number, message }
        ) == Ok(None),
        "one acceptance route"
    );
    domain.work.push(Work::Tasks(event));
}

fn person_refused(domain: &mut Domain, request: Token, why: people::Refusal) {
    domain.work.push(Work::People(people::Event::Decided { request, outcome: people::Outcome::Refused(why) }));
}

fn policy_standing(domain: &Domain, person: u64, project: u32, kind: authority::ProposalKind) -> bool {
    let Some(role) = domain.people.role(person, project) else { return false };
    let Some(policy) = domain.config.authority.role(project, super::escalation::role_number(role)) else {
        return false;
    };
    policy.decides.allows(kind)
}

#[expect(clippy::too_many_arguments, reason = "one keyed person route carries its authenticated holder and proposal")]
pub(super) fn person_decide(
    domain: &mut Domain,
    env: &Env<Limits>,
    _decision: &mut Decision,
    request: Token,
    person: u64,
    role: Option<people::Role>,
    project: u32,
    proposer: u64,
    number: u64,
    choice: people::ProposalDecision,
) {
    let Some(proposal) = domain.tasks.proposal(proposer, number) else {
        return person_refused(domain, request, people::Refusal::Ended);
    };
    if proposal.project != project {
        return person_refused(domain, request, people::Refusal::Standing);
    }
    let current = match proposal.state {
        tasks::ProposalState::Pending { holder, .. } => holder,
        tasks::ProposalState::Accepted { .. }
        | tasks::ProposalState::Rejected { .. }
        | tasks::ProposalState::Withdrawn => return person_refused(domain, request, people::Refusal::Ended),
    };
    let (_, proposal_kind) = kind(&proposal.action);
    let standing = match current {
        tasks::ProposalHolder::Person(holder) => holder == person,
        tasks::ProposalHolder::Policy { project: named, .. } => {
            named == project && policy_standing(domain, person, project, proposal_kind)
        }
        tasks::ProposalHolder::Task(_) => false,
    };
    if !standing || role.is_none() {
        return person_refused(domain, request, people::Refusal::Standing);
    }
    let next = match choice {
        people::ProposalDecision::Accept => {
            let Some(action) = action_for_check(&proposal.action) else {
                return person_refused(domain, request, people::Refusal::Authority);
            };
            if !covers_person(
                domain,
                &authority::needs(&action).expect("admitted proposal needs"),
                person,
                project,
                proposal_kind,
            ) {
                return person_refused(domain, request, people::Refusal::Authority);
            }
            return person_accept(domain, env, request, person, proposal);
        }
        people::ProposalDecision::Reject { reason } => tasks::ProposalDecision::Reject { reason },
        people::ProposalDecision::Pass => match current {
            tasks::ProposalHolder::Person(_) => tasks::ProposalDecision::Pass {
                holder: tasks::ProposalHolder::Policy { project, kind: kind(&proposal.action).0 },
            },
            tasks::ProposalHolder::Policy { .. } => {
                return person_refused(domain, request, people::Refusal::NoFurther);
            }
            tasks::ProposalHolder::Task(_) => unreachable!("person standing checked"),
        },
    };
    let message = match next {
        tasks::ProposalDecision::Reject { .. } => match crate::fresh(&mut domain.journal, Family::Message) {
            Some(message) => Some(message),
            None => return person_refused(domain, request, people::Refusal::Limit),
        },
        tasks::ProposalDecision::Pass { .. } => None,
        tasks::ProposalDecision::Accept => unreachable!("accept routed separately"),
    };
    assert!(
        domain
            .routing_people_proposals
            .insert(request, PersonProposalRoute::Deciding { request, proposer, proposal: number, by: person })
            == Ok(None),
        "one person proposal route"
    );
    domain.work.push(Work::PersonProposal(tasks::Event::DecideProposal {
        reply_to: ReplyTo::new(request),
        proposer,
        proposal: number,
        message,
        by: tasks::Party::Person(person),
        decision: next,
    }));
}

fn person_accept(domain: &mut Domain, _env: &Env<Limits>, request: Token, person: u64, proposal: tasks::Proposal) {
    let event = match proposal.action {
        tasks::ProposalAction::Batch(mut batch) => {
            let creator =
                if proposal.as_holder { tasks::Party::Person(person) } else { tasks::Party::Task(proposal.proposer) };
            super::goals::ensure_pool(domain, proposal.project, person);
            for member in &mut batch {
                member.funder = tasks::Funder::Pool { project: proposal.project, person, period: domain.config.period };
            }
            tasks::Event::Make { reply_to: ReplyTo::new(request), creator, batch }
        }
        tasks::ProposalAction::Amend { task, amendment } => {
            let Some(current) = domain.tasks.delegation(task) else {
                return person_refused(domain, request, people::Refusal::Ended);
            };
            let Some(amend_message) = crate::fresh(&mut domain.journal, Family::Message) else {
                return person_refused(domain, request, people::Refusal::Limit);
            };
            let after = authority_value(amendment.authority.as_ref().expect("admitted amendment authority"));
            let before = authority_value(&current.authority);
            let stop_run = !authority::at_most(&before, &after, &domain.config.authority.rules().implies);
            tasks::Event::Amend {
                reply_to: ReplyTo::new(request),
                by: tasks::Party::Person(person),
                task,
                message: amend_message,
                stop_run,
                amendment,
            }
        }
        tasks::ProposalAction::Widen { task, authority } => {
            let Some(amend_message) = crate::fresh(&mut domain.journal, Family::Message) else {
                return person_refused(domain, request, people::Refusal::Limit);
            };
            tasks::Event::Amend {
                reply_to: ReplyTo::new(request),
                by: tasks::Party::Person(person),
                task,
                message: amend_message,
                stop_run: false,
                amendment: tasks::Amendment {
                    spec: None,
                    wake: None,
                    dependencies: None,
                    authority: Some(authority),
                    reason: proposal.reason,
                },
            }
        }
        tasks::ProposalAction::Release { task } => tasks::Event::Control {
            reply_to: ReplyTo::new(request),
            by: tasks::Party::Person(person),
            task,
            control: tasks::Control::Release,
        },
    };
    // The final decision follows the accepted amendment's own message in the same task inbox.
    let Some(message) = crate::fresh(&mut domain.journal, Family::Message) else {
        return person_refused(domain, request, people::Refusal::Limit);
    };
    assert!(
        domain.routing_people_proposals.insert(
            request,
            PersonProposalRoute::Accepting {
                request,
                person,
                proposer: proposal.proposer,
                proposal: proposal.number,
                message,
            }
        ) == Ok(None),
        "one person acceptance route"
    );
    domain.work.push(Work::PersonProposal(event));
}
