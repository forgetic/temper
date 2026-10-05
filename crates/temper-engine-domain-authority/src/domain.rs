//! Policies in force, changed only by bounded events (domain/authority.md, 6).

use skein_lib::{Map, Queue};

use crate::limits::{authority_within, landing_rules_within, requirements_within, within};
use crate::{Limits, Policy, Role, Rules, at_most, max_out, worst_case};

#[derive(Debug)]
pub struct Domain {
    rules: Rules,
    policies: Map<u32, Policy>,
    limits: Limits,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Event {
    Policy { project: u32, policy: Policy ,},
    Dropped { project: u32 ,},
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum PolicyRefusal {
    Oversized,
    AboveRules,
    InvalidRole,
    Full,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum PolicyFact {
    Added { project: u32 ,},
    Changed { project: u32 ,},
    Dropped { project: u32, existed: bool ,},
    Refused { project: u32, reason: PolicyRefusal ,},
}

pub const POLICY_MAX_OUT: u32 = 1;

impl Domain {
    #[must_use]
    pub fn new(rules: Rules, limits: Limits) -> Option<Domain> {
        max_out(&limits)?;
        worst_case(&limits)?;
        if !authority_within(&rules.ceiling, &limits)
            || !requirements_within(&rules.requirements, &limits)
            || !landing_rules_within(&rules.landing, &limits)
            || rules.implies.len() > limits.implications
            || rules.minimum_run_spend >= rules.maximum_run_spend
            || rules.maximum_run_spend > rules.ceiling.budget.spend
        {
            return None;
        }
        Some(Domain { rules, policies: Map::with_capacity(limits.projects), limits })
    }

    #[must_use]
    pub const fn rules(&self) -> &Rules {
        &self.rules
    }

    #[must_use]
    pub const fn limits(&self) -> &Limits {
        &self.limits
    }

    #[must_use]
    pub fn policy(&self, project: u32) -> Option<&Policy> {
        self.policies.get(&project)
    }

    #[must_use]
    #[expect(clippy::manual_find, reason = "the foundation's step subset uses bounded loops without closures")]
    pub fn role(&self, project: u32, number: u32) -> Option<&Role> {
        for role in &self.policy(project)?.roles {
            if role.number == number {
                return Some(role);
            }
        }
        None
    }
}

/// Policy events emit one fact and retain the previous policy on refusal.
/// Caller reserves `POLICY_MAX_OUT` slots before applying the event.
pub fn step(domain: &mut Domain, event: Event, out: &mut Queue<PolicyFact>) {
    assert!(out.room() >= POLICY_MAX_OUT, "caller reserves policy lifecycle output");
    match event {
        Event::Policy { project, policy } => {
            if let Some(reason) = invalid(domain, &policy) {
                out.push(PolicyFact::Refused { project, reason });
                return;
            }
            match domain.policies.insert(project, policy) {
                Ok(previous) => match previous {
                    Some(previous) => {
                        drop(previous);
                        out.push(PolicyFact::Changed { project });
                    }
                    None => out.push(PolicyFact::Added { project }),
                },
                Err(returned) => {
                    drop(returned);
                    out.push(PolicyFact::Refused { project, reason: PolicyRefusal::Full });
                }
            }
        }
        Event::Dropped { project } => {
            let previous = domain.policies.remove(&project);
            let existed = previous.is_some();
            drop(previous);
            out.push(PolicyFact::Dropped { project, existed });
        }
    }
}

fn invalid(domain: &Domain, policy: &Policy) -> Option<PolicyRefusal> {
    if !authority_within(&policy.ceiling, &domain.limits)
        || !within(policy.roles.len(), domain.limits.roles)
        || !requirements_within(&policy.requirements, &domain.limits)
        || !landing_rules_within(&policy.landing, &domain.limits)
    {
        return Some(PolicyRefusal::Oversized);
    }
    if !at_most(&policy.ceiling, &domain.rules.ceiling, &domain.rules.implies)
        || policy.period_spend > domain.rules.period_spend
    {
        return Some(PolicyRefusal::AboveRules);
    }
    for (position, role) in policy.roles.iter().enumerate() {
        if !authority_within(&role.authority, &domain.limits) {
            return Some(PolicyRefusal::Oversized);
        }
        if !at_most(&role.authority, &policy.ceiling, &domain.rules.implies)
            || role.period_spend > policy.period_spend
            || role.requests.0 & !crate::Requests::ALL.0 != 0
            || role.decides.0 & !crate::Proposals::ALL.0 != 0
        {
            return Some(PolicyRefusal::InvalidRole);
        }
        for earlier in policy.roles.get(..position).expect("enumerated position is in the roles") {
            if earlier.number == role.number {
                return Some(PolicyRefusal::InvalidRole);
            }
        }
    }
    for rule in &policy.landing {
        for approval in &rule.approvals {
            let mut exists = false;
            for role in &policy.roles {
                exists |= role.number == approval.role;
            }
            if !exists {
                return Some(PolicyRefusal::InvalidRole);
            }
        }
    }
    None
}
