//! Owner changes to current policy and finite person pools (domain/people.md, 4–5).
//! Current policy route changes one role's period ceiling, preserving authority
//! already granted to live tasks. The root persists the override beside people.

use super::{Decision, Domain, Env, Limits, PersonTaskRoute, Token, Work, authority, people, roles, save, tasks};
use crate::{Record, Write};
use skein_lib::Queue;

fn refused(domain: &mut Domain, request: Token, why: people::Refusal) {
    domain.work.push(Work::People(people::Event::Decided { request, outcome: people::Outcome::Refused(why) }));
}

fn set_role_spend(policy: &mut authority::Policy, number: u32, spend: u64) -> bool {
    for role in &mut policy.roles {
        if role.number == number {
            role.period_spend = spend;
            return true;
        }
    }
    false
}

#[expect(clippy::too_many_arguments, reason = "one authenticated policy route carries its exact edit")]
pub(super) fn change(
    domain: &mut Domain,
    env: &Env<Limits>,
    decision: &mut Decision,
    request: Token,
    person: u64,
    project: u32,
    number: u32,
    spend: u64,
) {
    if let Err(why) = roles::allowed(domain, person, project) {
        return refused(domain, request, why);
    }
    if !decision.room_for(2, env.limits.people.waiters) {
        return refused(domain, request, people::Refusal::Busy);
    }
    let Some(mut policy) = domain.config.authority.policy(project).cloned() else {
        return refused(domain, request, people::Refusal::Unknown);
    };
    if !set_role_spend(&mut policy, number, spend) {
        return refused(domain, request, people::Refusal::Unknown);
    }
    let mut facts = Queue::with_capacity(authority::POLICY_MAX_OUT);
    authority::step(&mut domain.config.authority, authority::Event::Policy { project, policy }, &mut facts);
    match facts.pop().expect("policy update terminal") {
        authority::PolicyFact::Changed { .. } => {
            save(
                decision,
                &env.limits,
                Write::Save(Record::People(people::Stored::PolicyRole { project, role: number, period_spend: spend })),
            );
            domain.work.push(Work::People(people::Event::Decided {
                request,
                outcome: people::Outcome::PolicyChanged { project, role: number },
            }));
        }
        authority::PolicyFact::Refused { .. } => refused(domain, request, people::Refusal::Authority),
        authority::PolicyFact::Added { .. } | authority::PolicyFact::Dropped { .. } => {
            unreachable!("existing project policy replacement")
        }
    }
}

pub(super) fn restore(domain: &mut Domain, project: u32, number: u32, spend: u64) {
    let Some(mut policy) = domain.config.authority.policy(project).cloned() else {
        domain.startup = super::Startup::Failed;
        return;
    };
    if !set_role_spend(&mut policy, number, spend) {
        domain.startup = super::Startup::Failed;
        return;
    }
    let mut facts = Queue::with_capacity(authority::POLICY_MAX_OUT);
    authority::step(&mut domain.config.authority, authority::Event::Policy { project, policy }, &mut facts);
    if facts.pop() != Some(authority::PolicyFact::Changed { project }) {
        domain.startup = super::Startup::Failed;
    }
}

#[expect(clippy::too_many_arguments, reason = "one authenticated pool route carries its beneficiary and budget")]
pub(super) fn pool(
    domain: &mut Domain,
    env: &Env<Limits>,
    decision: &mut Decision,
    request: Token,
    owner: u64,
    project: u32,
    person: u64,
    budget: u64,
) {
    if let Err(why) = roles::allowed(domain, owner, project) {
        return refused(domain, request, why);
    }
    if domain.person_tasks.len() >= domain.limits.tasks.tasks
        || domain.work.room() < 2
        || !decision.room_for(4, env.limits.people.waiters)
    {
        return refused(domain, request, people::Refusal::Busy);
    }
    let Some(role) = domain.people.role(person, project) else {
        return refused(domain, request, people::Refusal::Unknown);
    };
    let Some(allowed) = domain.config.authority.role(project, super::escalation::role_number(role)) else {
        return refused(domain, request, people::Refusal::Unknown);
    };
    if budget > allowed.period_spend {
        return refused(domain, request, people::Refusal::Authority);
    }
    let Some(project_policy) = domain.config.authority.policy(project) else {
        return refused(domain, request, people::Refusal::Unknown);
    };
    if domain.config.period_budget > project_policy.period_spend {
        return refused(domain, request, people::Refusal::Authority);
    }
    let period = tasks::Funder::Period { project, period: domain.config.period };
    let funder = tasks::Funder::Pool { project, person, period: domain.config.period };
    let needed = u32::from(domain.tasks.funding(period).is_none())
        .saturating_add(u32::from(domain.tasks.funding(funder).is_none()));
    if domain.tasks.funding_room() < needed {
        return refused(domain, request, people::Refusal::Busy);
    }
    if domain.tasks.funding(period).is_none() && budget > domain.config.period_budget {
        return refused(domain, request, people::Refusal::Authority);
    }
    if domain.tasks.funding(period).is_none() {
        domain.work.push(Work::Tasks(tasks::Event::OpenPeriod {
            reply_to: super::internal(0),
            project,
            period: domain.config.period,
            budget: domain.config.period_budget,
        }));
    }
    assert!(
        domain.person_tasks.insert(request, PersonTaskRoute::PoolSet { project, person }) == Ok(None),
        "one admitted pool route"
    );
    if domain.tasks.funding(funder).is_some() {
        domain.work.push(Work::Tasks(tasks::Event::ResizePool {
            reply_to: super::internal(request.raw()),
            project,
            person,
            period: domain.config.period,
            budget,
        }));
    } else {
        domain.work.push(Work::Tasks(tasks::Event::CarvePool {
            reply_to: super::internal(request.raw()),
            project,
            person,
            period: domain.config.period,
            budget,
        }));
    }
}
