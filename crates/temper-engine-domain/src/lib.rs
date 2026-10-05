//! The engine's new root, built beside the legacy root (domain/engine.md).
//! [`engine`] owns the walking root's real tasks, authority, people, fleet,
//! brief and accounts. Child writes and synchronous handoffs form one decision;
//! assignments, turns, terminals and results pass through the durable barrier.
#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]
extern crate alloc;
mod decision;
pub mod engine;
pub mod loads;
mod store;
#[cfg(test)]
mod tests;
pub use decision::{
    Decision, Delivery, Journal, Limits as JournalLimits, Output, accept, committed, fresh, resume, takes, uncommitted,
    worst_case,
};
pub use store::{Deployment, Family, Key, Range, Record, TurnRecord, Write, record_bytes};
