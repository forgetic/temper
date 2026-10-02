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
//!   Forgejo protocol layer puts in what the engine creates (keys, records)
//!   and finds again, payloads filled in from the parent's tokens, and a
//!   deadline per call, past which the call times out and its late answer is
//!   dropped; webhooks, as hints;
//! - **the parent** ([`parent`]), scripted: the engine's top level, taking
//!   in what is handed in, running items' news, and writing what the runs
//!   answer: records, labels, replies, tasks, changes and their pull
//!   requests, merges, closes, notes; restarted with the engine;
//! - **people** ([`people`]) acting on the forge as forge users: opening,
//!   commenting, labelling and unlabelling, reviewing, pushing, closing, and
//!   mangling a record by hand; and the workers that push a change's branch;
//! - **the engine restarting**, injected by the referee: a new sub-model,
//!   starting cold, while the calls the old one made still land.
//!
//! The fake forge's faults are on as the settings say: latency and late
//! answers, failures before and after a call is made, rate limits, and
//! webhooks late or lost.
//!
//! It checks the contracts as it goes: every call is ended exactly once, in
//! the life of the sub-model that made it, and every fresh read and write
//! answered exactly once. Its referee ([`referee`]) holds the sub-model to
//! what the scenarios expect, from what the fake forge sees: no write the
//! parent did not plan, no creation made twice, labels as the last set
//! written, every change reaching the working set within the polling bound
//! whatever webhooks are lost, and no call before a rate limit's reset. And
//! the invariants once it settles: nothing in flight, no call, read or write
//! left in the sub-model, and the referee's verdict passed.

pub mod parent;
pub mod people;
pub mod referee;
pub mod translate;
mod world;

pub use temper_world::Span;
pub use world::{ENDINGS, ENGINE, HAND_IN, LABELS, MAIN, REPOSITORIES, Settings, Stats, TRACKING, World};
