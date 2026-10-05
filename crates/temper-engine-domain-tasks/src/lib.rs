//! The tasks child domain: atomic batches, dependencies and lifecycle
//! (programming-model.md, 4.5; domain/tasks.md, sections 2, 4 and 5).
//! Carries its own authority values and never judges them. The root checks
//! authority and issues all task/attempt numbers. Save/Erase and outward
//! outputs are emitted together; durability is the parent's (engine.md, 5.6).
#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]
extern crate alloc;
mod batch;
mod boundary;
mod closing;
mod domain;
mod facts;
mod failures;
mod limits;
mod run;
mod stored;
#[cfg(test)]
mod tests;
mod value;
pub use boundary::*;
pub use domain::{Domain, fire, max_out, step};
pub use facts::Fact;
pub use failures::{Class, Retries, Retry, Tries};
pub use limits::{Limits, worst_case};
pub use value::{
    Authority, AuthorityExecutor, Budget, Delegation, Funder, Grant, Last, Numbers, Pattern, Scopes, Tools,
};
