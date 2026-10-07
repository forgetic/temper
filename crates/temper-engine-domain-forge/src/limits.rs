//! Bounded connector top state (domain/engine.md, section 13).
use crate::{BranchHead, Hold, Key, Name, PullState, Repository, Stored, Subscriber, Topic};
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
    pub issue_policy: temper_engine_domain_forge_issues::Limits,
    pub change_policy: temper_engine_domain_forge_change::Limits,
    pub queue_window: skein_lib::Duration,
    pub client: client::Limits,
}

/// Maximum retained heap for the top and its child, excluding owned output
/// copies that the root takes at each decision boundary.
#[must_use]
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
        || l.facts == 0
        || l.adoptions == 0
        || l.collaborators == 0
        || l.landings == 0
        || l.repositories > l.client.repositories
        || l.entries > l.client.entries
        || l.holds > l.client.resources
    {
        return None;
    }
    if l.output < client::max_out(&l.client).checked_add(l.subscriptions)?.checked_add(l.holds)?.checked_add(8)? {
        return None;
    }
    let names =
        u64::from(l.tasks).checked_mul(u64::from(l.resources_per_task))?.checked_mul(u64::from(l.name_bytes))?;
    let subscribers = u64::from(l.subscriptions)
        .checked_mul(u64::from(l.paths_per_subscription))?
        .checked_mul(u64::from(l.name_bytes))?;
    client::worst_case(&l.client)?
        .checked_add(Map::<client::api::Repository, Repository>::worst_case(l.repositories)?)?
        .checked_add(Map::<u64, Box<[Name]>>::worst_case(l.tasks)?)?
        .checked_add(Map::<Name, Hold>::worst_case(l.holds)?)?
        .checked_add(Map::<(u64, Topic), Subscriber>::worst_case(l.subscriptions)?)?
        .checked_add(Map::<Name, BranchHead>::worst_case(l.client.resources)?)?
        .checked_add(Map::<Name, PullState>::worst_case(l.client.resources)?)?
        .checked_add(Map::<u64, client::Entry>::worst_case(l.entries)?)?
        .checked_add(Map::<client::api::Commit, u64>::worst_case(l.landings)?)?
        .checked_add(Map::<skein_lib::Token, crate::domain::PendingLanding>::worst_case(l.landings)?)?
        .checked_add(Map::<skein_lib::Token, crate::domain::PendingLost>::worst_case(l.holds)?)?
        .checked_add(Map::<u64, crate::ChangeRow>::worst_case(l.changes)?)?
        .checked_add(Map::<skein_lib::Token, crate::domain::PendingStep>::worst_case(l.changes)?)?
        .checked_add(Map::<u64, crate::IssueRow>::worst_case(l.issues)?)?
        .checked_add(Map::<skein_lib::Token, crate::domain::PendingAdoption>::worst_case(l.adoptions)?)?
        .checked_add(Queue::<Stored>::worst_case(l.output)?)?
        .checked_add(Queue::<Key>::worst_case(l.output)?)?
        .checked_add(Queue::<client::Fact>::worst_case(l.facts)?)?
        .checked_add(names)?
        .checked_add(subscribers)
}

use alloc::boxed::Box;
