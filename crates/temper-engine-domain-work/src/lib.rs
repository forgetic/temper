//! The work child domain of the temper engine's domain layer
//! (programming-model.md, 4.5; engine-domain.md, sections 3 and 4): the
//! engine's hub, as the host is the worker's. It knows an item's lifecycle
//! (waiting, due, claimed, running, applying, held, done, and parked runs) and
//! keeps, per item taken in, its claim and attempts, its failures per class
//! with their backoff, the outcome being applied, and at most one run in
//! flight. It asks its parent for everything it cannot do, and hears the
//! results: what is due (the plan), a record written, an outcome posted or
//! applied, an engine action made (the forge, the plan and the rules), a run
//! started or adopted, placed, cancelled or answered (the fleet), and people's
//! stops and releases.
//!
//! Its part of an item's record ([`Lifecycle`]) is written at every step that
//! must outlive the process: the claim before the run starts, the outcome
//! being applied before its writes, and the commit point after them (4.4); a
//! run's answer is acknowledged only once it is there. A restart is a cold
//! start that reads what is live (section 12), and the hub is rebuilt from
//! the records alone, fed to it as they are read ([`Event::Take`]), before
//! and after it is running normally: a claim read is adopted, an application
//! read is resumed.
//!
//! Sans-io: [`step`] and [`fire`] turn events into requests and change
//! nothing but the [`Domain`] they are given. Every effect is a [`Request`]
//! that its parent, the engine's root domain (`temper-engine-domain`), routes
//! on, and its outcome comes back later through the parent as an [`Event`]. The
//! hub owns its timers: an item's wake, and the backoff before a retry or a
//! claim made again (its jitter drawn from a seeded `Rng`). It arms none on a
//! run: the fleet runs the races on one, its grace among them.
//!
//! The hub knows nothing of the forge's API, the workers' channels or a plan's
//! vocabulary: items are named by their forge names, plain data; runs by their
//! item and attempt, as the fleet names them; what it passes on without reading
//! (the spec of a run, an engine action's writes, an outcome, a snapshot, an
//! inbox event) by tokens its parent issues; what it writes into a record
//! (a plan's reason to hold among it) is typed, never a token. Items are
//! bounded, and one past the working set is refused at the entrance.
//!
//! What happens is also told as content-free [`Fact`]s, kept in a bounded
//! queue the parent drains ([`Domain::pop_fact`]); what does not fit is dropped
//! and counted, and nothing the hub decides depends on it.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

mod boundary;
mod domain;
mod facts;
mod limits;
#[cfg(test)]
mod tests;
mod tracked;

pub use boundary::{
    Acted, Answer, Applied, Class, Due, Event, Failures, Hold, Item, Lifecycle, Phase, Read, Refusal, Request, Then,
    Wrote,
};
pub use domain::{Domain, fire, max_out, step};
pub use facts::Fact;
pub use limits::{Limits, Retries, Retry, worst_case};
