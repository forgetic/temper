//! Person-origin tracked goals and their policy proposals (domain/people.md, 5.1;
//! domain/tasks.md, 8). Tasks owns the durable pending proposal and goal;
//! root checks role authority and authentic funding before either mutation.
use super::{Domain, Env, Family, GoalRoute, Limits, ReplyTo, Token, Work, forge_route, people, tasks};
use alloc::boxed::Box;

fn refuse(domain: &mut Domain, request: Token, why: people::Refusal) {
    domain.work.push(Work::People(people::Event::Decided { request, outcome: people::Outcome::Refused(why) }));
}

pub(super) fn ensure_pool(domain: &mut Domain, project: u32, person: u64) {
    let (period, pool) = domain.core.goal_open_pool(project, person);
    if let Some(event) = period {
        domain.work.push(Work::Tasks(event));
    }
    if let Some(event) = pool {
        domain.work.push(Work::Tasks(event));
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
    let admitted = match domain.core.goal_start(project, person, role, charter, budget, env.limits.tasks.tree_tasks) {
        Ok(admitted) => admitted,
        Err(why) => return refuse(domain, request, why),
    };
    let source = admitted.source;
    let given = admitted.authority;
    let Some(task) = crate::fresh(&mut domain.core.counters, Family::Task) else {
        return refuse(domain, request, people::Refusal::Limit);
    };
    let spec = tasks::Spec { words: spec, parameters: Box::new([]), inputs: Box::new([]) };
    let Some(holdings) =
        forge_route::task_holdings(domain, env, project, task, task, tasks::Executor::Agent { charter }, &spec, None)
    else {
        return refuse(domain, request, people::Refusal::Limit);
    };
    let new = tasks::New {
        number: task,
        project,
        executor: tasks::Executor::Agent { charter },
        spec,
        contract: tasks::Contract::Report { words: env.limits.tasks.result_bytes },
        authority: super::task_authority(&given),
        numbers: tasks::Numbers { budget, spent: 0, spent_below: 0, reserved: 0 },
        funder: source,
        dependencies: Box::new([]),
        holdings,
        wake: tasks::WakePolicy::DEFAULT,
        recurring: None,
        tracked: Some(priority),
    };
    if !tasks::valid_spec(&env.limits.tasks, &new.spec) || !tasks::valid_authority(&env.limits.tasks, &new.authority) {
        return refuse(domain, request, people::Refusal::Limit);
    }
    if admitted.direct {
        ensure_pool(domain, project, person);
        assert!(domain.core.made.insert(request, (task, true)) == Ok(None), "one goal make flight");
        domain.work.push(Work::Tasks(tasks::Event::Make {
            reply_to: ReplyTo::new(request),
            creator: tasks::Party::Person(person),
            batch: Box::new([new]),
        }));
    } else {
        let Some(proposal) = crate::fresh(&mut domain.core.counters, Family::Message) else {
            return refuse(domain, request, people::Refusal::Limit);
        };
        assert!(
            domain.core.goal_routes.insert(request, GoalRoute::Proposing { proposal }) == Ok(None),
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
    let Some(proposal) = domain.core.tasks.person_proposal(proposer, number) else {
        return refuse(domain, request, people::Refusal::Ended);
    };
    if let Err(why) = domain.core.goal_standing(&proposal, project, role) {
        return refuse(domain, request, why);
    }
    let event = match choice {
        people::ProposalDecision::Accept => {
            let source = match domain.core.goal_accept_allowed(
                &proposal,
                project,
                person,
                role.expect("checked standing"),
                env.limits.tasks.tree_tasks,
            ) {
                Ok(source) => source,
                Err(why) => return refuse(domain, request, why),
            };
            ensure_pool(domain, project, person);
            let mut goal = proposal.goal.clone();
            goal.funder = source;
            assert!(
                domain.core.goal_routes.insert(
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
            let Some(message) = crate::fresh(&mut domain.core.counters, Family::Message) else {
                return refuse(domain, request, people::Refusal::Limit);
            };
            assert!(
                domain.core.goal_routes.insert(request, GoalRoute::Deciding { proposer, proposal: number, by: person })
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
