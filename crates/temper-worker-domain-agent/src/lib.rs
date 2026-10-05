//! The agent child domain of the temper worker's domain layer
//! (programming-model.md, 4.5; worker-domain.md, sections 3 and 6): the
//! worker's agent processes, one per hosted run, each with one channel over its
//! pipes. It spawns an agent in a contained process tree with what its run
//! starts with; speaks the channel (agent-domain.md, section 8) and holds the
//! run to its rules, passing the run's host calls, facts and finish up to its
//! client, and the client's inbound events, answers and cancel down to the run;
//! watches the run's progress and its wall time; and stops an agent by cancel,
//! then kill, telling the client it has gone only once io has proved that its
//! process exited and its tree is empty.
//!
//! Sans-io: [`step`] and [`fire`] turn events into requests and change nothing
//! but the [`Domain`] they are given. Time is an input; every effect, from
//! spawning a process to telling the client how its run finishes, is a
//! [`Request`] that its parent, the worker's root domain
//! (`temper-worker-domain`), routes on, and its outcome comes back later
//! through the parent as an [`Event`].
//!
//! The agent child domain knows agent processes, the channel's rules, the
//! watchdog and the wall time, and cancel then kill; nothing of git,
//! workspaces, the engine, or what a host call means. Its clients are opaque
//! owners (its parent translates to and from the host child domain's
//! vocabulary, as siblings share no types, 4.5), and what it passes through is
//! opaque bytes, bounded at its entrance: the charter and snapshot, inbound
//! events, host calls and their answers, the run's facts and outcome
//! ([`channel`]). What it acts on is typed: process lifecycle, the run's
//! progress, waiting and long operations, and how it finishes.
//!
//! What happens is also told as content-free [`Fact`]s, kept in a bounded
//! queue the parent drains ([`Domain::pop_fact`]); what does not fit is dropped
//! and counted, and nothing the agent child domain decides depends on it.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

mod agent;
mod boundary;
pub mod channel;
mod domain;
mod facts;
mod limits;
mod push;
#[cfg(test)]
mod tests;

pub use boundary::{Bounce, End, Event, Fault, Invalid, Request, Signal, Spawn, SpawnV2};
pub use domain::{Domain, MAX_OUT, fire, step};
pub use facts::Fact;
pub use limits::{Limits, worst_case};

pub use push::{PushDiagnostic, PushFailure, PushReason};
