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
pub fn worst_case(l: &Limits) -> Option<u64> {
    crate::domain::output_bound(l)?;
    if l.tasks == 0 || l.batch == 0 || l.tree_tasks == 0 {
        return None;
    }
    for class in [Class::Transient, Class::Permanent, Class::Run, Class::Agent, Class::Lost, Class::Invalid] {
        let retry = l.retries.of(class);
        if retry.retries == u32::MAX || retry.base == skein_lib::Duration::ZERO || retry.max < retry.base {
            return None;
        }
    }
    let segments = u64::from(l.authority_grants)
        .checked_mul(u64::from(l.authority_segments))?
        .checked_mul(u64::try_from(size_of::<Box<[u8]>>()).ok()?)?;
    let payload = u64::from(l.spec_bytes)
        .checked_add(u64::from(l.result_bytes).checked_mul(2)?)?
        .checked_add(u64::from(l.parameters).checked_mul(u64::try_from(size_of::<Parameter>()).ok()?)?)?
        .checked_add(u64::from(l.inputs).checked_mul(8)?)?
        .checked_add(u64::from(l.dependencies).checked_mul(16)?)?
        .checked_add(u64::from(l.delegates).checked_mul(8)?)?
        .checked_add(u64::from(l.contract_choices).checked_mul(u64::try_from(size_of::<Verdict>()).ok()?)?)?
        .checked_add(u64::from(l.authority_grants).checked_mul(u64::try_from(size_of::<Grant>()).ok()?)?)?
        .checked_add(segments)?
        .checked_add(u64::from(l.authority_bytes))?
        .checked_add(u64::from(l.executor_kinds).checked_mul(u64::try_from(size_of::<AuthorityExecutor>()).ok()?)?)?;
    Slab::<Task>::worst_case(l.tasks)?
        .checked_add(Map::<crate::Funder, crate::FundingRecord>::worst_case(l.funders)?)?
        .checked_add(Map::<u64, Id<Task>>::worst_case(l.tasks)?)?
        .checked_add(Deadlines::<u64>::worst_case(l.tasks)?)?
        .checked_add(Queue::<Fact>::worst_case(l.facts)?)?
        .checked_add(u64::from(l.tasks).checked_mul(payload)?)?
        .checked_add(u64::from(l.charters).checked_mul(4)?)?
        // Bounded graph/admission and traversal snapshots; no recursive walk.
        .checked_add(List::<u64>::worst_case(l.tasks.max(l.batch))?.checked_mul(3)?)?
        .checked_add(List::<u32>::worst_case(l.batch)?)?
        .checked_add(List::<u64>::worst_case(l.delegates)?)
}
