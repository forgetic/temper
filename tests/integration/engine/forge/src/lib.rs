//! A simulated world for the engine's forge sub-model (programming-style.md,
//! 4.5 and 11; engine-model.md, section 12; testing-pyramid.md, 2.2): the
//! forge sub-model, with the world as its parent, against the fake forge
//! (`testing/temper-forge-model`), driven by one loop, deterministically from
//! a seed.
//!
//! The world owns the clock and the seeds, and stands in for everything
//! around the sub-model:
//!
//! - **the protocol layer** ([`translate`]): the sub-model's calls as the
//!   fake's, the fake's answers as the sub-model's, with the markers a
//!   Forgejo protocol layer puts in what the engine creates (keys, records,
//!   nonces, the person a message is written for) and finds again, payloads
//!   filled in from the parent's tokens, reviews and statuses paged, CI
//!   combined, a rate limit's reset as a wait, and a deadline per call, past
//!   which the call times out and its late answer is dropped; webhooks, as
//!   hints;
//! - **the parent** ([`parent`]), scripted: the engine's top level, taking
//!   in what is handed in, running items' news, and writing what the runs
//!   answer: records, labels it owns, outcomes and what they cause (replies,
//!   messages for a person, tasks the item then depends on, verdicts),
//!   changes and their pull requests, reviewers, merges, closes, notes at the
//!   revision read; restarted with the engine, asking again after their
//!   causes for the creations it had in hand;
//! - **people** ([`people`]) acting on the forge as forge users: opening,
//!   commenting, labelling and unlabelling, reviewing (some reviews pending,
//!   submitted later), pushing, closing, and mangling or deleting a record by
//!   hand; and the workers that push a change's branch;
//! - **the engine restarting**, injected by the referee: a new sub-model,
//!   starting cold, while the calls the old one made still land.
//!
//! The fake forge's faults are on as the settings say: latency and late
//! answers, failures before and after a call is made, calls that land after
//! they timed out, rate limits, webhooks late or lost, CI with two contexts
//! that may run again, and a clock ahead of or behind the world's.
//!
//! It checks the contracts as it goes: every call is ended exactly once, in
//! the life of the sub-model that made it, and every fresh read and write
//! answered exactly once. Its referee ([`referee`]) holds the sub-model to
//! what the scenarios expect, from what the fake forge sees: no write the
//! parent did not plan (or no longer may land), no creation made twice,
//! labels as the last set written and only those the engine owns, every
//! change reaching the working set within the polling bound whatever
//! webhooks are lost (comments on an item and its pull request, labels,
//! closes, its pull request's head and CI, a reviewer's verdict, a hand-in,
//! and after a restart an item only the slow pass finds), and no call before
//! a rate limit's reset. And
//! the invariants once it settles: nothing in flight, no call, read or write
//! left in the sub-model, and the referee's verdict passed. Its own state is
//! bounded too: a world that grows its trace or its deliveries past their
//! bounds, or does not settle in the iterations it is given, fails with its
//! seed.

pub mod parent;
pub mod people;
pub mod referee;
pub mod translate;
mod world;

pub use temper_world::Span;
pub use world::{
    ENDINGS, ENGINE, HAND_IN, LABELS, MAIN, OWNED, REPOSITORIES, Settings, Stats, TRACKING, WAITING, WORKING, World,
};
