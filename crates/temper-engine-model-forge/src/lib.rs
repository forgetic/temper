//! The forge sub-model of the temper engine's model layer
//! (programming-style.md, 4.5; engine-model.md, section 12): the engine's
//! knowledge of the forge and its only way to change it. It holds a working
//! set of live work, not a copy of the forge: the items the engine tracks
//! that are not done, where each one's record is and the inbox position it
//! says, its labels, its pull request's head, CI and reviews, and the news
//! for its inbox since that position (4.3), derived from the forge's state
//! and so the same after a restart. What it holds and what it costs to keep up
//! grow with the work in progress, never with the forge's history.
//!
//! - **Starting** is cold: each repository's open items carrying the
//!   tracking label are listed, and read for their records, and its open
//!   issues carrying the hand-in label are offered to the parent (4.6).
//! - **Keeping up**: what changed since the last listing is listed again, a
//!   repository at a time, on a polling timer and sooner after a webhook,
//!   which is only ever a hint; the heads whose CI has not settled are read
//!   again; and a slow pass over all open items finds one whose tracking label
//!   a person removed ([`scans`]).
//! - **Fresh reads** for the parent, before its writes and for its runs and
//!   briefs: fetched, within the budget, ahead of everything else, and not
//!   kept.
//! - **Writes** are typed, serialised per item, retried with a jittered
//!   backoff, and repeat-safe: creations keyed and found by their keys, sets
//!   written as sets, a record read afresh before it is edited ([`writes`]).
//! - **The request budget** keeps it within the forge's rate limit, and
//!   honours the reset of a refusal: the parent's reads first, then writes,
//!   then keeping up, then the slow pass ([`calls`]).
//! - **The wiki**'s pages are read and written for the notes, in the same
//!   budget.
//!
//! Sans-io: [`step`], [`fire`] and [`resume`] turn events into requests and
//! change nothing but the [`Model`] they are given. Every call to the forge is
//! a [`Request::Call`] its parent, the engine's top-level model
//! (`temper-engine-model`), carries to the protocol layer, and its answer
//! comes back through the parent as an [`Event::Answered`]. Its vocabulary is
//! its own, forge-shaped ([`api`]); it knows nothing of plans, rules or runs,
//! nor what a record says beyond the inbox position: payloads it does not
//! interpret are named by the parent's tokens and filled in by the parent as
//! the call goes out.
//!
//! What happens is also told as content-free [`Fact`]s, kept in a bounded
//! queue the parent drains ([`Model::pop_fact`]); what does not fit is dropped
//! and counted, and nothing the sub-model decides depends on it.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod api;
mod boundary;
mod calls;
mod facts;
mod items;
mod limits;
mod model;
mod reads;
mod scans;
#[cfg(test)]
mod tests;
mod writes;

pub use boundary::{
    Ci, Content, Event, Failure, Item, Level, News, Position, Read, Record, Request, View, Write, Written,
};
pub use facts::{Fact, Priority};
pub use limits::{Limits, worst_case};
pub use model::{Config, Model, fire, max_out, resume, step};
