//! What a task may do, as an owned value with a preorder
//! (domain/authority.md, sections 3–5; programming-model.md, 4.5).
//!
//! Pure policy over values supplied by the engine's root: this increment
//! decides resource coverage and authority order, keeping no task, funding
//! numbers, connector, clock or configuration state. Connector kind orders
//! are supplied as validated [`Implies`] data. Checks allocate nothing and
//! scan only the bounded values their caller has admitted.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

mod order;
#[cfg(test)]
mod tests;
mod value;

pub use order::{Implication, Implies, at_most, grant_at_most, grant_covers, pattern_at_most, pattern_covers};
pub use value::{Authority, Budget, Delegation, Executor, Grant, Last, Name, Pattern, Scopes, Tools};
