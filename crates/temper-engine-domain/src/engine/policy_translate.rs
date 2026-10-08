//! Total bounded translations between a person's typed policy request and the
//! authority child's policy value. Siblings share no types (programming-model.md).

use alloc::boxed::Box;

use super::{Freshness, LandingRule, authority, forge};
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
pub(super) fn build_landing(rules: &[LandingRule], project: bool, limit: u32, connector: u16) -> Option<LandingBuilt> {
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

pub(super) fn landing_roles(policy: &authority::Policy, rules: &[LandingRule]) -> bool {
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
