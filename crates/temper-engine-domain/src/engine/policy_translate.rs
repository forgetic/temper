//! Total bounded translations between a person's typed policy request and the
//! authority child's policy value. Siblings share no types (programming-model.md).

use alloc::boxed::Box;

use super::{authority, forge, people};
use skein_lib::List;

pub(super) struct LandingBuilt {
    pub requirements: Box<[authority::Requirement]>,
    pub criteria: Box<[forge::Criterion]>,
}

pub(super) fn combine_requirements(
    base: &[authority::Requirement],
    added: &[authority::Requirement],
    limit: u32,
) -> Option<Box<[authority::Requirement]>> {
    let mut result = List::with_capacity(limit);
    for requirement in base {
        result.push(requirement.clone()).ok()?;
    }
    for requirement in added {
        result.push(requirement.clone()).ok()?;
    }
    Some(result.into_boxed())
}

/// Translate a connector's landing policy into authority's opaque judges and
/// the forge's own parameters. Both arrays use the same stable index.
pub(super) fn build_landing(rules: &[people::LandingRule], project: bool, limit: u32) -> Option<LandingBuilt> {
    if u32::try_from(rules.len()).ok()? > limit {
        return None;
    }
    let mut requirements = List::with_capacity(limit);
    let mut criteria = List::with_capacity(limit);
    for rule in rules {
        if rule.connector != 1 || rule.kind != 4 {
            return None;
        }
        if rule.ci {
            landing_item(rule, project, 1, forge::Criterion::Ci, &mut requirements, &mut criteria)?;
        }
        if rule.up_to_date {
            landing_item(rule, project, 2, forge::Criterion::UpToDate, &mut requirements, &mut criteria)?;
        }
        for gate in &rule.gates {
            if gate.blocking {
                landing_item(
                    rule,
                    project,
                    3,
                    forge::Criterion::Gate {
                        number: gate.number,
                        freshness: match gate.freshness {
                            people::Freshness::Exact => forge::JudgeFreshness::Exact,
                            people::Freshness::Clean => forge::JudgeFreshness::Clean,
                        },
                    },
                    &mut requirements,
                    &mut criteria,
                )?;
            }
        }
        for approval in &rule.approvals {
            landing_item(
                rule,
                project,
                4,
                forge::Criterion::Approval {
                    role: approval.role,
                    people: approval.people,
                    freshness: match approval.freshness {
                        people::Freshness::Exact => forge::JudgeFreshness::Exact,
                        people::Freshness::Clean => forge::JudgeFreshness::Clean,
                    },
                },
                &mut requirements,
                &mut criteria,
            )?;
        }
    }
    Some(LandingBuilt { requirements: requirements.into_boxed(), criteria: criteria.into_boxed() })
}

pub(super) fn landing_roles(policy: &authority::Policy, rules: &[people::LandingRule]) -> bool {
    for rule in rules {
        for approval in &rule.approvals {
            let mut found = false;
            for role in &policy.roles {
                if role.number == approval.role {
                    found = true;
                }
            }
            if !found {
                return false;
            }
        }
    }
    true
}

fn landing_item(
    rule: &people::LandingRule,
    project: bool,
    kind: u16,
    criterion: forge::Criterion,
    requirements: &mut List<authority::Requirement>,
    criteria: &mut List<forge::Criterion>,
) -> Option<()> {
    let index = criteria.len();
    if index >= 0x8000_0000 {
        return None;
    }
    let parameters = if project { index | 0x8000_0000 } else { index };
    let guarded = match criterion {
        forge::Criterion::UpToDate => false,
        forge::Criterion::Ci | forge::Criterion::Gate { .. } | forge::Criterion::Approval { .. } => true,
    };
    criteria.push(criterion).ok()?;
    requirements
        .push(authority::Requirement {
            connector: rule.connector,
            kind: rule.kind,
            pattern: pattern_to_authority(rule.pattern.clone()),
            judge: authority::Judge { connector: rule.connector, requirement: kind, parameters },
            guard: if guarded {
                authority::Guard::Guarded
            } else {
                authority::Guard::Observed { freshness: skein_lib::Duration::ZERO }
            },
            must_be_guarded: guarded,
        })
        .ok()?;
    Some(())
}

pub(super) fn pattern_to_authority(value: people::Pattern) -> authority::Pattern {
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

fn replace_landing(
    policy: &mut authority::Policy,
    previous: &[people::LandingRule],
    next: &[people::LandingRule],
    limit: u32,
) -> Option<()> {
    let old = build_landing(previous, true, limit)?;
    let new = build_landing(next, true, limit)?;
    let mut requirements = List::with_capacity(limit);
    for requirement in &policy.requirements {
        if !old.requirements.contains(requirement) {
            requirements.push(requirement.clone()).ok()?;
        }
    }
    for requirement in new.requirements {
        requirements.push(requirement).ok()?;
    }
    policy.requirements = requirements.into_boxed();
    Some(())
}

/// Apply one typed mutable edit, keeping the ceiling and deployment rules.
pub(super) fn apply(
    policy: &mut authority::Policy,
    landing: &mut Box<[people::LandingRule]>,
    change: people::PolicyChange,
    limit: u32,
) -> Option<()> {
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
            if !landing_roles(policy, &rules) {
                return None;
            }
            replace_landing(policy, landing, &rules, limit)?;
            *landing = rules;
        }
    }
    Some(())
}

/// Capture the entire mutable value after a validated policy event.
pub(super) fn snapshot(policy: &authority::Policy, landing: &[people::LandingRule]) -> Option<people::PolicyValue> {
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
    Some(people::PolicyValue {
        period_spend: policy.period_spend,
        roles: roles.into_boxed(),
        landing: Box::from(landing),
    })
}

/// Restore a complete committed mutable value over the configured ceiling.
pub(super) fn restore(
    policy: &mut authority::Policy,
    landing: &mut Box<[people::LandingRule]>,
    value: people::PolicyValue,
    limit: u32,
) -> Option<()> {
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
    if !landing_roles(policy, &value.landing) {
        return None;
    }
    replace_landing(policy, landing, &value.landing, limit)?;
    *landing = value.landing;
    Some(())
}
