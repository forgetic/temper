//! A simulated world for the engine's views sub-model (programming-style.md,
//! 4.5 and 11; engine-model.md, section 11): the views, with the world as
//! their parent, driven by one loop, deterministically from a seed.
//!
//! The world owns the clock and the seeds, and stands in for everything
//! around the views: the top level, which starts runs for items under
//! capture policies drawn, passes on what they report at drawn rates (now
//! and then past the limits, or after a run finished, as a late fact would
//! be), tells items' phase changes, and starts more runs than the views may
//! follow; the people, who watch runs, items and boards, some of runs that
//! are not followed, more than the views may hold, and stop after a while,
//! their streams taking each delivery after a latency, some slowly; and the
//! engine's store, which does one operation at a time, in the order asked,
//! slowly or failing at drawn chances, a failed append keeping its first
//! records.
//!
//! It checks the contracts as it goes: every watch is answered once and
//! ended once, only once its delivery has; a watcher has one delivery in
//! flight; and every store operation ends once. Its referee ([`referee`])
//! holds the views to what the scenarios expect, from what the parent tells
//! them, what they emit and what the store keeps and forgets: refusals only
//! when they are due; a watcher never sees chunks out of order or twice, and
//! is told exactly what it missed; the store keeps only what the policies
//! keep, in order; nothing is forgotten within the retention, nor kept past
//! it beyond a sweep; and every chunk taken while its watcher was caught up
//! reaches it in time. And the invariants once it settles: nothing in flight,
//! no run, watcher, record, operation or deadline left in the views, nothing
//! left in the store, and the referee's verdict passed. The world bounds its
//! own state: deliveries in flight, trace lines and iterations, past which
//! it fails with the seed.

pub mod referee;
mod world;

pub use temper_world::Span;
pub use world::{ENDINGS, KINDS, LIMITS, PHASES, Settings, Stats, World};
