//! A simulated world for the worker's checkout sub-model
//! (programming-style.md, 4.5; worker-model.md, 9): the checkout, with the
//! world as its parent, against a fake forge's git and a fake disk, driven by
//! one loop, deterministically from a seed.
//!
//! The world owns the clock and the seed, and stands in for everything around
//! the model: its clients, scripted ([`client`]), which prepare workspaces for
//! several workstreams (so the cache fills, evicts and reuses), edit the
//! working trees between their pushes as a run's tools would, push, save,
//! release, and abort or release at moments of their own; and the protocol
//! layer and io, which run each operation on the fakes
//! ([`temper_checkout_fake`]) after a latency, racing its deadline and any
//! cancel, with the faults the world scripts: a repository that cannot be
//! reached or refuses a push, io failing, another party advancing a branch,
//! and specs that name what the forge does not have ([`translate`]).
//!
//! It checks the boundary's contracts as it goes: one terminal event per
//! operation, a hold before its prepare's end, one end per prepare, push,
//! save and release; the cache within its bound, and a workspace held by one
//! hold at a time, so evicted only when idle; no operation once its client
//! has released, or outside an operation its client waits for, so none
//! after an abort has ended one; each operation's deadline and identity; a
//! branch moved only by a fast-forward, and a base branch only created; a
//! workspace prepared at its starting points; what landed exactly the tree
//! the client left, and nothing to push only when nothing changed. And the
//! invariants once it settles: no hold left, every workspace idle, nothing
//! in flight, every client heard every end, and facts that add up to what
//! crossed the boundary unless some were dropped.

pub mod client;
pub mod translate;
mod world;

pub use temper_world::Span;
pub use world::{IDENTITY, LIMITS, Settings, Stats, Told, World};
