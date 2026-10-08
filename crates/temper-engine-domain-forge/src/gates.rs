//! Forge-owned landing gate templates and selection. The caller supplies the
//! core's opaque required verdicts; this owner interprets gate parameters.
use crate::{Limits, Repository};
use alloc::boxed::Box;
use core::mem::size_of;
use skein_lib::{List, Map};
use temper_engine_domain_forge_change as change;

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum GateLast {
    Exact(Box<[u8]>),
    Open(Box<[u8]>),
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct GatePattern {
    pub segments: Box<[Box<[u8]>]>,
    pub last: GateLast,
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct GateCandidate {
    pub number: u32,
    pub parameters: u32,
    pub blocking: bool,
    pub project_scope: bool,
    pub freshness: change::Freshness,
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct GateTemplate {
    pub pattern: GatePattern,
    pub gates: Box<[GateCandidate]>,
}
#[derive(Debug)]
pub struct GatePolicy {
    pub deployment: Box<[GateTemplate]>,
    pub projects: Map<u32, Box<[GateTemplate]>>,
}
impl GatePolicy {
    #[must_use]
    pub fn empty(projects: u32) -> Self {
        Self { deployment: Box::new([]), projects: Map::with_capacity(projects) }
    }
}
fn bytes(rules: &[GateTemplate]) -> Option<u64> {
    let mut total = u64::try_from(rules.len()).ok()?.checked_mul(u64::try_from(size_of::<GateTemplate>()).ok()?)?;
    for rule in rules {
        total = total.checked_add(
            u64::try_from(rule.pattern.segments.len())
                .ok()?
                .checked_mul(u64::try_from(size_of::<Box<[u8]>>()).ok()?)?,
        )?;
        for segment in &rule.pattern.segments {
            total = total.checked_add(u64::try_from(segment.len()).ok()?)?;
        }
        total = total
            .checked_add(match &rule.pattern.last {
                GateLast::Exact(last) | GateLast::Open(last) => u64::try_from(last.len()).ok()?,
            })?
            .checked_add(
                u64::try_from(rule.gates.len()).ok()?.checked_mul(u64::try_from(size_of::<GateCandidate>()).ok()?)?,
            )?;
    }
    Some(total)
}
fn rules_within(rules: &[GateTemplate], limits: &Limits) -> bool {
    if rules.len() > usize::try_from(limits.judge_criteria).expect("bounded criteria") {
        return false;
    }
    if match bytes(rules) {
        Some(bytes) => bytes > u64::from(limits.client.op_bytes),
        None => true,
    } {
        return false;
    }
    for rule in rules {
        if rule.gates.len() > usize::try_from(limits.change_policy.gates).expect("bounded gates") {
            return false;
        }
    }
    true
}
pub(crate) fn within(policy: &GatePolicy, limits: &Limits) -> bool {
    if policy.projects.capacity() > limits.judge_projects || !rules_within(&policy.deployment, limits) {
        return false;
    }
    for (_, rules) in &policy.projects {
        if !rules_within(rules, limits) {
            return false;
        }
    }
    true
}
fn covers(pattern: &GatePattern, name: &[Box<[u8]>]) -> bool {
    if name.get(..pattern.segments.len()) != Some(pattern.segments.as_ref()) {
        return false;
    }
    match &pattern.last {
        GateLast::Exact(last) => {
            name.len().checked_sub(1) == Some(pattern.segments.len()) && name.get(pattern.segments.len()) == Some(last)
        }
        GateLast::Open(last) => match name.get(pattern.segments.len()) {
            Some(segment) => segment.starts_with(last),
            None => false,
        },
    }
}
pub(crate) fn candidates(
    policy: &GatePolicy,
    limits: &Limits,
    project: u32,
    name: &[Box<[u8]>],
) -> Option<Box<[GateCandidate]>> {
    let mut result =
        List::with_capacity(limits.judge_criteria.checked_mul(limits.change_policy.gates)?.checked_mul(2)?);
    for rule in &policy.deployment {
        if covers(&rule.pattern, name) {
            for gate in &rule.gates {
                result.push(*gate).ok()?;
            }
        }
    }
    if let Some(rules) = policy.projects.get(&project) {
        for rule in rules.as_ref() {
            if covers(&rule.pattern, name) {
                for gate in &rule.gates {
                    result.push(*gate).ok()?;
                }
            }
        }
    }
    Some(result.into_boxed())
}
pub(crate) fn selected(
    limits: &Limits,
    repository: &Repository,
    candidates: &[GateCandidate],
    required: &[bool],
) -> Option<Box<[change::Gate]>> {
    if candidates.len() != required.len() {
        return None;
    }
    let mut gates: List<change::Gate> = List::with_capacity(limits.change_policy.gates);
    for (at, candidate) in candidates.iter().enumerate() {
        let is_check = !repository.ci && repository.checks.contains(&candidate.number);
        let required = candidate.blocking && *required.get(at)?;
        let mut present = false;
        for prior in gates.as_slice() {
            if prior.number == u64::from(candidate.number) {
                present = true;
            }
        }
        if present || (!is_check && candidate.blocking && !required) {
            continue;
        }
        gates
            .push(change::Gate {
                number: u64::from(candidate.number),
                kind: if is_check { change::GateKind::Check } else { change::GateKind::Agent },
                blocking: is_check || required,
                freshness: if is_check { change::Freshness::Exact } else { candidate.freshness },
                eager: false,
            })
            .ok()?;
    }
    if !repository.ci {
        for number in &repository.checks {
            let mut present = false;
            for prior in gates.as_slice() {
                if prior.number == u64::from(*number) {
                    present = true;
                }
            }
            if !present {
                gates
                    .push(change::Gate {
                        number: u64::from(*number),
                        kind: change::GateKind::Check,
                        blocking: true,
                        freshness: change::Freshness::Exact,
                        eager: false,
                    })
                    .ok()?;
            }
        }
    }
    Some(gates.into_boxed())
}
