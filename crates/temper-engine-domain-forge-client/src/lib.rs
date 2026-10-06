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
    RecoveryClock, RepositoryRecord, Request, Resource, Stored, Watch, What,
};
pub use domain::{Domain, fire, max_out, resume, step};
pub use facts::{Fact, Priority};
pub use identity::{Config, Writer};
pub use limits::{Limits, worst_case};
