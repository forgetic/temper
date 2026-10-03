//! A domain world for the agent's tools child domain (programming-model.md,
//! 4.5; testing-strategy.md, 2.2): the tools, with the world as their parent,
//! over a fake checkout, driven by one loop, deterministically from a seed.
//!
//! The world owns the clock and the seed, and plays everything around the
//! tools: the sessions, each opening a kit, running a script of calls against
//! it and closing it; and io, running the tools' file operations on the fake
//! checkout ([`temper_checkout_fake`]) with latency, faults, deadlines and
//! cancels. It is the only code that knows both vocabularies, as a protocol
//! crate is the only one that sees both io's and the domain's ([`translate`]).
//! It checks the boundary contracts as it goes (one answer per call and per
//! open, one terminal per operation, a kit closed once with every call
//! answered) and the universal invariants once it settles (no live entities,
//! nothing in flight, every session ended).

pub mod calls;
pub mod fixture;
pub mod memory;
mod names;
mod noisy;
pub mod translate;
mod world;

pub use names::kind;
pub use noisy::noisy_world;
pub use temper_world::Span;
pub use world::{Settings, Stats, Step, World, authority, repo};
