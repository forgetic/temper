//! Bounded connector top state (domain/engine.md, section 13).
use crate::{BranchHead, Criterion, Hold, Key, Name, PullState, Repository, Stored, Subscriber, Topic};
use core::mem::size_of;
use skein_lib::{Map, Queue};
use temper_engine_domain_forge_client as client;

/// Capacity of each durable top table and its child.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Limits {
    pub repositories: u32,
    pub tasks: u32,
    pub holds: u32,
    pub subscriptions: u32,
    pub entries: u32,
    /// Connector payloads of live named calls, priced independently of outbox entries.
    pub named_calls: u32,
    pub changes: u32,
    pub issues: u32,
    pub resources_per_task: u32,
    pub paths_per_subscription: u32,
    pub name_bytes: u32,
    pub output: u32,
    pub facts: u32,
    pub adoptions: u32,
    pub collaborators: u32,
    pub landings: u32,
    /// Project judge tables retained beside committed policy.
    pub judge_projects: u32,
    /// Criteria in each deployment or project judge table.
    pub judge_criteria: u32,
    /// Connector brief sections gathering concurrently.
    pub brief_sections: u32,
    /// Maximum bytes retained for one gathered brief section.
    pub brief_bytes: u32,
    pub issue_policy: temper_engine_domain_forge_issues::Limits,
    pub change_policy: temper_engine_domain_forge_change::Limits,
    pub queue_window: skein_lib::Duration,
    pub client: client::Limits,
}

/// Maximum retained heap for the top and its child, excluding owned output
/// copies that the root takes at each decision boundary.
#[must_use]
#[expect(clippy::too_many_lines, reason = "one checked sum of the connector owners and retained payloads")]
pub fn worst_case(l: &Limits) -> Option<u64> {
    if l.repositories == 0
        || l.tasks == 0
        || l.holds == 0
        || l.subscriptions == 0
        || l.entries == 0
        || l.resources_per_task == 0
        || l.name_bytes == 0
        || l.queue_window == skein_lib::Duration::ZERO
        || l.output == 0
        || l.adoptions == 0
        || l.collaborators == 0
        || l.landings == 0
        || l.brief_sections == 0
        || l.brief_bytes == 0
        || l.repositories > l.client.repositories
        || l.entries > l.client.entries
        || l.holds > l.client.resources
    {
        return None;
    }
    if l.output
        < client::max_out(&l.client)
            .checked_add(l.subscriptions.checked_mul(3)?)?
            .checked_add(l.holds.checked_mul(3)?)?
            .checked_add(9)?
        || l.output < client::max_out(&l.client).checked_add(l.resources_per_task)?.checked_add(1)?
    {
        return None;
    }
    let names =
        u64::from(l.tasks).checked_mul(u64::from(l.resources_per_task))?.checked_mul(u64::from(l.name_bytes))?;
    let goal_tasks = u64::from(l.subscriptions).checked_mul(u64::from(l.issue_policy.plan_items))?.checked_mul(8)?;
    let subscribers = u64::from(l.subscriptions)
        .checked_mul(u64::from(l.paths_per_subscription))?
        .checked_mul(u64::from(l.name_bytes))?;
    let criteria = u64::from(l.judge_criteria).checked_mul(u64::try_from(size_of::<Criterion>()).ok()?)?;
    let judges = Map::<u32, Box<[Criterion]>>::worst_case(l.judge_projects)?
        .checked_add(criteria.checked_mul(u64::from(l.judge_projects).checked_add(1)?)?)?;
    client::worst_case(&l.client)?
        .checked_add(Map::<client::api::Repository, Repository>::worst_case(l.repositories)?)?
        .checked_add(
            u64::from(l.repositories)
                .checked_mul(u64::from(l.change_policy.gates))?
                .checked_mul(u64::try_from(size_of::<u32>()).ok()?)?,
        )?
        .checked_add(Map::<u64, Box<[Name]>>::worst_case(l.tasks)?)?
        .checked_add(Map::<Name, Hold>::worst_case(l.holds)?)?
        .checked_add(u64::from(l.holds).checked_mul(u64::from(l.client.answer_bytes).checked_mul(2)?)?)?
        .checked_add(Map::<(u64, Topic), Subscriber>::worst_case(l.subscriptions)?)?
        .checked_add(Map::<Name, BranchHead>::worst_case(l.client.resources)?)?
        .checked_add(Map::<Name, PullState>::worst_case(l.client.resources)?)?
        .checked_add(Map::<(client::api::Repository, client::api::Commit), crate::CiState>::worst_case(
            l.subscriptions,
        )?)?
        .checked_add(Map::<skein_lib::Token, crate::domain::PendingCi>::worst_case(l.subscriptions)?)?
        .checked_add(Map::<skein_lib::Token, crate::capabilities::Pending>::worst_case(l.repositories)?)?
        .checked_add(u64::from(l.repositories).checked_mul(u64::from(l.client.answer_bytes))?)?
        .checked_add(Map::<u64, client::Entry>::worst_case(l.entries)?)?
        .checked_add(Map::<crate::NamedCall, crate::CallPayload>::worst_case(l.named_calls)?)?
        .checked_add(u64::from(l.named_calls).checked_mul(u64::from(l.client.answer_bytes))?)?
        .checked_add(Map::<u64, crate::ProposedEffect>::worst_case(l.tasks)?)?
        .checked_add(
            u64::from(l.tasks).checked_mul(u64::from(l.client.op_bytes).checked_add(u64::from(l.name_bytes))?)?,
        )?
        .checked_add(Map::<client::api::Commit, u64>::worst_case(l.landings)?)?
        .checked_add(Map::<skein_lib::Token, crate::domain::PendingLanding>::worst_case(l.landings)?)?
        .checked_add(Map::<skein_lib::Token, crate::domain::PendingLost>::worst_case(l.holds)?)?
        .checked_add(Map::<u64, crate::ChangeRow>::worst_case(l.changes)?)?
        .checked_add(skein_lib::List::<(Box<[u8]>, crate::DriftChange)>::worst_case(l.changes)?)?
        .checked_add(u64::from(l.changes).checked_mul(u64::from(l.name_bytes).checked_mul(3)?)?)?
        .checked_add(Map::<u64, crate::topics::Files>::worst_case(l.changes)?)?
        .checked_add(Map::<skein_lib::Token, crate::topics::Pending>::worst_case(l.changes)?)?
        .checked_add(file_payload(l)?)?
        .checked_add(
            u64::from(l.changes)
                .checked_mul(u64::from(l.change_policy.gates))?
                .checked_mul(u64::try_from(size_of::<temper_engine_domain_forge_change::GateReport>()).ok()?)?,
        )?
        .checked_add(u64::from(l.changes).checked_mul(u64::from(l.change_policy.gates))?.checked_mul(
            u64::try_from(size_of::<crate::GateRemark>()).ok()?.checked_add(u64::from(l.client.answer_bytes))?,
        )?)?
        .checked_add(Map::<skein_lib::Token, crate::domain::PendingStep>::worst_case(l.changes)?)?
        .checked_add(Map::<u64, crate::IssueRow>::worst_case(l.issues)?)?
        .checked_add(Map::<u64, skein_lib::Wall>::worst_case(l.issues)?)?
        .checked_add(Map::<u64, skein_lib::Wall>::worst_case(l.changes)?)?
        .checked_add(Map::<u64, bool>::worst_case(l.changes)?)?
        .checked_add(Map::<u64, crate::ReleaseRow>::worst_case(l.tasks)?)?
        .checked_add(u64::from(l.tasks).checked_mul(u64::from(l.name_bytes))?)?
        .checked_add(issue_payload(l)?)?
        .checked_add(Map::<skein_lib::Token, crate::domain::PendingAdoption>::worst_case(l.adoptions)?)?
        .checked_add(Map::<skein_lib::Token, crate::brief::BriefFetch>::worst_case(l.brief_sections)?)?
        .checked_add(Map::<skein_lib::Token, crate::BriefSource>::worst_case(l.brief_sections)?)?
        .checked_add(Map::<skein_lib::Token, crate::held::Pending>::worst_case(l.brief_sections)?)?
        .checked_add(Map::<skein_lib::Token, crate::held::Held>::worst_case(l.brief_sections)?)?
        .checked_add(u64::from(l.brief_sections).checked_mul(u64::from(l.brief_bytes))?)?
        .checked_add(u64::from(l.brief_sections).checked_mul(u64::from(l.brief_bytes))?)?
        .checked_add(u64::from(l.brief_sections).checked_mul(u64::from(l.client.answer_bytes))?)?
        .checked_add(u64::from(l.brief_bytes).checked_mul(2)?)?
        .checked_add(Queue::<Stored>::worst_case(l.output)?)?
        .checked_add(Queue::<Key>::worst_case(l.output)?)?
        .checked_add(Queue::<client::Fact>::worst_case(l.facts)?)?
        .checked_add(names)?
        .checked_add(subscribers)?
        .checked_add(goal_tasks)?
        .checked_add(judges)
}

use alloc::boxed::Box;

fn file_payload(l: &Limits) -> Option<u64> {
    u64::from(l.changes)
        .checked_mul(u64::from(l.paths_per_subscription))?
        .checked_mul(u64::from(l.name_bytes).checked_add(u64::try_from(size_of::<Box<[u8]>>()).ok()?)?)?
        .checked_mul(2)
}

fn issue_payload(l: &Limits) -> Option<u64> {
    use temper_engine_domain_forge_issues::{Milestone, MilestoneKey, PlanItem};
    let plan = u64::from(l.issue_policy.plan_items)
        .checked_mul(u64::from(l.issue_policy.body_bytes).checked_add(u64::try_from(size_of::<PlanItem>()).ok()?)?)?;
    let milestones = u64::from(l.issue_policy.milestones).checked_mul(
        u64::from(l.issue_policy.comment_bytes)
            .checked_add(u64::try_from(size_of::<Milestone>()).ok()?)?
            .checked_add(u64::try_from(size_of::<MilestoneKey>()).ok()?.checked_mul(2)?)?,
    )?;
    u64::from(l.issues).checked_mul(
        u64::from(l.issue_policy.title_bytes)
            .checked_add(u64::from(l.issue_policy.body_bytes))?
            .checked_add(u64::from(l.issue_policy.comment_bytes))?
            .checked_add(plan)?
            .checked_add(milestones)?,
    )
}
