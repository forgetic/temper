//! Person-origin tracked goals and their policy proposals (domain/people.md, 5.1;
//! domain/tasks.md, 8). Tasks owns the durable pending proposal and goal;
//! root checks role authority and authentic funding before either mutation.
use super::{
    Domain, Env, Family, GoalRoute, Limits, ReplyTo, Token, Work, authority, authority_numbers, people, tasks,
};
use alloc::boxed::Box;
use skein_lib::Queue;

fn refuse(domain: &mut Domain, request: Token, why: people::Refusal) {
    domain.work.push(Work::People(people::Event::Decided { request, outcome: people::Outcome::Refused(why) }));
}

fn pool(domain: &Domain, project: u32, person: u64) -> (tasks::Funder, tasks::Numbers) {
    let source = tasks::Funder::Pool { project, person, period: domain.config.period };
    let numbers = match domain.tasks.funding(source) {
        Some(row) => row.numbers,
        None => tasks::Numbers { budget: domain.config.person_budget, spent: 0, spent_below: 0, reserved: 0 },
    };
    (source, numbers)
}

pub(super) fn ensure_pool(domain: &mut Domain, project: u32, person: u64) {
    let period = tasks::Funder::Period { project, period: domain.config.period };
    if domain.tasks.funding(period).is_none() {
        domain.work.push(Work::Tasks(tasks::Event::OpenPeriod {
            reply_to: super::internal(0),
            project,
            period: domain.config.period,
            budget: domain.config.period_budget,
        }));
    }
    let (source, _) = pool(domain, project, person);
    if domain.tasks.funding(source).is_none() {
        domain.work.push(Work::Tasks(tasks::Event::CarvePool {
            reply_to: super::internal(0),
            project,
            person,
            period: domain.config.period,
            budget: domain.config.person_budget,
        }));
    }
}

/// Admit one keyed goal, creating it now only when the member can fund it.
#[expect(clippy::too_many_arguments, reason = "one typed goal request carries its full specification")]
pub(super) fn start(
    domain: &mut Domain,
    env: &Env<Limits>,
    request: Token,
    person: u64,
    project: u32,
    role: people::Role,
    spec: Box<[u8]>,
    charter: u32,
    budget: u64,
    priority: u32,
) {
    let Some(policy) = domain.config.authority.policy(project) else {
        return refuse(domain, request, people::Refusal::Unknown);
    };
    let role_number = super::escalation::role_number(role);
    let Some(role_policy) = domain.config.authority.role(project, role_number) else {
        return refuse(domain, request, people::Refusal::Role);
    };
    if !role_policy.requests.allows(authority::RequestKind::Create) {
        return refuse(domain, request, people::Refusal::Authority);
    }
    let mut given = domain.config.chat_authority.clone();
    given.budget.spend = budget;
    if charter != domain.config.charter
        || budget > policy.period_spend
        || budget > domain.config.authority.rules().period_spend
        || !authority::at_most(&given, &policy.ceiling, &domain.config.authority.rules().implies)
    {
        return refuse(domain, request, people::Refusal::Authority);
    }
    let (source, numbers) = pool(domain, project, person);
    let mut findings =
        Queue::with_capacity(authority::max_out(domain.config.authority.limits()).expect("authority room"));
    let checked = authority::check_request(
        &domain.config.authority,
        &authority::PersonAsk {
            project,
            role: role_number,
            pool: authority_numbers(numbers),
            tasks_left: env.limits.tasks.tree_tasks,
            request: authority::PersonRequest::Create(Box::new([authority::Delegate {
                executor: authority::Executor::Charter(charter),
                authority: given.clone(),
                symbolic: Box::new([]),
            }])),
        },
        &mut findings,
    );
    let beyond = budget > role_policy.authority.budget.spend
        || budget
            > numbers
                .budget
                .saturating_sub(numbers.spent.saturating_add(numbers.spent_below).saturating_add(numbers.reserved));
    if checked.answer != authority::Answer::Allow && !beyond {
        return refuse(domain, request, people::Refusal::Authority);
    }
    let Some(task) = crate::fresh(&mut domain.counters, Family::Task) else {
        return refuse(domain, request, people::Refusal::Limit);
    };
    let new = tasks::New {
        number: task,
        project,
        executor: tasks::Executor::Agent { charter },
        spec: tasks::Spec { words: spec, parameters: Box::new([]), inputs: Box::new([]) },
        contract: tasks::Contract::Report { words: env.limits.tasks.result_bytes },
        authority: super::task_authority(&given),
        numbers: tasks::Numbers { budget, spent: 0, spent_below: 0, reserved: 0 },
        funder: source,
        dependencies: Box::new([]),
        wake: tasks::WakePolicy::DEFAULT,
        recurring: None,
        tracked: Some(priority),
    };
    if !tasks::valid_spec(&env.limits.tasks, &new.spec) || !tasks::valid_authority(&env.limits.tasks, &new.authority) {
        return refuse(domain, request, people::Refusal::Limit);
    }
    if checked.answer == authority::Answer::Allow {
        ensure_pool(domain, project, person);
        assert!(domain.made.insert(request, (task, true)) == Ok(None), "one goal make flight");
        domain.work.push(Work::Tasks(tasks::Event::Make {
            reply_to: ReplyTo::new(request),
            creator: tasks::Party::Person(person),
            batch: Box::new([new]),
        }));
    } else {
        let Some(proposal) = crate::fresh(&mut domain.counters, Family::Message) else {
            return refuse(domain, request, people::Refusal::Limit);
        };
        assert!(
            domain.goal_routes.insert(request, GoalRoute::Proposing { proposal }) == Ok(None),
            "one goal proposal flight"
        );
        domain.work.push(Work::Tasks(tasks::Event::ProposePerson {
            reply_to: ReplyTo::new(request),
            proposal: tasks::PersonProposal {
                number: proposal,
                proposer: person,
                project,
                goal: new,
                state: tasks::PersonProposalState::Pending { since: env.wall },
            },
        }));
    }
}

/// Current policy-holder decision on a person's pending goal.
#[expect(clippy::too_many_arguments, reason = "one exact authenticated proposal decision")]
pub(super) fn decide(
    domain: &mut Domain,
    env: &Env<Limits>,
    request: Token,
    person: u64,
    role: Option<people::Role>,
    project: u32,
    proposer: u64,
    number: u64,
    choice: people::ProposalDecision,
) {
    let Some(proposal) = domain.tasks.person_proposal(proposer, number) else {
        return refuse(domain, request, people::Refusal::Ended);
    };
    if proposal.project != project || role.is_none() {
        return refuse(domain, request, people::Refusal::Standing);
    }
    let Some(role_policy) =
        domain.config.authority.role(project, super::escalation::role_number(role.expect("checked role")))
    else {
        return refuse(domain, request, people::Refusal::Standing);
    };
    if !role_policy.decides.allows(authority::ProposalKind::Batch) {
        return refuse(domain, request, people::Refusal::Standing);
    }
    let event = match choice {
        people::ProposalDecision::Accept => {
            let (source, numbers) = pool(domain, project, person);
            let mut findings =
                Queue::with_capacity(authority::max_out(domain.config.authority.limits()).expect("authority room"));
            let checked = authority::check_request(
                &domain.config.authority,
                &authority::PersonAsk {
                    project,
                    role: super::escalation::role_number(role.expect("checked role")),
                    pool: authority_numbers(numbers),
                    tasks_left: env.limits.tasks.tree_tasks,
                    request: authority::PersonRequest::Accept(authority::Action::Batch(Box::new([
                        authority::Delegate {
                            executor: authority::Executor::Charter(match proposal.goal.executor {
                                tasks::Executor::Agent { charter } => charter,
                                tasks::Executor::Procedure { .. } | tasks::Executor::Person(_) => {
                                    unreachable!("goal is agent")
                                }
                            }),
                            authority: super::authority_value(&proposal.goal.authority),
                            symbolic: Box::new([]),
                        },
                    ]))),
                },
                &mut findings,
            );
            if checked.answer != authority::Answer::Allow {
                return refuse(domain, request, people::Refusal::Authority);
            }
            ensure_pool(domain, project, person);
            let mut goal = proposal.goal.clone();
            goal.funder = source;
            assert!(
                domain.goal_routes.insert(
                    request,
                    GoalRoute::Accepting { proposer, proposal: number, by: person, task: goal.number }
                ) == Ok(None),
                "one goal acceptance"
            );
            tasks::Event::Make {
                reply_to: ReplyTo::new(request),
                creator: tasks::Party::Person(proposer),
                batch: Box::new([goal]),
            }
        }
        people::ProposalDecision::Reject { reason } => {
            let Some(message) = crate::fresh(&mut domain.counters, Family::Message) else {
                return refuse(domain, request, people::Refusal::Limit);
            };
            assert!(
                domain.goal_routes.insert(request, GoalRoute::Deciding { proposer, proposal: number, by: person })
                    == Ok(None),
                "one goal decision"
            );
            tasks::Event::DecidePersonProposal {
                reply_to: ReplyTo::new(request),
                proposer,
                proposal: number,
                by: tasks::Party::Person(person),
                message: Some(message),
                decision: tasks::ProposalDecision::Reject { reason },
            }
        }
        people::ProposalDecision::Pass => return refuse(domain, request, people::Refusal::NoFurther),
    };
    domain.work.push(Work::Tasks(event));
}
