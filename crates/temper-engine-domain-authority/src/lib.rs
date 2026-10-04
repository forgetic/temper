//! What a task may do, as an owned value with a preorder, and its carved
//! funding (domain/authority.md, sections 3–7; programming-model.md, 4.5).
//!
//! Pure policy over values supplied by the engine's root: resource coverage,
//! authority order and accounting over snapshots, keeping no task, funding
//! numbers, connector, clock or configuration state. The tasks child keeps
//! the numbers; accounting answers say what they become. Connector kind orders
//! are supplied as validated [`Implies`] data. Checks allocate nothing and
//! scan only the bounded values their caller has admitted.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

mod numbers;
mod order;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_numbers;
mod value;

pub use numbers::{Charged, Funder, Funding, Moved, Numbers, carve, charge, left, move_funding, settle};
pub use order::{Implication, Implies, at_most, grant_at_most, grant_covers, pattern_at_most, pattern_covers};
pub use value::{Authority, Budget, Delegation, Executor, Grant, Last, Name, Pattern, Scopes, Tools};
