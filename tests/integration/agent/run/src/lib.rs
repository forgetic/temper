//! A simulated world for the agent's run sub-model (programming-model.md,
//! 4.5): the runs, with the world as their parent, a fake worker's model
//! starting them, and a scripted partner playing their conversations, driven
//! by one loop, deterministically from a seed.
//!
//! The world owns the clock and the seeds, and stands in for everything around
//! the run: the top level that will route its conversations to sessions, the
//! sessions and their LLMs ([`partner`]), and both protocol layers and the
//! channel between the agent and the worker ([`translate`]). It checks the
//! boundary contracts as it goes (one answer per start, `Started` first and
//! one `Ended` per open, `Say` only to a yielded conversation, spending within
//! the budget and one turn) and the universal invariants once it settles (no
//! live entities, nothing in flight, every start answered, every turn spent
//! in exactly one answer).

pub mod partner;
pub mod translate;
mod world;

pub use temper_world::Span;
pub use world::{Checkouts, Settings, Stats, World};
