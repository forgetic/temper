//! Bounds for retained client state and its output queue.
use skein_lib::Duration;

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
    let base = skein_lib::List::<crate::ChatLine>::worst_case(limits.window)?
        .checked_add(skein_lib::List::<crate::Project>::worst_case(limits.projects)?)?
        .checked_add(skein_lib::Queue::<crate::Notice>::worst_case(limits.notices)?)?
        .checked_add(skein_lib::Queue::<crate::Fact>::worst_case(limits.facts)?)?;
    let texts = u64::from(limits.window)
        .checked_mul(u64::from(limits.text))?
        .checked_add(u64::from(limits.projects).checked_mul(u64::from(limits.text))?)?
        .checked_add(u64::from(limits.requests).checked_mul(u64::from(limits.words))?)?;
    base.checked_add(texts)
}
