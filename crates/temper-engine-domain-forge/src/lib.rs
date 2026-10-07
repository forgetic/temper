//! The forge connector's top (domain/connectors.md, sections 2–7;
//! domain/forge.md, sections 3–12). It keeps adopted repositories, resource
//! holds, writer slots, subscriptions, procedure and projection rows, and
//! outbox entries. The client alone executes API calls; change and issues
//! decide policy without retained state.
//!
//! `step` accepts one parent or client event. Saves and erases join the
//! parent's decision. `Committed` is the only way a newly enqueued effect
//! reaches the client. `resume` and `fire` drive the client after earlier
//! saves have crossed that barrier. The top never knows tasks beyond numbers,
//! authorization grants or protocol wire formats.
#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]
extern crate alloc;
pub mod boundary;
mod domain;
mod limits;
#[cfg(test)]
mod tests;
pub use boundary::*;
pub use domain::{Domain, fire, max_out, resume, step};
pub use limits::{Limits, worst_case};
