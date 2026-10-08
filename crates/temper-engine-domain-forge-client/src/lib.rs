//! Forge client for live reads, resource keeping, and effect execution
//! (domain/forge.md, sections 5–6; domain/connectors.md, section 4).
//!
//! Its state is the bounded working set, request budget, and in-flight write
//! lanes. It never knows task policy, grants, subscribers, or the top's durable
//! outbox. `step` accepts parent events, `fire` handles deadlines, and `resume`
//! sends eligible calls. The parent commits progress and outcomes before it
//! releases the calls produced by the same decision.
#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]
extern crate alloc;
pub mod api;
mod boundary;
mod bounds;
mod calls;
mod domain;
mod facts;
mod identity;
mod keep;
mod limits;
mod outbox;
#[cfg(test)]
mod tests;
pub use boundary::{
    Attempt, Cached, Condition, Delivery, Echo, Effect, Entry, Event, Key, LiveRecord, Made, Outcome, Position,
    Recovery, RecoveryClock, RepositoryRecord, Request, Resource, Stored, Watch, What, recovery,
};
pub use domain::{Domain, fire, max_out, resume, step};
pub use facts::{Fact, Priority};
pub use identity::{Config, EffectPurpose, Writer, effect_key};
pub use limits::{Limits, worst_case};
/// Checked deep payload bytes of one durable client row.
#[must_use]
pub fn stored_bytes(row: &Stored) -> Option<u64> {
    bounds::stored_bytes(row)
}
/// Checked deep payload bytes of one outbox effect.
#[must_use]
pub fn effect_bytes(effect: &Effect) -> Option<u64> {
    bounds::effect_bytes(effect)
}
/// Checked deep payload bytes of one typed provider answer.
#[must_use]
pub fn answer_bytes(answer: &api::Answer, limits: &Limits) -> Option<u64> {
    bounds::answer_bytes(answer, limits)
}
/// Checked deep answer payload bytes before caller-specific admission limits.
#[must_use]
pub fn answer_bytes_unbounded(answer: &api::Answer) -> Option<u64> {
    bounds::answer_bytes_unbounded(answer)
}
