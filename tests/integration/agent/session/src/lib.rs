//! A simulated world for the agent's session sub-model (programming-model.md,
//! 4.5): the sessions, with the world as their parent, and a fake LLM
//! provider's model, driven by one loop, deterministically from a seed.
//!
//! The world owns the clock and the seeds, and stands in for everything
//! between the two models: both protocol layers, both io layers, the network,
//! and the tools. It is the only code that knows both vocabularies, as a
//! protocol crate is the only one that sees both io's and the model's
//! ([`translate`]). It checks the boundary contracts as it goes (one terminal
//! event per request, one reply per call) and the universal invariants once it
//! settles (no live entities, nothing in flight, every run answered).

pub mod translate;
mod world;

pub use world::{Settings, Span, Stats, World, task};
