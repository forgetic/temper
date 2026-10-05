use crate::domain::{Answered, Pending, SignIn};
use crate::{Fact, Holding, Identity, IdentityKey, InitialOwner, RequestKey};
use alloc::boxed::Box;
use skein_lib::{Deadlines, Duration, Id, List, Map, Queue, Slab};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Limits {
    pub people: u32,
    pub sign_ins: u32,
    pub projects: u32,
    pub holdings: u32,
    pub initial_owners: u32,
    pub requests: u32,
    pub pending: u32,
    pub waiters: u32,
    pub identity_bytes: u32,
    pub words: u32,
    pub sign_in_lifetime: Duration,
    pub facts: u32,
}

/// Containers, retained bytes, pending copies and bounded bootstrap config.
#[must_use]
pub fn worst_case(limits: &Limits) -> Option<u64> {
    limits.initial_owners.checked_add(3)?;
    limits.waiters.checked_add(1)?;
    limits.sign_ins.checked_add(1)?;
    if limits.waiters == 0 || limits.sign_in_lifetime == Duration::ZERO {
        return None;
    }
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
        .checked_add(u64::from(limits.requests).checked_mul(u64::from(limits.words))?)?
        .checked_add(Slab::<Pending>::worst_case(limits.pending)?)?
        .checked_add(Map::<RequestKey, Id<Pending>>::worst_case(limits.pending)?)?
        .checked_add(u64::from(limits.pending).checked_mul(List::<skein_lib::ReplyTo>::worst_case(limits.waiters)?)?)?
        .checked_add(u64::from(limits.pending).checked_mul(u64::from(limits.words))?)?
        .checked_add(u64::from(limits.initial_owners).checked_mul(u64::try_from(size_of::<InitialOwner>()).ok()?)?)?
        .checked_add(List::<(u64, SignIn)>::worst_case(limits.sign_ins)?)?
        .checked_add(List::<Holding>::worst_case(limits.holdings)?)?
        .checked_add(Queue::<Fact>::worst_case(limits.facts)?)
}
