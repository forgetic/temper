//! Total bounded translations between a person's typed policy request and the
//! authority child's policy value. Siblings share no types (programming-model.md).

use super::{authority, people};
use skein_lib::List;

fn pattern_to_authority(value: people::Pattern) -> authority::Pattern {
    authority::Pattern {
        segments: value.segments,
        last: match value.last {
            people::Last::Exact(word) => authority::Last::Exact(word),
            people::Last::Open(word) => authority::Last::Open(word),
        },
    }
}

fn pattern_to_people(value: &authority::Pattern) -> people::Pattern {
    people::Pattern {
        segments: value.segments.clone(),
        last: match &value.last {
            authority::Last::Exact(word) => people::Last::Exact(word.clone()),
            authority::Last::Open(word) => people::Last::Open(word.clone()),
        },
    }
}

fn authority_to_authority(value: people::Authority) -> Option<authority::Authority> {
    let mut grants = List::with_capacity(u32::try_from(value.grants.len()).ok()?);
    for grant in value.grants {
        grants
            .push(authority::Grant {
                connector: grant.connector,
                kind: grant.kind,
                pattern: pattern_to_authority(grant.pattern),
            })
            .ok()?;
    }
    let mut kinds = List::with_capacity(u32::try_from(value.delegation.kinds.len()).ok()?);
    for kind in value.delegation.kinds {
        kinds
            .push(match kind {
                people::Executor::Charter(number) => authority::Executor::Charter(number),
                people::Executor::Procedure(number) => authority::Executor::Procedure(number),
                people::Executor::Role(number) => authority::Executor::Role(number),
            })
            .ok()?;
    }
    Some(authority::Authority {
        tools: authority::Tools(value.tools),
        grants: grants.into_boxed(),
        delegation: authority::Delegation {
            kinds: kinds.into_boxed(),
            tasks: value.delegation.tasks,
            depth: value.delegation.depth,
        },
        budget: authority::Budget { spend: value.spend, deadline: value.deadline },
        notes: authority::Scopes(value.notes),
    })
}

fn authority_to_people(value: &authority::Authority) -> Option<people::Authority> {
    let mut grants = List::with_capacity(u32::try_from(value.grants.len()).ok()?);
    for grant in &value.grants {
        grants
            .push(people::Grant {
                connector: grant.connector,
                kind: grant.kind,
                pattern: pattern_to_people(&grant.pattern),
            })
            .ok()?;
    }
    let mut kinds = List::with_capacity(u32::try_from(value.delegation.kinds.len()).ok()?);
    for kind in &value.delegation.kinds {
        kinds
            .push(match kind {
                authority::Executor::Charter(number) => people::Executor::Charter(*number),
                authority::Executor::Procedure(number) => people::Executor::Procedure(*number),
                authority::Executor::Role(number) => people::Executor::Role(*number),
            })
            .ok()?;
    }
    Some(people::Authority {
        tools: value.tools.0,
        grants: grants.into_boxed(),
        delegation: people::Delegation {
            kinds: kinds.into_boxed(),
            tasks: value.delegation.tasks,
            depth: value.delegation.depth,
        },
        spend: value.budget.spend,
        deadline: value.budget.deadline,
        notes: value.notes.0,
    })
}

fn freshness_to_authority(value: people::Freshness) -> authority::Freshness {
    match value {
        people::Freshness::Exact => authority::Freshness::Exact,
        people::Freshness::Clean => authority::Freshness::Clean,
    }
}

fn freshness_to_people(value: authority::Freshness) -> people::Freshness {
    match value {
        authority::Freshness::Exact => people::Freshness::Exact,
        authority::Freshness::Clean => people::Freshness::Clean,
    }
}

fn landing_to_authority(value: people::LandingRule) -> Option<authority::LandingRule> {
    let mut gates = List::with_capacity(u32::try_from(value.gates.len()).ok()?);
    for gate in value.gates {
        gates
            .push(authority::Gate {
                number: gate.number,
                blocking: gate.blocking,
                freshness: freshness_to_authority(gate.freshness),
            })
            .ok()?;
    }
    let mut approvals = List::with_capacity(u32::try_from(value.approvals.len()).ok()?);
    for approval in value.approvals {
        approvals
            .push(authority::Approval {
                role: approval.role,
                people: approval.people,
                freshness: freshness_to_authority(approval.freshness),
            })
            .ok()?;
    }
    Some(authority::LandingRule {
        connector: value.connector,
        kind: value.kind,
        pattern: pattern_to_authority(value.pattern),
        ci: value.ci,
        up_to_date: value.up_to_date,
        gates: gates.into_boxed(),
        approvals: approvals.into_boxed(),
    })
}

fn landing_to_people(value: &authority::LandingRule) -> Option<people::LandingRule> {
    let mut gates = List::with_capacity(u32::try_from(value.gates.len()).ok()?);
    for gate in &value.gates {
        gates
            .push(people::Gate {
                number: gate.number,
                blocking: gate.blocking,
                freshness: freshness_to_people(gate.freshness),
            })
            .ok()?;
    }
    let mut approvals = List::with_capacity(u32::try_from(value.approvals.len()).ok()?);
    for approval in &value.approvals {
        approvals
            .push(people::Approval {
                role: approval.role,
                people: approval.people,
                freshness: freshness_to_people(approval.freshness),
            })
            .ok()?;
    }
    Some(people::LandingRule {
        connector: value.connector,
        kind: value.kind,
        pattern: pattern_to_people(&value.pattern),
        ci: value.ci,
        up_to_date: value.up_to_date,
        gates: gates.into_boxed(),
        approvals: approvals.into_boxed(),
    })
}

/// Apply one typed mutable edit, keeping the ceiling and deployment rules.
pub(super) fn apply(policy: &mut authority::Policy, change: people::PolicyChange) -> Option<()> {
    match change {
        people::PolicyChange::ProjectSpend { period_spend } => policy.period_spend = period_spend,
        people::PolicyChange::Role(edit) => {
            let value = authority_to_authority(edit.authority)?;
            let mut found = false;
            for role in &mut policy.roles {
                if role.number == edit.number {
                    role.authority = value.clone();
                    role.period_spend = edit.period_spend;
                    role.requests = authority::Requests(edit.requests);
                    role.decides = authority::Proposals(edit.decides);
                    found = true;
                }
            }
            if !found {
                return None;
            }
        }
        people::PolicyChange::Landing { rules } => {
            let mut translated = List::with_capacity(u32::try_from(rules.len()).ok()?);
            for rule in rules {
                translated.push(landing_to_authority(rule)?).ok()?;
            }
            policy.landing = translated.into_boxed();
        }
    }
    Some(())
}

/// Capture the entire mutable value after a validated policy event.
pub(super) fn snapshot(policy: &authority::Policy) -> Option<people::PolicyValue> {
    let mut roles = List::with_capacity(u32::try_from(policy.roles.len()).ok()?);
    for role in &policy.roles {
        roles
            .push(people::PolicyRole {
                number: role.number,
                authority: authority_to_people(&role.authority)?,
                period_spend: role.period_spend,
                requests: role.requests.0,
                decides: role.decides.0,
            })
            .ok()?;
    }
    let mut landing = List::with_capacity(u32::try_from(policy.landing.len()).ok()?);
    for rule in &policy.landing {
        landing.push(landing_to_people(rule)?).ok()?;
    }
    Some(people::PolicyValue {
        period_spend: policy.period_spend,
        roles: roles.into_boxed(),
        landing: landing.into_boxed(),
    })
}

/// Restore a complete committed mutable value over the configured ceiling.
pub(super) fn restore(policy: &mut authority::Policy, value: people::PolicyValue) -> Option<()> {
    policy.period_spend = value.period_spend;
    let mut roles = List::with_capacity(u32::try_from(value.roles.len()).ok()?);
    for role in value.roles {
        roles
            .push(authority::Role {
                number: role.number,
                authority: authority_to_authority(role.authority)?,
                period_spend: role.period_spend,
                requests: authority::Requests(role.requests),
                decides: authority::Proposals(role.decides),
            })
            .ok()?;
    }
    policy.roles = roles.into_boxed();
    let mut landing = List::with_capacity(u32::try_from(value.landing.len()).ok()?);
    for rule in value.landing {
        landing.push(landing_to_authority(rule)?).ok()?;
    }
    policy.landing = landing.into_boxed();
    Some(())
}
