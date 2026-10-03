//! What the domain worlds share (testing-pyramid.md, 2.2). Each world keeps
//! its own fakes, translations, settings, invariants and scenarios; this is the
//! machinery under them, the same in every world:
//!
//! - [`Schedule`]: deliveries in flight, by time and then by the order they
//!   were sent, withdrawn by their [`Key`]s, and the count that names them,
//!   which names the world's other things too;
//! - [`Span`]: latencies, drawn from the world's seed;
//! - [`Stage`]: a domain as the shell drives it, its events taken only while
//!   its queue has room for what one more step may emit (7);
//! - [`Ledger`]: requests in flight, each ended once (one terminal per
//!   request, one reply per call);
//! - [`Trace`]: what crossed the world's boundaries, with times, and
//!   [`assert_replays`], that a seed replays to the same run;
//! - [`heap`]: a counting allocator that records the peak of live heap, for
//!   the memory tests' check against the worst case (6.3);
//! - [`Referee`]: a scenario's [`Expectations`] as a step machine of the
//!   world's loop (testing-pyramid.md, 5.2), safety checked on every
//!   observation and liveness armed as deadlines of its own, which injects
//!   what belongs to no fake and ends the test with a [`Verdict`].

pub mod heap;
mod ledger;
mod referee;
mod schedule;
mod stage;
mod trace;

pub use ledger::Ledger;
pub use referee::{Expectations, Failure, Judge, Referee, Verdict};
pub use schedule::{Key, Schedule};
pub use stage::Stage;
pub use trace::{Trace, assert_replays};

use temper_lib::{Duration, Rng};

/// Durations drawn uniformly from `min..=max`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Span {
    pub min: Duration,
    pub max: Duration,
}

impl Span {
    #[must_use]
    pub const fn millis(min: u64, max: u64) -> Span {
        Span { min: Duration::from_millis(min), max: Duration::from_millis(max) }
    }

    /// A duration drawn from the span.
    pub fn draw(self, rng: &mut Rng) -> Duration {
        Duration::from_nanos(rng.between(self.min.as_nanos(), self.max.as_nanos()))
    }
}
