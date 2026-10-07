//! Typed owner policy amendments and committed snapshots (domain/people.md,
//! section 5.2; domain/authority.md, sections 6 and 10). The people child
//! retains requests by key; the root checks and applies these values.

use alloc::boxed::Box;
use core::mem::size_of;

use crate::{Authority, Pattern};

/// A keyed edit to one mutable part of a project's policy.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum PolicyChange {
    /// Replace project spend for future periods and decisions.
    ProjectSpend { period_spend: u64 },
    /// Replace one existing role's authority, allotment and proposal decisions.
    Role(PolicyRole),
    /// Replace all project landing rules, one per branch pattern.
    Landing { rules: Box<[LandingRule]> },
}

/// Mutable project policy saved as one full committed value.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct PolicyValue {
    pub period_spend: u64,
    pub roles: Box<[PolicyRole]>,
    pub landing: Box<[LandingRule]>,
}

/// One role's grant, funding and request decision authority.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct PolicyRole {
    pub number: u32,
    pub authority: Authority,
    pub period_spend: u64,
    pub requests: u16,
    pub decides: u8,
}

/// Validity of a gate or review verdict at a later landing head.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Freshness {
    Exact,
    Clean,
}

/// One numbered required or advisory gate in a landing rule.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Gate {
    pub number: u32,
    pub blocking: bool,
    pub freshness: Freshness,
}

/// Required count of distinct reviewers with a project role.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Approval {
    pub role: u32,
    pub people: u32,
    pub freshness: Freshness,
}

/// Branch-scoped checks a project requires before landing.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct LandingRule {
    pub connector: u16,
    pub kind: u16,
    pub pattern: Pattern,
    pub ci: bool,
    pub up_to_date: bool,
    pub gates: Box<[Gate]>,
    pub approvals: Box<[Approval]>,
}

/// Checked deep-byte size of one owner policy payload.
#[must_use]
pub fn policy_bytes(policy: &PolicyValue) -> Option<u64> {
    let mut total =
        u64::try_from(policy.roles.len()).ok()?.checked_mul(u64::try_from(size_of::<PolicyRole>()).ok()?)?;
    for role in &policy.roles {
        total = total.checked_add(crate::authority_bytes(&role.authority)?)?;
    }
    total.checked_add(landing_rules_bytes(&policy.landing)?)
}

fn landing_rules_bytes(rules: &[LandingRule]) -> Option<u64> {
    let mut total = u64::try_from(rules.len()).ok()?.checked_mul(u64::try_from(size_of::<LandingRule>()).ok()?)?;
    for rule in rules {
        total = total.checked_add(pattern_bytes(&rule.pattern)?)?;
        total = total
            .checked_add(u64::try_from(rule.gates.len()).ok()?.checked_mul(u64::try_from(size_of::<Gate>()).ok()?)?)?;
        total = total.checked_add(
            u64::try_from(rule.approvals.len()).ok()?.checked_mul(u64::try_from(size_of::<Approval>()).ok()?)?,
        )?;
    }
    Some(total)
}

/// Checked deep-byte size of one keyed amendment.
#[must_use]
pub fn policy_change_bytes(change: &PolicyChange) -> Option<u64> {
    match change {
        PolicyChange::ProjectSpend { .. } => Some(0),
        PolicyChange::Role(role) => crate::authority_bytes(&role.authority),
        PolicyChange::Landing { rules } => landing_rules_bytes(rules),
    }
}

fn pattern_bytes(pattern: &Pattern) -> Option<u64> {
    let mut bytes =
        u64::try_from(pattern.segments.len()).ok()?.checked_mul(u64::try_from(size_of::<Box<[u8]>>()).ok()?)?;
    for segment in &pattern.segments {
        bytes = bytes.checked_add(u64::try_from(segment.len()).ok()?)?;
    }
    bytes.checked_add(match &pattern.last {
        crate::Last::Exact(word) | crate::Last::Open(word) => u64::try_from(word.len()).ok()?,
    })
}
