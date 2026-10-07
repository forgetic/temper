//! Owner changes to current policy and finite person pools (domain/people.md, 4–5).
//! The root validates one edit under deployment rules and persists the whole
//! mutable policy value. Existing task grants keep their prior authority.

use alloc::boxed::Box;

use super::{
    Decision, Domain, Env, Limits, PersonTaskRoute, Token, Work, authority, people, policy_translate, roles, save,
    tasks,
};
use crate::{Record, Write};
use skein_lib::Queue;

fn refused(domain: &mut Domain, request: Token, why: people::Refusal) {
    domain.work.push(Work::People(people::Event::Decided { request, outcome: people::Outcome::Refused(why) }));
}

pub(super) fn change(
    domain: &mut Domain,
    env: &Env<Limits>,
    decision: &mut Decision,
    request: Token,
    person: u64,
    project: u32,
    change: people::PolicyChange,
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
    let mut permissions = match domain.config.permission_roles.get(&project) {
        Some(mappings) => mappings.clone(),
        None => Box::new([]),
    };
    if policy_translate::apply(&mut policy, &mut permissions, change, env.limits.authority.requirements).is_none() {
        return refused(domain, request, people::Refusal::Unknown);
    }
    let Some(snapshot) = policy_translate::snapshot(&policy, &permissions) else {
        return refused(domain, request, people::Refusal::Limit);
    };
    if match people::policy_bytes(&snapshot) {
        Some(bytes) => {
            bytes > u64::from(env.limits.journal.transcript_bytes)
                || bytes > super::row_bound(&env.limits).expect("validated row bound")
        }
        None => true,
    } {
        return refused(domain, request, people::Refusal::Limit);
    }
    let mut facts = Queue::with_capacity(authority::POLICY_MAX_OUT);
    authority::step(&mut domain.config.authority, authority::Event::Policy { project, policy }, &mut facts);
    match facts.pop().expect("policy update terminal") {
        authority::PolicyFact::Changed { .. } => {
            assert!(domain.config.permission_roles.insert(project, permissions).is_ok(), "admitted permission policy");
            save(
                decision,
                &env.limits,
                Write::Save(Record::People(people::Stored::Policy { project, value: snapshot })),
            );
            domain.work.push(Work::People(people::Event::Decided {
                request,
                outcome: people::Outcome::PolicyChanged { project },
            }));
        }
        authority::PolicyFact::Refused { .. } => refused(domain, request, people::Refusal::Authority),
        authority::PolicyFact::Added { .. } | authority::PolicyFact::Dropped { .. } => {
            unreachable!("existing project policy replacement")
        }
    }
}

pub(super) fn restore(domain: &mut Domain, project: u32, value: people::PolicyValue) {
    let row_bound = super::row_bound(&domain.limits).expect("validated row bound");
    match people::policy_bytes(&value) {
        Some(bytes) if bytes <= u64::from(domain.limits.journal.transcript_bytes) && bytes <= row_bound => {}
        Some(_) | None => {
            domain.startup = super::Startup::Failed;
            return;
        }
    }
    let Some(mut policy) = domain.config.authority.policy(project).cloned() else {
        domain.startup = super::Startup::Failed;
        return;
    };
    let mut permissions = match domain.config.permission_roles.get(&project) {
        Some(mappings) => mappings.clone(),
        None => Box::new([]),
    };
    if policy_translate::restore(&mut policy, &mut permissions, value, domain.limits.authority.requirements).is_none() {
        domain.startup = super::Startup::Failed;
        return;
    }
    let mut facts = Queue::with_capacity(authority::POLICY_MAX_OUT);
    authority::step(&mut domain.config.authority, authority::Event::Policy { project, policy }, &mut facts);
    if facts.pop() != Some(authority::PolicyFact::Changed { project })
        || domain.config.permission_roles.insert(project, permissions).is_err()
    {
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
