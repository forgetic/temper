//! The brief child domain of the temper engine's domain layer
//! (programming-model.md, 4.5; engine-domain.md, sections 3 and 9): it
//! renders the brief of every run (agent-domain.md, 4.1) from typed sections
//! the step selects, within byte budgets. A section is the item and its
//! lineage, the comments since the run's last turn, the outcomes of its
//! dependencies, the CI failures on its pull request's head with their
//! output, review comments, its pull request against its base, its earlier
//! attempts, the plan's status, the index of the notes in its scope, or the
//! template it follows.
//! The new root also asks for a task's spec, contract and requester lineage
//! as [`Source::Task`], with its own budget (domain/engine.md, section 9).
//! The task remains first; the farthest lineage is cut first. Legacy sources
//! remain through the cutover, and are not inferred from a task number.
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
//! bytes or that it is missing and why), in the order they were asked for,
//! which the parent puts in the charter apart from the instructions the plan
//! writes: what the run needs to know reaches it as sections, never folded
//! into prose.
//!
//! Which sections a run's brief has follows from why it runs, as the plan
//! says when a run is due (engine-domain.md, 5.3; the plan's `Sections`), and
//! the parent marks as required the sections the run is for:
//!
//! ```text
//! the run                         its sections                            required
//! any                             item, comments, attempts, notes;        item
//!                                 dependencies, if it has any;
//!                                 template, if it follows one
//! an agent step that may grow     and plan
//! a session's turn, supervising   and plan
//! a repair: CI failed             and ci                                  ci
//! a repair: changes asked for     and reviews                             reviews
//! a repair: base moved, conflict  and pull                                pull
//! a review at a head              and reviews
//! ```
//!
//! A brief that fails says which required section it lacked and why (its
//! read failed, brought more than a read may, or had not ended in time):
//! the parent decides whether the run waits for another try.
//!
//! Sans-io: [`step`] and [`fire`] turn events into requests and change
//! nothing but the [`Domain`] they are given. Every effect is a [`Request`]
//! that its parent, the engine's root domain (`temper-legacy-engine-domain`),
//! routes on: a read to the forge child domain, the notes child domain or its
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
//! queue the parent drains ([`Domain::pop_fact`]); what does not fit is
//! dropped and counted, and nothing the brief decides depends on it.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

mod boundary;
mod brief;
mod cut;
mod domain;
mod facts;
mod limits;
mod planned;
#[cfg(test)]
mod tests;

pub use boundary::{
    Body, Commit, Event, Fit, Item, Keep, Kind, Part, Read, Refusal, Request, Section, Source, TaskPart, Unread, Wanted,
};
pub use domain::{Domain, fire, max_out, step};
pub use facts::{Fact, Gathered};
pub use limits::{Budgets, Limits, worst_case};
pub use planned::{ConnectorAction, Core, Placement, Plan, Planned, plan};
