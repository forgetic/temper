//! The brief sub-model of the temper engine's model layer
//! (programming-style.md, 4.5; engine-model.md, sections 3 and 9): it
//! renders the brief of every run (agent-model.md, 4.1) from typed sections
//! the step selects, within byte budgets. A section is the item and its
//! lineage, the comments since the run's last turn, the outcomes of its
//! dependencies, the CI failures on its pull request's head with their
//! output, review comments, its pull request against its base, its earlier
//! attempts, the plan's status, the index of the notes in its scope, or the
//! template it follows.
//!
//! For each brief its parent asks for, the brief asks its parent to read
//! every section's content (forge reads on demand, the notes' index, a
//! template from the configuration: "everything else on demand", section
//! 12), each within what a read may bring; takes the content as it arrives;
//! and renders the brief once the last read has ended, or with what it has
//! when its time runs out, a section not read in time missing. A brief
//! without a section its parent marked required fails instead. Rendering
//! cuts each section to its kind's budget and all of them to the brief's,
//! losing first what matters least to that kind, and says in each section
//! where it cut and how many bytes it left out.
//!
//! The answer is the brief as typed sections ([`Section`]: a kind, and its
//! bytes or that it is missing), in the order they were asked for, which the
//! parent puts in the charter apart from the instructions the plan writes:
//! what the run needs to know reaches it as sections, never folded into
//! prose.
//!
//! Sans-io: [`step`] and [`fire`] turn events into requests and change
//! nothing but the [`Model`] they are given. Every effect is a [`Request`]
//! that its parent, the engine's top-level model (`temper-engine-model`),
//! routes on: a read to the forge sub-model, the notes sub-model or its
//! configuration, as the [`Source`] says, and the answer to whoever asked.
//! Their outcomes come back later through the parent as an [`Event`]. Each
//! brief's deadline is the brief's own.
//!
//! The brief knows nothing of plans, the forge's API or the workers: the
//! parent describes each section's source in the brief's terms, and the
//! content comes back as bytes the brief cuts but never parses. Briefs,
//! sections, parts and bytes are bounded, and a brief past them is refused
//! at the entrance.
//!
//! What happens is also told as content-free [`Fact`]s, kept in a bounded
//! queue the parent drains ([`Model::pop_fact`]); what does not fit is
//! dropped and counted, and nothing the brief decides depends on it.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

mod boundary;
mod brief;
mod cut;
mod facts;
mod limits;
mod model;
#[cfg(test)]
mod tests;

pub use boundary::{Body, Commit, Event, Item, Keep, Kind, Part, Read, Refusal, Request, Section, Source, Wanted};
pub use facts::{Fact, Gathered};
pub use limits::{Budgets, Limits, worst_case};
pub use model::{Model, fire, max_out, step};
