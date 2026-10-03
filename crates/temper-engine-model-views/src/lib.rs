//! The views sub-model of the temper engine's model layer
//! (programming-model.md, 4.5; engine-model.md, sections 2, 3 and 11): what
//! runs report (agent-model.md, section 7), which reaches the engine through
//! the workers best effort (worker-model.md, section 7), turned into what
//! people and the engine's store see. Nothing the engine decides depends on
//! it.
//!
//! - **Live streams.** People watch a run, an item or a repository's board,
//!   through the parent, from a snapshot the parent gives, delivered first.
//!   What a watched run reports (a session's text as it
//!   is written, its progress, its calls and tools) is streamed to each
//!   watcher of the run and of its item, and an item's phase as it changes
//!   to each watcher of the item and of its board. A watcher has one
//!   delivery in flight; what comes meanwhile waits in its backlog, which is
//!   bounded: when it overflows, what waits is dropped, and the watcher is
//!   told how much it missed with the next delivery, which catches it up
//!   from there. What its stream did not take in time, and a report it would
//!   have had that was dropped, are told as missed the same way. A slow
//!   watcher never holds the engine up, and never grows.
//! - **Traces.** What runs report is kept in the engine's store for a
//!   retention period, with as much of each report as the run's capture
//!   policy says ([`Policy`]: of each kind of report, nothing, its shape, or
//!   its content too), which the engine sends with each assignment. Reports
//!   are batched in memory, bounded, and appended to the store through the
//!   parent; a periodic sweep asks the store to expire what is past the
//!   retention. Traces are expendable: a batch the store fails to take, or
//!   that finds no room, is dropped and counted, never retried.
//!
//! Sans-io: [`step`] and [`fire`] turn events into requests and change
//! nothing but the [`Model`] they are given. Every effect is a [`Request`]
//! that its parent, the engine's top-level model (`temper-engine-model`),
//! routes on: a watcher's deliveries to the person's stream, the store's
//! operations to the store. Their outcomes come back later through the parent
//! as an [`Event`]. The batch's flush and the store's sweep are the views'
//! own deadlines.
//!
//! The views know nothing of plans, the forge or the workers: runs, items and
//! watchers are tokens the parent gives, and what a run reports is bytes the
//! views pass on and keep but never parse, beyond the [`Kind`] the parent
//! gives with them. Runs, watchers, backlogs and batches are bounded; a watch
//! past them is refused at the entrance, and a report past them dropped and
//! counted.
//!
//! What happens is also told as content-free [`Fact`]s, kept in a bounded
//! queue the parent drains ([`Model::pop_fact`]); what does not fit is
//! dropped and counted, and nothing the views decide depends on it. What the
//! views lose is counted in [`Lost`] ([`Model::lost`]).

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

mod boundary;
mod facts;
mod limits;
mod model;
#[cfg(test)]
mod tests;
mod trace;
mod watch;

pub use boundary::{Capture, Chunk, End, Event, Kind, Policy, Record, Refusal, Request, Subject};
pub use facts::{Dropped, Fact, Kept, Lost};
pub use limits::{Limits, worst_case};
pub use model::{Model, fire, max_out, step};
