//! Capacity limits and the client's heap bound (domain/forge.md, section 5).
//!
//! The limits bound retained calls, live rows and in-flight writes. This
//! module retains no state and knows no task policy. `worst_case` rejects
//! incoherent limits and bounds allocations at simultaneous saturation.
use crate::Fact;
use crate::calls::Call;
use crate::domain::Alarm;
use skein_lib::{Deadlines, Duration, Id, Map, Queue, Slab, Token};
/// Caps on retained calls, live rows, writes and reply bytes.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Limits {
    /// Logical calls retained, including queued and in flight.
    pub pending: u32,
    pub calls: u32,
    pub rate: u32,
    pub reserve: u32,
    pub window: Duration,
    /// Aggregate owned allocation bytes per operation, including row storage.
    pub op_bytes: u32,
    pub answer_bytes: u32,
    pub rows: u32,
    /// Distinct comments and reviews remembered per participating resource,
    /// each in its own bounded delivery stamp table.
    pub inbox: u32,
    pub read_attempts: u32,
    pub backoff: Duration,
    pub backoff_max: Duration,
    pub entries: u32,
    pub write_attempts: u32,
    /// Longest time a sent write may still arrive at Forgejo.
    pub lifetime: Duration,
    /// Allowance for the wall clock moving forward between hosts or restarts.
    pub clock_margin: Duration,
    pub resources: u32,
    pub repositories: u32,
    pub poll: Duration,
    pub poll_max: Duration,
    pub hinted: Duration,
    pub slow: Duration,
    pub facts: u32,
}
/// Retained operations, four class queues, budget timers and diagnostics.
/// A sent operation is cloned from its bounded retained retry value; its
/// outward copy belongs to the parent's output queue, counted there.
#[must_use]
pub fn worst_case(l: &Limits) -> Option<u64> {
    if l.pending == 0
        || l.calls == 0
        || l.calls > l.pending
        || l.rate == 0
        || l.reserve > l.rate
        || l.window == Duration::ZERO
        || l.read_attempts == 0
        || l.backoff == Duration::ZERO
        || l.backoff > l.backoff_max
        || l.entries == 0
        || l.write_attempts == 0
        || l.lifetime == Duration::ZERO
        || l.clock_margin == Duration::ZERO
        || l.rows == 0
        || l.inbox == 0
        || l.resources == 0
        || l.repositories == 0
        || l.poll == Duration::ZERO
        || l.poll > l.poll_max
        || l.hinted == Duration::ZERO
        || l.slow == Duration::ZERO
    {
        return None;
    }
    Slab::<Call>::worst_case(l.pending)?
        .checked_add(Map::<Token, Id<Call>>::worst_case(l.calls)?)?
        .checked_add(Queue::<Id<Call>>::worst_case(l.pending)?.checked_mul(4)?)?
        .checked_add(u64::from(l.pending).checked_mul(u64::from(l.op_bytes))?)?
        .checked_add(Deadlines::<Alarm>::worst_case(
            l.pending.checked_add(l.entries)?.checked_add(l.resources)?.checked_add(l.repositories)?.checked_add(2)?,
        )?)?
        .checked_add(crate::outbox::worst_case(l)?)?
        .checked_add(crate::keep::worst_case(l)?)?
        // Deployment namespace and authenticated writer table share one cap.
        .checked_add(u64::from(l.op_bytes))?
        // Owner names and in-flight operation copies are
        // independently owned; bound them even at simultaneous saturation.
        .checked_add(u64::from(l.pending).checked_mul(u64::from(l.op_bytes).checked_mul(2)?)?)?
        .checked_add(Queue::<Fact>::worst_case(l.facts)?)
}
