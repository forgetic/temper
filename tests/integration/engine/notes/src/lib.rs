//! A simulated world for the engine's notes sub-model (programming-style.md,
//! 4.5 and 11; engine-model.md, section 10): the notes, with the world as
//! their parent, driven by one loop, deterministically from a seed.
//!
//! The world owns the clock and the seeds, and stands in for everything
//! around the notes: the top level that routes to and from them, and the
//! forge sub-model and the protocol layers below it, as a wiki of pages in
//! each scope (the deployment's, two repositories', a goal in each) that
//! serves listings, reads and writes after a latency, some of them late and
//! some failing, and cuts a listing at the notes' limit; people who make,
//! edit and delete pages, some of whose edits are hinted by a webhook, late;
//! the parent's polling, which refreshes every scope; and the runs and
//! briefs that ask for indexes, searches, recalls and notes, some past the
//! limits, in more scopes than the notes keep at once, revising now and then
//! a page someone wrote over since they recalled it.
//!
//! It checks the contracts as it goes: every call is answered exactly once,
//! every wiki operation ended exactly once, and the parent keeps its own
//! (listings and pages cut to the notes' limits). Its referee ([`referee`])
//! holds the notes to what the scenarios expect, from what the wiki sees:
//! no line for a page deleted before its scope was last read, every line
//! saying what its page held, a recall answered with the pages as they were
//! meanwhile and finding nothing only for a page absent meanwhile, and every
//! call answered in time. And the invariants once it settles: nothing in
//! flight, no call or operation left in the notes, and the referee's verdict
//! passed. The world's own state is bounded: a run past its iterations, its
//! trace or the deliveries it keeps in flight fails with its seed.

pub mod referee;
mod world;

pub use temper_world::Span;
pub use world::{ENDINGS, LIMITS, RUNS, SCOPES, Settings, Stats, World};
