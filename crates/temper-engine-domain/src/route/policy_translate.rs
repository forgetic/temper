//! Total bounded translations between a person's typed policy request and the
//! authority child's policy value. Siblings share no types (programming-model.md).

use alloc::boxed::Box;

use super::{Freshness, LandingRule, authority, forge};
use skein_lib::List;

pub(crate) struct LandingBuilt {
    pub requirements: Box<[authority::Requirement]>,
    pub criteria: Box<[forge::Criterion]>,
}

pub(crate) fn combine_requirements(
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
pub(crate) fn build_landing(rules: &[LandingRule], project: bool, limit: u32, connector: u16) -> Option<LandingBuilt> {
    if u32::try_from(rules.len()).ok()? > limit {
        return None;
    }
    let mut requirements = List::with_capacity(limit);
    let mut criteria = List::with_capacity(limit);
    for rule in rules {
        if rule.connector != connector || rule.kind != 4 {
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
                            Freshness::Exact => forge::JudgeFreshness::Exact,
                            Freshness::Clean => forge::JudgeFreshness::Clean,
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
                        Freshness::Exact => forge::JudgeFreshness::Exact,
                        Freshness::Clean => forge::JudgeFreshness::Clean,
                    },
                },
                &mut requirements,
                &mut criteria,
            )?;
        }
    }
    Some(LandingBuilt { requirements: requirements.into_boxed(), criteria: criteria.into_boxed() })
}

pub(crate) fn landing_roles(policy: &authority::Policy, rules: &[LandingRule]) -> bool {
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
    rule: &LandingRule,
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
    if rule.enforce {
        requirements
            .push(authority::Requirement {
                connector: rule.connector,
                kind: rule.kind,
                pattern: rule.pattern.clone(),
                judge: authority::Judge { connector: rule.connector, requirement: kind, parameters },
                guard: if guarded {
                    authority::Guard::Guarded
                } else {
                    authority::Guard::Observed { freshness: skein_lib::Duration::ZERO }
                },
                must_be_guarded: guarded,
            })
            .ok()?;
    }
    Some(())
}

pub(crate) fn gate_policy(
    policy: &crate::LandingPolicy,
    rules_limit: u32,
    gates_limit: u32,
) -> Option<forge::GatePolicy> {
    let deployment = gate_templates(&policy.deployment, false, rules_limit, gates_limit)?;
    let mut projects = skein_lib::Map::with_capacity(policy.projects.capacity());
    for (project, rules) in &policy.projects {
        projects.insert(*project, gate_templates(rules, true, rules_limit, gates_limit)?).ok()?;
    }
    Some(forge::GatePolicy { deployment, projects })
}
fn gate_templates(
    rules: &[LandingRule],
    project: bool,
    rules_limit: u32,
    gates_limit: u32,
) -> Option<Box<[forge::GateTemplate]>> {
    let mut templates = List::with_capacity(rules_limit);
    let mut criterion = 0_u32;
    for rule in rules {
        if rule.ci {
            criterion = criterion.checked_add(1)?;
        }
        if rule.up_to_date {
            criterion = criterion.checked_add(1)?;
        }
        let mut gates = List::with_capacity(gates_limit);
        for gate in &rule.gates {
            let parameters = criterion | if project { 0x8000_0000 } else { 0 };
            if gate.blocking {
                criterion = criterion.checked_add(1)?;
            }
            gates
                .push(forge::GateCandidate {
                    number: gate.number,
                    parameters,
                    blocking: gate.blocking,
                    project_scope: project,
                    freshness: match gate.freshness {
                        Freshness::Exact => temper_engine_domain_forge_change::Freshness::Exact,
                        Freshness::Clean => temper_engine_domain_forge_change::Freshness::Clean,
                    },
                })
                .ok()?;
        }
        criterion = criterion.checked_add(u32::try_from(rule.approvals.len()).ok()?)?;
        templates
            .push(forge::GateTemplate {
                pattern: forge::GatePattern {
                    segments: rule.pattern.segments.clone(),
                    last: match &rule.pattern.last {
                        authority::Last::Exact(last) => forge::GateLast::Exact(last.clone()),
                        authority::Last::Open(last) => forge::GateLast::Open(last.clone()),
                    },
                },
                gates: gates.into_boxed(),
            })
            .ok()?;
    }
    Some(templates.into_boxed())
}
