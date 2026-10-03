//! A domain world for the engine's work hub (programming-model.md, 4.5;
//! testing-strategy.md, 2.2; engine-domain.md, sections 4, 8 and 12): the
//! hub, with the world as its parent, driven by one loop, deterministically
//! from a seed.
//!
//! The world owns the clock and the seeds, and stands in for everything
//! around the hub: the top level that routes to and from it, and below it
//! the plan, which decides what is due (waits, engine actions, runs, holds,
//! done) and judges outcomes (applied, stale, invalid, waiting for a person's
//! acceptance); the forge, which keeps the items' records, posts outcomes and
//! makes keyed writes, after a latency, some late, some failing transiently
//! and served again, some refused for good; the fleet and its workers, which
//! run what starts (runs that end, park or fail by class), keep their
//! answers until told to forget them, drop their channels and come back with
//! their runs within the grace, or crash and are presumed lost; and people,
//! who hand items in, write on them, stop runs and release what is held,
//! accepting what waits for them. The engine restarts at moments the
//! scenario sets: what it asked of the forge lands then or never, its memory
//! is gone, and a new hub is rebuilt from the records the forge holds.
//!
//! It checks the contracts as it goes: every call is answered exactly once,
//! and an item has one request in flight at most, ended exactly once. Its
//! referee ([`referee`]) holds the hub to what the scenarios expect, from
//! what the fakes see: one run per item, claims written before runs start,
//! attempts that only grow across restarts, outcomes applied at most once,
//! held items never due until released, and every item ending, done or
//! held. And the invariants once it settles: nothing in flight, every item
//! taken in and ended, no worker running or keeping anything, no alarm left,
//! and the referee's verdict passed.

pub mod referee;
mod world;

pub use temper_world::Span;
pub use world::{ENDINGS, LIMITS, Settings, Stats, World};
