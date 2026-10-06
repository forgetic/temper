//! Root-configured admission and memory bounds (domain/tasks.md, section 10). Counts retained live state and bounded scratch; parents count
//! their owned input/output copies separately. This module performs no admission
//! mutation, authority decision or allocation during the bound calculation.
use crate::domain::Task;
use crate::{AuthorityExecutor, Class, Fact, Grant, Parameter, Retries, Verdict};
use alloc::boxed::Box;
use skein_lib::{Deadlines, Id, List, Map, Queue, Slab};

/// Immutable root-configured limits for live task state, finite sources, owned semantic payloads
/// and retry policies; validate through `worst_case` before construction. (domain/tasks.md, section 10).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Limits {
    /// Maximum retained live task slots, name-index entries and retry alarms. (domain/tasks.md, section 10).
    pub tasks: u32,
    /// Maximum retained finite period/pool identities; source pressure is refused before mutation.
    /// (domain/tasks.md, section 10).
    pub funders: u32,
    /// Maximum live tasks per project, including every member of an admitted batch.
    /// (domain/tasks.md, section 10).
    pub project_tasks: u32,
    /// Maximum lifetime tasks made in a requester tree, including its root; completed delegates do
    /// not return this capacity. (domain/tasks.md, section 10).
    pub tree_tasks: u32,
    /// Maximum structural depth below the requester-tree root, which has depth zero.
    /// (domain/tasks.md, section 10).
    pub depth: u32,
    /// Maximum live direct delegates of one requester task. (domain/tasks.md, section 10).
    pub delegates: u32,
    /// Maximum directly created tasks in one atomic nonempty `Make` batch. (domain/tasks.md, section 10).
    pub batch: u32,
    /// Maximum immutable dependency identities per task; also bounds its remaining live
    /// `waiting_on` subset. (domain/tasks.md, section 10).
    pub dependencies: u32,
    /// Shape bound for typed input identities; current `Make` and live restore require
    /// `Spec::inputs` to be empty because no historical-input route is implemented.
    /// (domain/tasks.md, section 10).
    pub inputs: u32,
    /// Maximum combined specification words and byte-valued parameter bytes per task.
    /// (domain/tasks.md, section 10).
    pub spec_bytes: u32,
    /// Maximum typed parameters per specification. (domain/tasks.md, section 10).
    pub parameters: u32,
    /// Maximum result/reason bytes; contracts cannot allow larger results and cancellation may
    /// retain one bounded reason plus one bounded partial result. (domain/tasks.md, section 10).
    pub result_bytes: u32,
    /// Maximum distinct-code choices in a nonempty verdict contract. (domain/tasks.md, section 10).
    pub contract_choices: u32,
    /// Maximum retained configured agent-charter numbers. (domain/tasks.md, section 10).
    pub charters: u32,
    /// Maximum resource grants per task authority carrier. (domain/tasks.md, section 10).
    pub authority_grants: u32,
    /// Maximum base segments per carried grant pattern. (domain/tasks.md, section 10).
    pub authority_segments: u32,
    /// Maximum total bytes across every grant's base and terminal segments in one authority
    /// carrier. (domain/tasks.md, section 10).
    pub authority_bytes: u32,
    /// Maximum carried authority executor-permission entries; task execution itself is agent-only.
    /// (domain/tasks.md, section 10).
    pub executor_kinds: u32,
    /// Validated per-class failure retry and pause policies. (domain/tasks.md, section 10).
    pub retries: Retries,
    /// Optional content-free observation capacity; overflow is diagnostic and changes no decision.
    /// (domain/tasks.md, section 10).
    pub facts: u32,
}

/// Pure checked heap bound for live containers and nested payloads, finite ledgers, charter
/// configuration, optional facts and bounded graph/traversal scratch under `limits`. Returns `None` on
/// overflow or invalid task/batch/tree/retry settings. Validates output-bound arithmetic; caller
/// separately counts incoming data, `RunContext`/saved-row copies and `Request` queues.
/// (domain/tasks.md, section 10).
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
        .checked_add(u64::from(limits.result_bytes).checked_mul(3)?)?
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
