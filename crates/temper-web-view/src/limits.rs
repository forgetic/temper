//! The largest first-slice tree and its diff, derived from domain limits.

use skein_lib::{List, Queue, Stack};
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
    /// Frame, notices, the chats window and a Markdown result at their bounds.
    #[must_use]
    pub fn of(domain: &DomainLimits) -> Option<Limits> {
        let nodes = domain
            .window
            .checked_mul(8)?
            .checked_add(domain.notices.checked_mul(2)?)?
            .checked_add(domain.text.checked_mul(2)?)?
            .checked_add(domain.words.checked_mul(2)?)?
            .checked_add(128)?;
        let patches = nodes.checked_mul(6)?;
        Some(Limits { nodes, patches, depth: 32, markdown_depth: 4 })
    }
}

/// Heap for two trees, patches, diff scratch and bounded text payloads.
#[must_use]
pub fn worst_case(limits: &Limits) -> Option<u64> {
    let trees = List::<crate::Node>::worst_case(limits.nodes)?.checked_mul(2)?;
    let patches = Queue::<crate::Patch>::worst_case(limits.patches)?;
    let maps = List::<Option<u32>>::worst_case(limits.nodes)?.checked_mul(4)?;
    let stacks = Stack::<(u32, u32)>::worst_case(limits.depth)?.checked_mul(2)?;
    let made = List::<crate::Made>::worst_case(limits.nodes)?;
    // A single text or name is at most the domain's byte limit; `of`
    // includes twice that limit in nodes. This covers owned bytes in both
    // trees and in patches, including the largest inserted subtree.
    let payloads = u64::from(limits.nodes).checked_mul(u64::from(limits.nodes))?.checked_mul(8)?;
    trees.checked_add(patches)?.checked_add(maps)?.checked_add(stacks)?.checked_add(made)?.checked_add(payloads)
}
