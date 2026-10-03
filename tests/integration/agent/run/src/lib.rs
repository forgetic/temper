//! A simulated world for the agent's run child domain (programming-model.md,
//! 4.5): the runs, with the world as their parent, a scripted host starting
//! them, and a scripted partner playing their conversations, driven by one
//! loop, deterministically from a seed.
//!
//! The world owns the clock and the seeds, and stands in for everything around
//! the run: the top level that will route its conversations to sessions, the
//! sessions and their LLMs ([`partner`]), the worker with both protocol layers
//! and the channel between it and the agent ([`host`]), and io, which makes
//! the checkouts' files and runs their checks. Both scripts speak the run's
//! vocabulary, as the top level will once it translates its neighbours'. The
//! host takes liberties a worker does not, to reach every state of a run: it
//! cancels runs twice, and after they have answered.
//!
//! The world checks the boundary contracts as it goes (one answer per start,
//! `Started` first and one `Ended` per open, `Say` only to a yielded
//! conversation, one end per host call, spending within the budget and one
//! turn) and the universal invariants once it settles (no live entities,
//! nothing in flight, every start answered, every turn spent in exactly one
//! answer).

pub mod host;
mod noisy;
pub mod partner;
mod world;

pub use noisy::noisy;
pub use temper_world::Span;
pub use world::{Checkouts, Settings, Stats, World};
