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
pub fn worst_case(l: &Limits) -> Option<u64> {
    l.initial_owners.checked_add(3)?;
    l.waiters.checked_add(1)?;
    l.sign_ins.checked_add(1)?;
    if l.waiters == 0 || l.sign_in_lifetime == Duration::ZERO {
        return None;
    }
    let people = Map::<u64, Identity>::worst_case(l.people)?
        .checked_add(Map::<IdentityKey, u64>::worst_case(l.people)?)?
        .checked_add(u64::from(l.people).checked_mul(u64::from(l.identity_bytes))?)?;
    let roles = Map::<u32, Box<[Holding]>>::worst_case(l.projects)?.checked_add(
        u64::from(l.projects)
            .checked_mul(u64::from(l.holdings))?
            .checked_mul(u64::try_from(size_of::<Holding>()).ok()?)?,
    )?;
    people
        .checked_add(roles)?
        .checked_add(Map::<u64, SignIn>::worst_case(l.sign_ins)?)?
        .checked_add(Deadlines::<u64>::worst_case(l.sign_ins)?)?
        .checked_add(Map::<RequestKey, Answered>::worst_case(l.requests)?)?
        .checked_add(u64::from(l.requests).checked_mul(u64::from(l.words))?)?
        .checked_add(Slab::<Pending>::worst_case(l.pending)?)?
        .checked_add(Map::<RequestKey, Id<Pending>>::worst_case(l.pending)?)?
        .checked_add(u64::from(l.pending).checked_mul(List::<skein_lib::ReplyTo>::worst_case(l.waiters)?)?)?
        .checked_add(u64::from(l.pending).checked_mul(u64::from(l.words))?)?
        .checked_add(u64::from(l.initial_owners).checked_mul(u64::try_from(size_of::<InitialOwner>()).ok()?)?)?
        .checked_add(List::<(u64, SignIn)>::worst_case(l.sign_ins)?)?
        .checked_add(List::<Holding>::worst_case(l.holdings)?)?
        .checked_add(Queue::<Fact>::worst_case(l.facts)?)
}
