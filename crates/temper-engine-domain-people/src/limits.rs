//! Root-configured capacities and checked heap accounting (domain/people.md, sections 2–5). `worst_case` validates limits before construction;
//! bounds cover retained state and scratch. Root accounts output queues and
//! payloads separately. These calculations know no IO, credentials or tasks.

use crate::domain::{Answered, Pending, SignIn};
use crate::{Fact, Holding, Identity, IdentityKey, InitialOwner, RequestKey};
use alloc::boxed::Box;
use skein_lib::{Deadlines, Duration, Id, List, Map, Queue, Slab};

/// Immutable root-configured capacities and payload limits for people's retained state and one-step
/// outputs; validate via `worst_case` before construction. (domain/people.md, sections 2–5).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Limits {
    /// Maximum retained people and forge/user index entries. (domain/people.md, sections 2–5).
    pub people: u32,
    /// Maximum live secret-free sign-ins and expiry alarms. (domain/people.md, sections 2–5).
    pub sign_ins: u32,
    /// Maximum retained project role sets. (domain/people.md, sections 2–5).
    pub projects: u32,
    /// Maximum unique-person holdings per project role set. (domain/people.md, sections 2–5).
    pub holdings: u32,
    /// Maximum retained configured first-owner entries; also bounds bootstrap output matches.
    /// (domain/people.md, sections 2–5).
    pub initial_owners: u32,
    /// Maximum completed keys plus slots reserved by pending flights; no timed eviction is
    /// implemented here. (domain/people.md, sections 2–5).
    pub requests: u32,
    /// Maximum simultaneous routed keyed flights. (domain/people.md, sections 2–5).
    pub pending: u32,
    /// Positive maximum reply destinations per flight, including its first caller.
    /// (domain/people.md, sections 2–5).
    pub waiters: u32,
    /// Maximum combined login and display-name bytes per person. (domain/people.md, sections 2–5).
    pub identity_bytes: u32,
    /// Maximum opening-word or escalation rejection-reason bytes per keyed ask, including pending and completed copies.
    /// (domain/people.md, sections 2–5).
    pub words: u32,
    /// Nonzero configured lifetime projected once from admission's wall/monotonic environment.
    /// (domain/people.md, sections 2–5).
    pub sign_in_lifetime: Duration,
    /// Capacity of optional content-free observations; overflow increments a diagnostic lost
    /// counter. (domain/people.md, sections 2–5).
    pub facts: u32,
}

/// Containers, retained bytes, pending copies and bounded bootstrap config.
/// Checked heap bound under `limits` for retained containers/bytes, pending copies, bootstrap owners,
/// restoration sign-in snapshot, role-update scratch and facts. Returns `None` for arithmetic
/// overflow, zero waiters or zero sign-in lifetime; validates `max_out` additions. Caller separately
/// counts incoming/outgoing payloads and Request queue storage. (domain/people.md, sections 2–5).
#[must_use]
pub fn worst_case(limits: &Limits) -> Option<u64> {
    limits.initial_owners.checked_add(3)?;
    limits.waiters.checked_add(1)?;
    limits.sign_ins.checked_add(1)?;
    if limits.waiters == 0 || limits.sign_in_lifetime == Duration::ZERO {
        return None;
    }
    let ask_bytes =
        u64::from(limits.words).max(u64::from(limits.holdings).checked_mul(u64::try_from(size_of::<Holding>()).ok()?)?);
    let people = Map::<u64, Identity>::worst_case(limits.people)?
        .checked_add(Map::<IdentityKey, u64>::worst_case(limits.people)?)?
        .checked_add(u64::from(limits.people).checked_mul(u64::from(limits.identity_bytes))?)?;
    let roles = Map::<u32, Box<[Holding]>>::worst_case(limits.projects)?.checked_add(
        u64::from(limits.projects)
            .checked_mul(u64::from(limits.holdings))?
            .checked_mul(u64::try_from(size_of::<Holding>()).ok()?)?,
    )?;
    people
        .checked_add(roles)?
        .checked_add(Map::<u64, SignIn>::worst_case(limits.sign_ins)?)?
        .checked_add(Deadlines::<u64>::worst_case(limits.sign_ins)?)?
        .checked_add(Map::<RequestKey, Answered>::worst_case(limits.requests)?)?
        .checked_add(u64::from(limits.requests).checked_mul(ask_bytes)?)?
        .checked_add(Slab::<Pending>::worst_case(limits.pending)?)?
        .checked_add(Map::<RequestKey, Id<Pending>>::worst_case(limits.pending)?)?
        .checked_add(u64::from(limits.pending).checked_mul(List::<skein_lib::ReplyTo>::worst_case(limits.waiters)?)?)?
        .checked_add(u64::from(limits.pending).checked_mul(ask_bytes)?)?
        .checked_add(u64::from(limits.initial_owners).checked_mul(u64::try_from(size_of::<InitialOwner>()).ok()?)?)?
        .checked_add(List::<(u64, SignIn)>::worst_case(limits.sign_ins)?)?
        .checked_add(List::<Holding>::worst_case(limits.holdings)?)?
        .checked_add(Queue::<Fact>::worst_case(limits.facts)?)
}
