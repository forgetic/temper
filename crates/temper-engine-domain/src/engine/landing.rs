//! Temper's forge landing policy and connector judge parameters. These values
//! belong to the application; people carries only authority requirements
//! (jig's domain/authority.md, sections 6 and 10).

use alloc::boxed::Box;
use core::mem::size_of;
use jig_core_authority::Pattern;

/// Validity of a forge gate or review verdict at a later landing head.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Freshness {
    Exact,
    Clean,
}

/// One numbered required or advisory forge gate.
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

/// Forge branch checks selected by this application's configuration.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct LandingRule {
    pub connector: u16,
    pub kind: u16,
    pub pattern: Pattern,
    /// Whether this configured criterion is required before a project policy edit.
    pub enforce: bool,
    pub ci: bool,
    pub up_to_date: bool,
    pub gates: Box<[Gate]>,
    pub approvals: Box<[Approval]>,
}

/// Checked deep-byte size of application-owned landing rules.
#[must_use]
pub fn landing_rules_bytes(rules: &[LandingRule]) -> Option<u64> {
    let mut total = u64::try_from(rules.len()).ok()?.checked_mul(u64::try_from(size_of::<LandingRule>()).ok()?)?;
    for rule in rules {
        total = total.checked_add(
            u64::try_from(rule.pattern.segments.len())
                .ok()?
                .checked_mul(u64::try_from(size_of::<Box<[u8]>>()).ok()?)?,
        )?;
        for segment in &rule.pattern.segments {
            total = total.checked_add(u64::try_from(segment.len()).ok()?)?;
        }
        total = total.checked_add(match &rule.pattern.last {
            jig_core_authority::Last::Exact(word) | jig_core_authority::Last::Open(word) => {
                u64::try_from(word.len()).ok()?
            }
        })?;
        total = total
            .checked_add(u64::try_from(rule.gates.len()).ok()?.checked_mul(u64::try_from(size_of::<Gate>()).ok()?)?)?;
        total = total.checked_add(
            u64::try_from(rule.approvals.len()).ok()?.checked_mul(u64::try_from(size_of::<Approval>()).ok()?)?,
        )?;
    }
    Some(total)
}
