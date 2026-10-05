//! The engine's new root, built beside the legacy root (domain/engine.md).
//! The first seam is the ordered commit barrier: a decision gathers writes
//! and owned deliveries, then releases those deliveries only after durability.
//! The child routing and walking skeleton are still being added in step 06a.
#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]
extern crate alloc;
mod decision;
mod store;
#[cfg(test)]
mod tests;
pub use decision::{
    Decision, Delivery, Journal, Limits, Output, accept, committed, fresh, resume, takes, uncommitted, worst_case,
};
pub use store::{Deployment, Family, Key, Record, TurnRecord, Write};
