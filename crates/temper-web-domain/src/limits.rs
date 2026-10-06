//! Bounds for retained client state and its output queue.
use crate::domain::Timer;
use crate::reads::ReadSlot;
use crate::requests::Pending;
use crate::streams::Stream;
use crate::{Fact, Key, Notice, Object, ObjectKey, Project};
use skein_lib::{Deadlines, Duration, Id, List, Map, Queue, Slab};

/// Jittered reconnect and retry bounds.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Backoff {
    pub first: Duration,
    pub most: Duration,
}

/// Immutable capacities and durations shared with the view.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Limits {
    pub objects: u32,
    pub requests: u32,
    pub streams: u32,
    pub reads: u32,
    pub window: u32,
    pub turns: u32,
    pub tree: u32,
    pub drafts: u32,
    pub notices: u32,
    pub words: u32,
    pub text: u32,
    pub streaming: u32,
    pub backoff: Backoff,
    pub heartbeat: Duration,
    pub linger: Duration,
    pub notice: Duration,
    pub save: Duration,
    pub facts: u32,
    pub projects: u32,
}

/// Checked maximum retained heap, excluding input and output payload ownership.
#[must_use]
pub fn worst_case(limits: &Limits) -> Option<u64> {
    if limits.objects == 0
        || limits.requests == 0
        || limits.streams == 0
        || limits.reads == 0
        || limits.projects == 0
        || limits.words == 0
        || limits.text == 0
        || limits.words > limits.text
        || limits.backoff.first == Duration::ZERO
        || limits.backoff.most < limits.backoff.first
        || limits.heartbeat == Duration::ZERO
        || limits.notice == Duration::ZERO
        || limits.linger == Duration::ZERO
        || limits.save == Duration::ZERO
    {
        return None;
    }
    limits.requests.checked_add(limits.streams)?.checked_add(5)?;
    let timer_capacity = limits.requests.checked_add(limits.streams)?.checked_add(limits.objects)?.checked_add(2)?;
    let base = List::<crate::ChatLine>::worst_case(limits.window)?
        .checked_add(List::<Project>::worst_case(limits.projects)?)?
        .checked_add(Queue::<Notice>::worst_case(limits.notices)?)?
        .checked_add(Queue::<Fact>::worst_case(limits.facts)?)?
        .checked_add(Slab::<Stream>::worst_case(limits.streams)?)?
        .checked_add(Slab::<ReadSlot>::worst_case(limits.reads)?)?
        .checked_add(Slab::<Pending>::worst_case(limits.requests)?)?
        .checked_add(Slab::<Object>::worst_case(limits.objects)?)?
        .checked_add(Map::<ObjectKey, Id<Object>>::worst_case(limits.objects)?)?
        .checked_add(Map::<Key, Id<Pending>>::worst_case(limits.requests)?)?
        .checked_add(Deadlines::<Timer>::worst_case(timer_capacity)?)?;
    let texts = u64::from(limits.window)
        .checked_mul(u64::from(limits.text))?
        .checked_add(u64::from(limits.projects).checked_mul(u64::from(limits.text))?)?
        .checked_add(u64::from(limits.requests).checked_mul(u64::from(limits.words))?)?
        .checked_add(u64::from(limits.objects).checked_mul(u64::from(limits.text))?)?
        .checked_add(u64::from(limits.text))?
        .checked_add(u64::from(limits.words).checked_mul(2)?)?;
    base.checked_add(texts)
}
