//! A simulated world for the agent's session sub-model (programming-style.md,
//! 4.5): the sessions, with the world as their parent, and a fake LLM
//! provider's model, driven by one loop, deterministically from a seed.
//!
//! The world owns the clock and the seeds, and stands in for everything
//! around the two models: the sessions' opener, which nudges a session that
//! yields a few times and then closes it, or closes it at a moment of its own;
//! both protocol layers, both io layers and the network. The sessions own the
//! real tools, whose file operations and commands the world runs as io would
//! on a fake checkout ([`temper_checkout_fake`]), seeded with a repository for
//! each session ([`fixture`]). It is the only code that knows both
//! vocabularies, as a protocol crate is the only one that sees both io's and
//! the model's ([`translate`]). It checks the boundary contracts as it goes
//! (one terminal event per request, one end per open, a continue only to a
//! yielded session, a yield or an end only with nothing in flight, a write
//! alone and reads no wider than the limits, no completion started past a
//! session's budget, an end that adds up what was used, each result of the
//! tools what comes of its own call) and the universal invariants once it
//! settles (no live entities, every kit closed, nothing in flight, every
//! operation ended once, every session ended, and facts that add up to what
//! crossed the boundary unless some were dropped).

pub mod fixture;
mod noisy;
pub mod tickets;
pub mod translate;
mod world;

pub use noisy::{noisy, submit_noisily};
pub use temper_world::Span;
pub use world::{BUDGET, Count, Ended, Session, Settings, Stats, TOOLS, Told, World, spec};
