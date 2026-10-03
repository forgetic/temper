//! A simulated world for the engine's brief sub-model (programming-model.md,
//! 4.5 and 11; engine-model.md, section 9): the brief, with the world as its
//! parent, driven by one loop, deterministically from a seed.
//!
//! The world owns the clock and the seeds, and stands in for everything
//! around the brief: the top level that asks for briefs (of every kind of
//! section, some required, some past the limits, with lists of dependencies
//! longer than a source may name, faster than they are answered), and the
//! forge sub-model, the notes sub-model and the configuration behind it, as
//! sources that serve each read after a latency with content drawn (text of
//! one- to four-byte characters, in parts; an index of lines for the notes),
//! fitted to the read's bounds as the read asks, more than a brief's budget
//! often; some late past a brief's deadline, some just at it, some failing
//! and some past the read's bounds. A random world draws the brief's limits
//! too, to their tightest now and then.
//!
//! It checks the contracts as it goes: every brief is answered exactly once,
//! and every read ended exactly once. Its referee ([`referee`]) holds the
//! brief to what the scenarios expect, from what the parent asks, what the
//! brief reads and what its sources serve, and what it answers, each seen as
//! the step that made it ends: the entrance (busy exactly when there is no
//! room, and room told when it is made), each read as its section is cut,
//! each answer as soon as it is due, each section rebuilt from its parts and
//! compared byte for byte, its order kept, its budget and its share of the
//! brief's, and every brief answered within its time. And the invariants
//! once it settles: nothing in flight, no brief, read or deadline left in
//! the brief, and the referee's verdict passed. The world bounds its own
//! state: deliveries in flight, trace lines and iterations, past which it
//! fails with the seed.

pub mod referee;
mod world;

pub use temper_world::Span;
pub use world::{ENDINGS, KINDS, LIMITS, Settings, Stats, World};
