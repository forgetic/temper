//! Temper's sized, domain-independent wire protocol (channel.md).
#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]
extern crate alloc;

pub mod codec;
mod kinds;
pub mod machine;
mod memory;
pub mod payload;
mod primitives;
pub mod sizes;
pub mod wire;

pub use sizes::{Limits, Sizes};

#[cfg(test)]
mod tests;

#[cfg(test)]
mod golden;
