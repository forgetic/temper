//! Admission, queue room and owned policy memory (domain/authority.md, 6–10).

use alloc::boxed::Box;
use core::mem::size_of;

use skein_lib::Map;

use crate::{
    Approval, Authority, Executor, Gate, Grant, Implication, Landing, LandingRule, Name, Pattern, Policy, Requirement,
    Role,
};

/// Root-configured capacities for policy state and admitted questions; owned values do not enforce
/// these bounds until admission. (domain/authority.md, sections 6–11).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Limits {
    /// Maximum live project policies.
    pub projects: u32,
    /// Maximum roles per project policy.
    pub roles: u32,
    /// Maximum generic requirements per deployment or project policy.
    pub requirements: u32,
    /// Maximum fact kinds per requirement and maximum reports per effect question.
    pub facts: u32,
    /// Maximum grants per admitted authority value.
    pub grants: u32,
    /// Maximum permitted executor entries per admitted authority value.
    pub executors: u32,
    /// Maximum base or resource-name segment count.
    pub segments: u32,
    /// Maximum bytes per base segment or terminal value.
    pub segment_bytes: u32,
    /// Maximum retained connector-kind implication pairs.
    pub implications: u32,
    /// Maximum directly delegated tasks per batch question.
    pub batch: u32,
    /// Maximum model-account usability reports per run question.
    pub accounts: u32,
    /// Maximum written resources per run question.
    pub writes: u32,
    /// Maximum landing rules per deployment or project policy.
    pub landing_rules: u32,
    /// Maximum gates per landing rule and per landing snapshot.
    pub gates: u32,
    /// Maximum approval requirements per landing rule.
    pub approvals: u32,
    /// Maximum clean predecessor heads per landing snapshot.
    pub heads: u32,
    /// Maximum gate verdict reports per landing snapshot.
    pub verdicts: u32,
    /// Maximum human review reports per snapshot; also bounds each positive required-person count.
    pub reviews: u32,
}

/// Room for the largest check, including every independent reason. Checked free-slot bound for any
/// one `check_*` finding output; caller reserves that room before the query. Returns `None` on
/// bound arithmetic overflow; policy lifecycle outputs use `POLICY_MAX_OUT` separately.
#[must_use]
pub fn max_out(limits: &Limits) -> Option<u32> {
    let batch = limits.batch.checked_mul(6)?.checked_add(12)?;
    let run = limits.writes.checked_mul(4)?.checked_add(limits.accounts)?.checked_add(10)?;
    let one_landing = limits.gates.checked_add(limits.approvals.checked_mul(2)?)?.checked_add(2)?;
    let landing = limits.landing_rules.checked_mul(2)?.checked_mul(one_landing)?.checked_add(limits.gates)?;
    let effect = limits.requirements.checked_mul(limits.facts)?.checked_mul(2)?.checked_add(8)?.checked_add(landing)?;
    Some(batch.max(run).max(effect))
}

/// Heap held by the domain's rules and full project table. Questions and finding queues belong to
/// the caller and are counted there. Checked retained-heap bound for deployment rules and a full
/// live policy table under `limits`; `None` means unrepresentable arithmetic. Caller separately
/// counts question payloads, constructed needs and output queues.
#[must_use]
pub fn worst_case(limits: &Limits) -> Option<u64> {
    let authority = authority_heap(limits)?;
    let requirements = requirement_heap(limits)?.checked_add(landing_heap(limits)?)?;
    let role_heap = bytes(u64::from(limits.roles), size_of::<Role>())?
        .checked_add(authority.checked_mul(u64::from(limits.roles))?)?;
    let policy = authority.checked_add(requirements)?.checked_add(role_heap)?;
    let configured = authority
        .checked_add(requirements)?
        .checked_add(bytes(u64::from(limits.implications), size_of::<Implication>())?)?;
    Map::<u32, Policy>::worst_case(limits.projects)?
        .checked_add(policy.checked_mul(u64::from(limits.projects))?)?
        .checked_add(configured)
}

fn bytes(count: u64, size: usize) -> Option<u64> {
    count.checked_mul(u64::try_from(size).ok()?)
}

fn pattern_heap(limits: &Limits) -> Option<u64> {
    let segment = u64::from(limits.segment_bytes).checked_add(u64::try_from(size_of::<Box<[u8]>>()).ok()?)?;
    u64::from(limits.segments).checked_mul(segment)?.checked_add(u64::from(limits.segment_bytes))
}

fn authority_heap(limits: &Limits) -> Option<u64> {
    let grants = u64::from(limits.grants)
        .checked_mul(u64::try_from(size_of::<Grant>()).ok()?.checked_add(pattern_heap(limits)?)?)?;
    grants.checked_add(bytes(u64::from(limits.executors), size_of::<Executor>())?)
}

fn requirement_heap(limits: &Limits) -> Option<u64> {
    let one = u64::try_from(size_of::<Requirement>())
        .ok()?
        .checked_add(pattern_heap(limits)?)?
        .checked_add(bytes(u64::from(limits.facts), size_of::<u16>())?)?;
    u64::from(limits.requirements).checked_mul(one)
}

fn landing_heap(limits: &Limits) -> Option<u64> {
    let one = u64::try_from(size_of::<LandingRule>())
        .ok()?
        .checked_add(pattern_heap(limits)?)?
        .checked_add(bytes(u64::from(limits.gates), size_of::<Gate>())?)?
        .checked_add(bytes(u64::from(limits.approvals), size_of::<Approval>())?)?;
    u64::from(limits.landing_rules).checked_mul(one)
}

pub(crate) fn landing_rules_within(rules: &[LandingRule], limits: &Limits) -> bool {
    if !within(rules.len(), limits.landing_rules) {
        return false;
    }
    for rule in rules {
        if !pattern_within(&rule.pattern, limits)
            || !within(rule.gates.len(), limits.gates)
            || !within(rule.approvals.len(), limits.approvals)
        {
            return false;
        }
        for approval in &rule.approvals {
            if approval.people == 0 || approval.people > limits.reviews {
                return false;
            }
        }
    }
    true
}

pub(crate) fn landing_within(landing: &Landing, limits: &Limits) -> bool {
    within(landing.clean.len(), limits.heads)
        && within(landing.checks.len(), limits.gates)
        && within(landing.gates.len(), limits.gates)
        && within(landing.verdicts.len(), limits.verdicts)
        && within(landing.reviews.len(), limits.reviews)
}

pub(crate) fn within(len: usize, limit: u32) -> bool {
    match u32::try_from(len) {
        Ok(len) => len <= limit,
        Err(_) => false,
    }
}

pub(crate) fn pattern_within(pattern: &Pattern, limits: &Limits) -> bool {
    if !name_within_segments(&pattern.segments, limits) {
        return false;
    }
    match &pattern.last {
        crate::Last::Exact(bytes) | crate::Last::Open(bytes) => within(bytes.len(), limits.segment_bytes),
    }
}

pub(crate) fn name_within(name: &Name, limits: &Limits) -> bool {
    name_within_segments(&name.segments, limits)
}

fn name_within_segments(segments: &[Box<[u8]>], limits: &Limits) -> bool {
    if !within(segments.len(), limits.segments) {
        return false;
    }
    for segment in segments {
        if !within(segment.len(), limits.segment_bytes) {
            return false;
        }
    }
    true
}

pub(crate) fn authority_within(authority: &Authority, limits: &Limits) -> bool {
    if !within(authority.grants.len(), limits.grants)
        || !within(authority.delegation.kinds.len(), limits.executors)
        || authority.notes.0 & !15 != 0
    {
        return false;
    }
    for grant in &authority.grants {
        if !pattern_within(&grant.pattern, limits) {
            return false;
        }
    }
    true
}

pub(crate) fn requirements_within(requirements: &[Requirement], limits: &Limits) -> bool {
    if !within(requirements.len(), limits.requirements) {
        return false;
    }
    for requirement in requirements {
        if !pattern_within(&requirement.pattern, limits) || !within(requirement.facts.len(), limits.facts) {
            return false;
        }
    }
    true
}
