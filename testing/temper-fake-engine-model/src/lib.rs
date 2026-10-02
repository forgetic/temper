//! The model layer of a fake engine, for simulations.
//!
//! An engine seen from the inside, as much of it as a worker meets
//! (engine-model.md, section 8; worker-model.md, sections 2 and 9). It has
//! work items to run, drawn from its configuration (see the `workspace` and
//! `charter` modules), and assigns each to a worker that has said hello,
//! within the slots the worker reported, and sometimes past them. It decides
//! what follows a run (engine-model.md, 4.2 to 4.4, simplified): an ended run
//! is recorded; a failed one is retried with a new attempt, by chance and
//! within a bound; a parked one is woken later, resumed from its snapshot or
//! started fresh from its saved work (section 6). Meanwhile it sends live runs
//! inbound events, cancels some of them, answers their relayed calls, and
//! sends some traffic that names attempts already answered or replaced, which
//! a worker must drop. When the world says a worker's channel dropped, it
//! waits out its grace; on the worker's next hello it keeps or cancels each
//! run the worker reports, and presumes lost, and retries, the runs it does
//! not hear about, or does not hear from in time.
//!
//! It checks the worker as it goes, by asserting: every assignment is
//! answered exactly once per attempt, unless the attempt was presumed lost;
//! nothing arrives for an attempt it never made; a worker reports only the
//! attempts it was given; a run names its relayed calls apart. Facts are only
//! counted. A world reads its [`Tally`] at settle.
//!
//! Its vocabulary ([`api`]) is its own: it shares nothing with the worker's
//! model. Between the two sits a protocol layer on each side, or a simulator
//! standing in for both. It stands in for the engine's model until that
//! exists.
//!
//! It follows the same programming model as any other step crate.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod api;
mod charter;
mod fleet;
mod model;
mod traffic;
mod work;
mod workspace;

#[cfg(test)]
mod tests;

pub use model::{Config, Endings, Event, MAX_OUT, Model, Origin, Request, Tally, fire, resume, step};
pub use workspace::{BASE, COMMIT, IDENTITY};
