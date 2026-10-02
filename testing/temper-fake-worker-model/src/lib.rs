//! The model layer of a fake worker, for simulations.
//!
//! A worker seen from the inside: it has jobs to run, and starts an agent run
//! for each at a time drawn from its configuration, on a charter drawn the
//! same way (see the `charter` module). It cancels some of the runs once the
//! agent has admitted them, serves the pushes they ask for, and takes each
//! run's one answer. Its requests go
//! down to its protocol layer as [`Request`]s, and what the agent says comes
//! back up as [`Event`]s.
//!
//! Its vocabulary ([`api`]) is its own: it shares nothing with the agent's
//! model. Between the two sits a protocol layer on each side, or a simulator
//! standing in for both.
//!
//! It follows the same programming model as any other step crate.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod api;
mod charter;
mod model;

pub use model::{Config, Event, MAX_OUT, Model, Request, fire, step};
