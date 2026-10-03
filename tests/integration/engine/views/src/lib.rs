//! A simulated world for the engine's views child domain (programming-model.md,
//! 4.5; engine-domain.md, section 11): the views, with the world as
//! their parent, driven by one loop, deterministically from a seed.
//!
//! The world owns the clock and the seeds, and stands in for everything
//! around the views: the top level, which starts runs for items (a run named
//! by its item, as in the engine), each attempt under a capture policy drawn,
//! starts a live run again for a new attempt, passes on what runs report at
//! drawn rates (now and then past the limits, or after a run finished, as a
//! late fact would be), tells items' phase changes, starts more runs than the
//! views may follow, and routes what the views ask for as each step emits
//! it; the people, who watch runs, items and boards from a snapshot, some of
//! runs that are not followed, more than the views may hold, several at once,
//! under tokens they reuse, and stop after a while, their streams taking each
//! delivery after a latency, some slowly, some not at all, some hanging until
//! the parent's bound; and the engine's store, which does one operation at a
//! time, in the order asked, slowly, failing, or at once within the
//! iteration, a failed append keeping its first records. Its referee
//! restarts the engine at moments drawn: new views, every watch gone with its
//! stream, and the store keeping what it holds.
//!
//! It checks the contracts as it goes: every watch is answered once and
//! ended once, only once its delivery has; a watcher has one delivery in
//! flight; and every store operation ends once. Its referee ([`referee`])
//! holds the views to what the scenarios expect, from what the parent tells
//! them, what they emit and what the store keeps and forgets: refusals only
//! when they are due; a watch begins with its snapshot; a watcher never sees
//! chunks out of order or twice, and is told exactly what it missed; the
//! store keeps only what the policies keep, in order; nothing is forgotten
//! within the retention, nor kept past it beyond a sweep, across restarts
//! too; and every chunk that goes out to a watcher reaches it in time, unless
//! its stream does not take it. And the invariants once it settles: nothing
//! in flight, no run, watcher, record, operation or deadline left in the
//! views, nothing left in the store, the referee's verdict passed, and the
//! views' counts of what they lost what the referee saw lost. The world
//! bounds its own state: deliveries in flight, trace lines and iterations,
//! past which it fails with the seed.

pub mod referee;
mod world;

pub use temper_world::Span;
pub use world::{ENDINGS, KINDS, LIMITS, Settings, Stats, World};
