//! What the domain worlds share (testing-strategy.md, 2.2). Each world keeps
//! its own fakes, translations, settings, invariants and scenarios; this is the
//! machinery under them, the same in every world:
//!
//! - [`Schedule`]: deliveries in flight, by time and then by the order they
//!   were sent, withdrawn by their [`Key`]s, and the count that names them,
//!   which names the world's other things too;
//! - [`Span`]: latencies, drawn from the world's seed;
//! - [`Stage`]: a domain as the shell drives it, its events taken only while
//!   its queue has room for what one more step may emit (programming-model.md,
//!   section 2);
//! - [`Ledger`]: requests in flight, each ended once (one terminal per
//!   request, one reply per call);
//! - [`Trace`]: what crossed the world's boundaries, with times, and
//!   [`assert_replays`], that a seed replays to the same run;
//! - [`heap`]: a counting allocator that records the peak of live heap, for
//!   the memory tests' check against the worst case (programming-model.md,
//!   6.3);
//! - [`Referee`]: a scenario's [`Expectations`] as a step machine of the
//!   world's loop (testing.md, 5.2), safety checked on every observation and
//!   liveness armed as deadlines of its own, which injects what belongs to no
//!   fake and ends the test with a [`Verdict`].

pub mod heap;
// Legacy test import paths stay stable while smith uses the shared kit directly.
pub use skein_world::domain::{
    Expectations, Failure, Judge, Key, Ledger, Referee, Schedule, Span, Stage, Trace, Verdict, assert_replays,
};
