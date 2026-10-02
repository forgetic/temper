//! The notes sub-model of the temper engine's model layer
//! (programming-style.md, 4.5; engine-model.md, sections 3 and 10): what
//! temper learns over time and passes on, kept in the forge's wiki, a page
//! per entry. It holds the index of each scope in use (the deployment's, a
//! repository's, a goal's), one line per entry (its name, description, who
//! wrote it, what it refers to), learned from the wiki: it asks for a
//! scope's list of pages and the pages it does not know, and hears of
//! changes. From the indexes it answers which notes are in a run's scopes,
//! as much of their index as fits a brief's budget (saying how many more
//! there are) and what a search of descriptions finds. It serves a run's
//! `recall` by reading the entries' pages afresh, and turns a `note` write
//! (an entry new, revised or removed) into the wiki operation it needs.
//!
//! Sans-io: [`step`] and [`resume`] turn events into requests and change
//! nothing but the [`Model`] they are given. Every effect is a [`Request`]
//! that its parent, the engine's top-level model (`temper-engine-model`),
//! routes on: the wiki operations go to the forge sub-model, the answers to
//! the brief or the run that asked. Their outcomes come back later through
//! the parent as an [`Event`]. The notes own no timers: the forge sub-model
//! bounds each operation, and its keeping up with the forge hints at what
//! changed.
//!
//! The notes know nothing of plans or rules: whether a note of wide scope
//! needs a person's acceptance is the rules' to say, asked by the top level
//! before the write reaches the notes. Scopes, entries, calls and bytes are
//! bounded, and a call past them is refused at the entrance.
//!
//! What happens is also told as content-free [`Fact`]s, kept in a bounded
//! queue the parent drains ([`Model::pop_fact`]); what does not fit is
//! dropped and counted, and nothing the notes decide depends on it.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

mod boundary;
mod call;
mod facts;
mod kept;
mod limits;
mod model;
#[cfg(test)]
mod tests;

pub use boundary::{
    Author, Change, Entry, Event, Fetched, Item, Line, Listed, Noted, Page, Recall, Reference, Refusal, Request, Scope,
    Scopes, Wrote,
};
pub use facts::Fact;
pub use limits::{Limits, worst_case};
pub use model::{MAX_OUT, Model, max_out, resume, step};
