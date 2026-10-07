//! The forge connector's top (domain/connectors.md, sections 2–7;
//! domain/forge.md, sections 3–12). It keeps adopted repositories, resource
//! holds, writer slots, subscriptions, procedure and projection rows, and
//! outbox entries. The client alone executes API calls; change and issues
//! decide policy without retained state.
//!
//! `step` accepts one parent or client event. Saves and erases join the
//! parent's decision. `Committed` is the only way a newly enqueued effect
//! reaches the client. `resume` and `fire` drive the client after earlier
//! saves have crossed that barrier. The top never knows tasks beyond numbers,
//! authorization grants or protocol wire formats.
#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]
extern crate alloc;
pub mod boundary;
mod brief;
mod domain;
mod limits;
#[cfg(test)]
mod tests;
pub use boundary::*;
pub use domain::{Domain, fire, max_out, resume, step};
pub use limits::{Limits, worst_case};
/// Stable key of one durable connector row.
#[must_use]
pub fn stored_key(row: &Stored) -> Key {
    use temper_engine_domain_forge_client as client;
    match row {
        Stored::Repository(row) => Key::Repository(row.provider),
        Stored::Hold(row) => Key::Hold(row.name.clone()),
        Stored::Names { task, .. } => Key::Names(*task),
        Stored::Subscription(row) => Key::Subscription { task: row.task, topic: row.topic.clone() },
        Stored::BranchHead(row) => Key::BranchHead(row.name.clone()),
        Stored::PullState(row) => Key::PullState(row.name.clone()),
        Stored::Ci(row) => Key::Ci { repository: row.repository, head: row.head },
        Stored::Landed { commit, .. } => Key::Landed(*commit),
        Stored::Entry(row) => Key::Entry(row.number),
        Stored::Client(row) => Key::Client(match row {
            client::Stored::Live(row) => client::Key::Live(row.watch.resource.clone()),
            client::Stored::Repository(row) => client::Key::Repository(row.repository),
        }),
        Stored::Change(row) => Key::Change(row.task),
        Stored::Issue(row) => Key::Issue(row.goal),
        Stored::Release(row) => Key::Release(row.task),
    }
}
/// Checked payload bytes of one connector record for its parent's store page.
#[must_use]
#[expect(clippy::too_many_lines, reason = "one exhaustive connector record payload sum")]
pub fn stored_bytes(record: &Stored) -> Option<u64> {
    use alloc::boxed::Box;
    use core::mem::size_of;
    use temper_engine_domain_forge_change as change;
    use temper_engine_domain_forge_client as client;
    fn bytes(value: &[u8]) -> Option<u64> {
        u64::try_from(value.len()).ok()
    }
    fn remark_bytes(remarks: &[GateRemark]) -> Option<u64> {
        let mut total = 0_u64;
        for remark in remarks {
            total = total.checked_add(bytes(&remark.words)?)?;
        }
        Some(total)
    }
    fn name(value: &Name) -> Option<u64> {
        match &value.what {
            What::Branch(parts) => {
                let mut held =
                    u64::try_from(parts.len()).ok()?.checked_mul(u64::try_from(size_of::<Box<[u8]>>()).ok()?)?;
                for part in parts {
                    held = held.checked_add(bytes(part)?)?;
                }
                Some(held)
            }
            What::Repository | What::Pull(_) | What::Issue(_) => Some(0),
        }
    }
    fn topic(value: &Topic) -> Option<u64> {
        match value {
            Topic::Landings { branch, .. } => bytes(branch),
            Topic::Ci { .. } | Topic::Pull { .. } | Topic::Participation { .. } => Some(0),
        }
    }
    fn state(value: &change::State, depth: u32) -> Option<u64> {
        if depth > 16 {
            return None;
        }
        match value {
            change::State::Gating { asked, .. } => {
                u64::try_from(asked.len()).ok()?.checked_mul(u64::try_from(size_of::<u64>()).ok()?)
            }
            change::State::Held { was, .. } => {
                u64::try_from(size_of::<change::State>()).ok()?.checked_add(state(was, depth.checked_add(1)?)?)
            }
            change::State::Producing { .. }
            | change::State::Opening { .. }
            | change::State::Recreating { .. }
            | change::State::Reopening { .. }
            | change::State::Checking { .. }
            | change::State::Queued { .. }
            | change::State::First { .. }
            | change::State::Updating { .. }
            | change::State::Resolving { .. }
            | change::State::Repairing { .. }
            | change::State::Landing { .. }
            | change::State::Landed { .. } => Some(0),
        }
    }
    match record {
        Stored::Repository(row) => {
            let mut held = bytes(&row.host)?
                .checked_add(bytes(&row.owner)?)?
                .checked_add(bytes(&row.name)?)?
                .checked_add(bytes(&row.prefix)?)?
                .checked_add(bytes(&row.settings.default_branch)?)?;
            match &row.protection {
                Protection::Rule(rule) => {
                    held = held.checked_add(bytes(&rule.branch)?)?.checked_add(
                        u64::try_from(rule.contexts.len())
                            .ok()?
                            .checked_mul(u64::try_from(size_of::<Box<[u8]>>()).ok()?)?,
                    )?;
                    for context in &rule.contexts {
                        held = held.checked_add(bytes(context)?)?;
                    }
                }
                Protection::Unknown | Protection::Absent => {}
            }
            Some(held)
        }
        Stored::Hold(row) => name(&row.name),
        Stored::Names { resources, .. } => {
            let mut held = u64::try_from(resources.len()).ok()?.checked_mul(u64::try_from(size_of::<Name>()).ok()?)?;
            for resource in resources {
                held = held.checked_add(name(resource)?)?;
            }
            Some(held)
        }
        Stored::Subscription(row) => {
            let mut held = topic(&row.topic)?.checked_add(
                u64::try_from(row.paths.len()).ok()?.checked_mul(u64::try_from(size_of::<Box<[u8]>>()).ok()?)?,
            )?;
            for path in &row.paths {
                held = held.checked_add(bytes(path)?)?;
            }
            Some(held)
        }
        Stored::BranchHead(row) => name(&row.name),
        Stored::PullState(row) => name(&row.name),
        Stored::Ci(_) | Stored::Landed { .. } => Some(0),
        Stored::Entry(row) => client::effect_bytes(&row.effect),
        Stored::Client(row) => client::stored_bytes(row),
        Stored::Change(row) => bytes(&row.branch)?
            .checked_add(bytes(&row.base)?)?
            .checked_add(bytes(&row.title)?)?
            .checked_add(bytes(&row.body)?)?
            .checked_add(
                u64::try_from(row.change.gates.len())
                    .ok()?
                    .checked_mul(u64::try_from(size_of::<change::Gate>()).ok()?)?,
            )?
            .checked_add(u64::try_from(row.change.clean.len()).ok()?.checked_mul(32)?)?
            .checked_add(
                u64::try_from(row.verdicts.len())
                    .ok()?
                    .checked_mul(u64::try_from(size_of::<change::GateReport>()).ok()?)?,
            )?
            .checked_add(
                u64::try_from(row.gate_remarks.len())
                    .ok()?
                    .checked_mul(u64::try_from(size_of::<GateRemark>()).ok()?)?,
            )?
            .checked_add(remark_bytes(&row.gate_remarks)?)?
            .checked_add(state(&row.change.state, 0)?),
        Stored::Issue(row) => {
            let before = match &row.before {
                Some(before) => before.milestones.len(),
                None => 0,
            };
            u64::try_from(row.state.milestones.len().checked_add(before)?)
                .ok()?
                .checked_mul(u64::try_from(size_of::<temper_engine_domain_forge_issues::MilestoneKey>()).ok()?)
        }
        Stored::Release(row) => match &row.pending {
            Some((_, Some(resource))) => name(resource),
            Some((_, None)) | None => Some(0),
        },
    }
}
