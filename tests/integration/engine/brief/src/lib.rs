//! A simulated world for the engine's brief sub-model (programming-style.md,
//! 4.5 and 11; engine-model.md, section 9): the brief, with the world as its
//! parent, driven by one loop, deterministically from a seed.
//!
//! The world owns the clock and the seeds, and stands in for everything
//! around the brief: the top level that asks for briefs (of every kind of
//! section, some required, some past the limits, faster than they are
//! answered), and the forge sub-model, the notes sub-model and the
//! configuration behind it, as sources that serve each read after a
//! latency with content drawn (text of one- to four-byte characters, in
//! parts), cut to the read's bounds from the end the read keeps, more than a
//! brief's budget often; some late past a brief's deadline, some failing,
//! and some past the read's bounds.
//!
//! It checks the contracts as it goes: every brief is answered exactly once,
//! and every read ended exactly once. Its referee ([`referee`]) holds the
//! brief to what the scenarios expect, from what the parent asks, what the
//! sources serve and what the brief answers: the sections in the order
//! asked, each with text exactly when it was read in time, none over its
//! budget and all within the brief's, every cut told with the right count
//! and the content passed through unchanged around it, failures and
//! refusals only when they are due, and every brief answered within its
//! time. And the invariants once it settles: nothing in flight, no brief,
//! read or deadline left in the brief, and the referee's verdict passed.
//! The world bounds its own state: deliveries in flight, trace lines and
//! iterations, past which it fails with the seed.

pub mod referee;
mod world;

pub use temper_world::Span;
pub use world::{ENDINGS, KINDS, LIMITS, Settings, Stats, World};
