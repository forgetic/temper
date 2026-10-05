//! Admission, queue room and owned policy memory (domain/authority.md, 6–8).

use alloc::boxed::Box;
use core::mem::size_of;

use skein_lib::Map;

use crate::{Authority, Executor, Grant, Implication, Name, Pattern, Policy, Requirement, Role};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Limits {
    pub projects: u32,
    pub roles: u32,
    pub requirements: u32,
    pub facts: u32,
    pub grants: u32,
    pub executors: u32,
    pub segments: u32,
    pub segment_bytes: u32,
    pub implications: u32,
    pub batch: u32,
    pub accounts: u32,
    pub writes: u32,
}

/// Room for the largest check, including every independent reason.
#[must_use]
pub fn max_out(limits: &Limits) -> Option<u32> {
    let batch = limits.batch.checked_mul(6)?.checked_add(12)?;
    let run = limits.writes.checked_mul(4)?.checked_add(limits.accounts)?.checked_add(10)?;
    let effect = limits.requirements.checked_mul(limits.facts)?.checked_mul(2)?.checked_add(8)?;
    Some(batch.max(run).max(effect))
}

/// Heap held by the domain's rules and full project table. Questions and
/// finding queues belong to the caller and are counted there.
#[must_use]
pub fn worst_case(limits: &Limits) -> Option<u64> {
    let authority = authority_heap(limits)?;
    let requirements = requirement_heap(limits)?;
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
        crate::Last::None => true,
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
