//! Compatibility import paths for the shared in-memory checkout kit
//! (skein's docs/design/fake-checkout.md, sections 1–4;
//! testing-strategy.md, section 4.3).
//! This crate keeps no state or mechanics: skein owns files, scripted commands
//! and local git, while temper's worlds retain remote policy, clocks, delivery
//! and cancellation. The explicit exports preserve the existing entry points
//! and their synchronous outcomes without duplicating the implementation.

pub mod git;

pub use skein_fake_checkout::{
    Checkout, Exit, Expect, Failure, Found, Kind, Listing, Process, Program, Searched, in_git,
};
