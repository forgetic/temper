//! The work sub-model of the temper engine's model layer (programming-style.md,
//! 4.5; engine-model.md, sections 3 and 4): the engine's hub, as the host is
//! the worker's. It knows an item's lifecycle (waiting, due, claimed, running,
//! applying, held, done, and parked runs) and keeps, per item taken in, its
//! claim and attempts, its failures per class with their backoff, the outcome
//! being applied, and at most one run in flight. It asks its parent for
//! everything it cannot do, and hears the results: what is due (the plan), a
//! record written, an outcome posted or applied, an engine action made (the
//! forge, the plan and the rules), a run started, cancelled or answered (the
//! fleet), and people's stops and releases.
//!
//! Its part of an item's record ([`Lifecycle`]) is written at every step that
//! must outlive the process: the claim before the run starts, the outcome
//! being applied before its writes, and the commit point after them (4.4). A
//! restart is a cold start that reads what is live (section 12), and the hub
//! is rebuilt from the records alone, fed to it as they are read
//! ([`Event::Take`]), before and after it is running normally.
//!
//! Sans-io: [`step`] and [`fire`] turn events into requests and change
//! nothing but the [`Model`] they are given. Every effect is a [`Request`] that
//! its parent, the engine's top-level model (`temper-engine-model`), routes on,
//! and its outcome comes back later through the parent as an [`Event`]. The
//! hub owns its timers: an item's wake, the backoff before a retry (its
//! jitter drawn from a seeded `Rng`), and the grace a claim read at a start
//! waits for a worker's word.
//!
//! The hub knows nothing of the forge's API, the workers' channels or a plan's
//! vocabulary: items are named by their forge names, plain data; runs by their
//! item and attempt, as the fleet names them; what it passes on without reading
//! (the spec of a run, an engine action's writes, an outcome, a snapshot, an
//! inbox event, the plan's reason to hold) by tokens its parent issues. Items
//! are bounded, and one past the working set is refused at the entrance.
//!
//! What happens is also told as content-free [`Fact`]s, kept in a bounded
//! queue the parent drains ([`Model::pop_fact`]); what does not fit is dropped
//! and counted, and nothing the hub decides depends on it.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

mod boundary;
mod facts;
mod limits;
mod model;
#[cfg(test)]
mod tests;
mod tracked;

pub use boundary::{
    Acted, Answer, Applied, Class, Due, Event, Failures, Hold, Item, Lifecycle, Phase, Read, Refusal, Request, Then,
    Wrote,
};
pub use facts::Fact;
pub use limits::{Limits, Retries, Retry, worst_case};
pub use model::{MAX_OUT, Model, fire, max_out, step};
