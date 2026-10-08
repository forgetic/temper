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
use alloc::boxed::Box;
pub mod boundary;
mod brief;
mod domain;
mod held;
mod judge;
mod limits;
#[cfg(test)]
mod tests;
mod topics;
pub use boundary::*;
pub use domain::{Domain, fire, max_out, resume, step};
pub use judge::{Criterion, Freshness as JudgeFreshness, Judges, Reviewer, Verdict as JudgeVerdict};
pub use limits::{Limits, worst_case};
/// Connector-owned access to one exact effect on an adopted resource.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Access {
    Owned,
    Participant,
    Context,
    Unavailable,
}

/// What a forge write does to its named target.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EffectForm {
    Creation,
    Transition,
    Set,
}

/// Recovery promise of one forge effect kind.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Recovery {
    Keyed,
    Conditional,
    Idempotent,
    Unrecoverable,
}

/// Effect form and recovery class declared by the forge connector.
#[must_use]
pub const fn effect_shape(kind: u16) -> Option<(EffectForm, Recovery)> {
    match kind {
        2 => Some((EffectForm::Transition, Recovery::Unrecoverable)),
        4 => Some((EffectForm::Transition, Recovery::Conditional)),
        3 | 9 => Some((EffectForm::Creation, Recovery::Keyed)),
        5 | 7 | 8 => Some((EffectForm::Creation, Recovery::Unrecoverable)),
        6 => Some((EffectForm::Set, Recovery::Idempotent)),
        0 | 1 | 10..=u16::MAX => None,
    }
}

/// Connector-owned request and the facts jig may inspect before admitting it.
#[derive(Debug)]
pub struct AgentEffect {
    pub effect: temper_engine_domain_forge_client::Effect,
    pub kind: u16,
    pub state: [u8; 32],
    pub form: EffectForm,
    pub recovery: Recovery,
}

/// Classify and bound an agent write in the connector's own vocabulary.
pub fn describe_agent_effect(
    repository: &Repository,
    resource: &What,
    write: temper_engine_domain_forge_client::api::Write,
    key: Box<[u8]>,
    op_bytes: u32,
) -> Result<AgentEffect, temper_engine_domain_forge_client::api::Error> {
    use temper_engine_domain_forge_client as client;
    let (write, kind, state, form, valid_resource) = match write {
        client::api::Write::CreateIssue { title, body, .. } => (
            client::api::Write::CreateIssue { key, title, body },
            8,
            [0; 32],
            EffectForm::Creation,
            *resource == What::Repository,
        ),
        client::api::Write::Post { number, body, .. } => (
            client::api::Write::Post { number, key, body },
            7,
            [0; 32],
            EffectForm::Creation,
            *resource == What::Issue(number) || *resource == What::Pull(number),
        ),
        client::api::Write::Status { commit, context, check } => (
            client::api::Write::Status { commit, context, check },
            6,
            commit,
            EffectForm::Set,
            *resource == What::Repository,
        ),
        client::api::Write::OpenPull { .. }
        | client::api::Write::Review { .. }
        | client::api::Write::Edit { .. }
        | client::api::Write::SetReviewers { .. }
        | client::api::Write::Close { .. }
        | client::api::Write::Reopen { .. }
        | client::api::Write::Merge { .. }
        | client::api::Write::Update { .. }
        | client::api::Write::CreateBranch { .. }
        | client::api::Write::DeleteBranch { .. } => return Err(client::api::Error::Forbidden),
    };
    let access = effect_access(repository, resource, kind);
    let available = match access {
        Access::Owned | Access::Participant => true,
        Access::Context | Access::Unavailable => false,
    };
    if !valid_resource || !available {
        return Err(client::api::Error::Forbidden);
    }
    let effect = client::Effect { write, condition: client::Condition::None };
    let recovery = match client::recovery(&effect.write) {
        client::Recovery::Keyed => Recovery::Keyed,
        client::Recovery::Conditional => Recovery::Conditional,
        client::Recovery::Idempotent => Recovery::Idempotent,
        client::Recovery::Unrecoverable => Recovery::Unrecoverable,
    };
    match client::effect_bytes(&effect) {
        Some(bytes) if bytes <= u64::from(op_bytes) => {}
        Some(_) | None => return Err(client::api::Error::Forbidden),
    }
    Ok(AgentEffect { effect, kind, state, form, recovery })
}

/// Describe current access from the forge's adopted role and effect kinds.
#[must_use]
pub fn effect_access(repository: &Repository, what: &What, kind: u16) -> Access {
    let permitted = match kind {
        1 => repository.kinds.read,
        2 => repository.kinds.push,
        3 => repository.kinds.open,
        4 => repository.kinds.land,
        5 => repository.kinds.review,
        6 => repository.kinds.status,
        7 => repository.kinds.comment,
        8 => repository.kinds.issue,
        9 => repository.kinds.branch,
        _ => false,
    };
    let shared = match what {
        What::Issue(_) | What::Pull(_) => true,
        What::Repository | What::Branch(_) => false,
    };
    match repository.role {
        Role::Context => Access::Context,
        Role::Owned | Role::Adopted | Role::Fork if !permitted => Access::Unavailable,
        Role::Owned | Role::Adopted | Role::Fork if shared => Access::Participant,
        Role::Adopted => Access::Participant,
        Role::Fork if kind == 4 => Access::Participant,
        Role::Owned | Role::Fork => Access::Owned,
    }
}
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
        Stored::ProposedEffect(row) => Key::ProposedEffect(row.number),
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
            let tasks = match &row.goal_tasks {
                Some(tasks) => u64::try_from(tasks.len()).ok()?.checked_mul(8)?,
                None => 0,
            };
            let mut held = topic(&row.topic)?.checked_add(tasks)?.checked_add(
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
        Stored::ProposedEffect(row) => {
            let resource = match &row.resource {
                What::Branch(branch) => {
                    let mut total = 0_u64;
                    for segment in branch {
                        total = total.checked_add(bytes(segment)?)?;
                    }
                    total
                }
                What::Repository | What::Pull(_) | What::Issue(_) => 0,
            };
            client::effect_bytes(&row.entry.effect)?.checked_add(resource)
        }
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
            let mut held = u64::try_from(row.state.milestones.len().checked_add(before)?)
                .ok()?
                .checked_mul(u64::try_from(size_of::<temper_engine_domain_forge_issues::MilestoneKey>()).ok()?)?;
            if let Some(view) = &row.desired {
                held =
                    held.checked_add(bytes(view.title.as_bytes())?)?
                        .checked_add(bytes(view.goal_text.as_bytes())?)?
                        .checked_add(u64::try_from(view.plan.len()).ok()?.checked_mul(
                            u64::try_from(size_of::<temper_engine_domain_forge_issues::PlanItem>()).ok()?,
                        )?)?
                        .checked_add(u64::try_from(view.milestones.len()).ok()?.checked_mul(
                            u64::try_from(size_of::<temper_engine_domain_forge_issues::Milestone>()).ok()?,
                        )?)?;
                for item in &view.plan {
                    held = held.checked_add(bytes(item.text.as_bytes())?)?;
                }
                for milestone in &view.milestones {
                    held = held.checked_add(bytes(milestone.text.as_bytes())?)?;
                }
                if let Some(finished) = &view.finished {
                    held = held.checked_add(bytes(finished.as_bytes())?)?;
                }
            }
            Some(held)
        }
        Stored::Release(row) => match &row.pending {
            Some((_, Some(resource))) => name(resource),
            Some((_, None)) | None => Some(0),
        },
    }
}

/// Connector-owned description of one goal projection write.
#[derive(PartialEq, Eq, Debug)]
pub struct ProjectionEffect {
    pub resource: What,
    pub kind: u16,
    pub form: EffectForm,
    pub recovery: Recovery,
    pub access: Access,
}

/// Describe the write selected by the issue projection mechanism.
#[must_use]
pub fn describe_projection_effect(
    repository: &Repository,
    write: &temper_engine_domain_forge_client::api::Write,
) -> Option<ProjectionEffect> {
    use temper_engine_domain_forge_client as client;
    let (resource, kind, form) = match write {
        client::api::Write::CreateIssue { .. } => (What::Repository, 8, EffectForm::Creation),
        client::api::Write::Edit { number, .. } => (What::Issue(*number), 8, EffectForm::Set),
        client::api::Write::Post { number, .. } => (What::Issue(*number), 7, EffectForm::Creation),
        client::api::Write::Close { number } => (What::Issue(*number), 8, EffectForm::Transition),
        client::api::Write::Status { .. }
        | client::api::Write::OpenPull { .. }
        | client::api::Write::Review { .. }
        | client::api::Write::SetReviewers { .. }
        | client::api::Write::Reopen { .. }
        | client::api::Write::Merge { .. }
        | client::api::Write::Update { .. }
        | client::api::Write::CreateBranch { .. }
        | client::api::Write::DeleteBranch { .. } => return None,
    };
    let recovery = match client::recovery(write) {
        client::Recovery::Keyed => Recovery::Keyed,
        client::Recovery::Conditional => Recovery::Conditional,
        client::Recovery::Idempotent => Recovery::Idempotent,
        client::Recovery::Unrecoverable => Recovery::Unrecoverable,
    };
    Some(ProjectionEffect { access: effect_access(repository, &resource, kind), resource, kind, form, recovery })
}
