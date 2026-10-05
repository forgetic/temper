use crate::domain::Task;
use crate::{AuthorityExecutor, Class, Fact, Grant, Parameter, Retries, Verdict};
use alloc::boxed::Box;
use skein_lib::{Deadlines, Id, List, Map, Queue, Slab};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Limits {
    pub tasks: u32,
    /// Live period/pool identities, refused before carving (domain/tasks.md, 2).
    pub funders: u32,
    pub project_tasks: u32,
    pub tree_tasks: u32,
    pub depth: u32,
    pub delegates: u32,
    pub batch: u32,
    pub dependencies: u32,
    pub inputs: u32,
    pub spec_bytes: u32,
    pub parameters: u32,
    pub result_bytes: u32,
    pub contract_choices: u32,
    pub charters: u32,
    pub authority_grants: u32,
    pub authority_segments: u32,
    pub authority_bytes: u32,
    pub executor_kinds: u32,
    pub retries: Retries,
    pub facts: u32,
}

#[must_use]
pub fn worst_case(limits: &Limits) -> Option<u64> {
    crate::domain::output_bound(limits)?;
    if limits.tasks == 0 || limits.batch == 0 || limits.tree_tasks == 0 {
        return None;
    }
    for class in [Class::Transient, Class::Permanent, Class::Run, Class::Agent, Class::Lost, Class::Invalid] {
        let retry = limits.retries.of(class);
        if retry.retries == u32::MAX || retry.base == skein_lib::Duration::ZERO || retry.max < retry.base {
            return None;
        }
    }
    let segments = u64::from(limits.authority_grants)
        .checked_mul(u64::from(limits.authority_segments))?
        .checked_mul(u64::try_from(size_of::<Box<[u8]>>()).ok()?)?;
    let payload = u64::from(limits.spec_bytes)
        .checked_add(u64::from(limits.result_bytes).checked_mul(2)?)?
        .checked_add(u64::from(limits.parameters).checked_mul(u64::try_from(size_of::<Parameter>()).ok()?)?)?
        .checked_add(u64::from(limits.inputs).checked_mul(8)?)?
        .checked_add(u64::from(limits.dependencies).checked_mul(16)?)?
        .checked_add(u64::from(limits.delegates).checked_mul(8)?)?
        .checked_add(u64::from(limits.contract_choices).checked_mul(u64::try_from(size_of::<Verdict>()).ok()?)?)?
        .checked_add(u64::from(limits.authority_grants).checked_mul(u64::try_from(size_of::<Grant>()).ok()?)?)?
        .checked_add(segments)?
        .checked_add(u64::from(limits.authority_bytes))?
        .checked_add(
            u64::from(limits.executor_kinds).checked_mul(u64::try_from(size_of::<AuthorityExecutor>()).ok()?)?,
        )?;
    Slab::<Task>::worst_case(limits.tasks)?
        .checked_add(Map::<crate::Funder, crate::FundingRecord>::worst_case(limits.funders)?)?
        .checked_add(Map::<u64, Id<Task>>::worst_case(limits.tasks)?)?
        .checked_add(Deadlines::<u64>::worst_case(limits.tasks)?)?
        .checked_add(Queue::<Fact>::worst_case(limits.facts)?)?
        .checked_add(u64::from(limits.tasks).checked_mul(payload)?)?
        .checked_add(u64::from(limits.charters).checked_mul(4)?)?
        // Bounded graph/admission and traversal snapshots; no recursive walk.
        .checked_add(List::<u64>::worst_case(limits.tasks.max(limits.batch))?.checked_mul(3)?)?
        .checked_add(List::<u32>::worst_case(limits.batch)?)?
        .checked_add(List::<u64>::worst_case(limits.delegates)?)
}
