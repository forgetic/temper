//! The largest first-slice tree and its diff, derived from domain limits.

use core::mem::size_of;
use temper_web_domain::Limits as DomainLimits;

/// Bounded capacities for rendering one page.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Limits {
    pub nodes: u32,
    pub patches: u32,
    pub depth: u32,
    pub markdown_depth: u32,
}

impl Limits {
    /// Frame, notices, and the largest chats window at their bounds.
    #[must_use]
    pub fn of(domain: &DomainLimits) -> Option<Limits> {
        let nodes = domain.window.checked_mul(8)?.checked_add(domain.notices.checked_mul(2)?)?.checked_add(64)?;
        let patches = nodes.checked_mul(6)?;
        Some(Limits { nodes, patches, depth: 16, markdown_depth: 0 })
    }
}

/// Heap for two trees and the largest patch queue, excluding text payloads.
#[must_use]
pub fn worst_case(limits: &Limits) -> Option<u64> {
    let nodes = u64::from(limits.nodes).checked_mul(2)?;
    let tree_bytes = nodes.checked_mul(u64::try_from(size_of::<crate::Node>()).ok()?)?;
    let patches = u64::from(limits.patches).checked_mul(u64::try_from(size_of::<crate::Patch>()).ok()?)?;
    tree_bytes.checked_add(patches)
}
