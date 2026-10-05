//! Compatibility with the worlds' established memory-test API
//! (testing-strategy.md, section 6; programming-model.md, section 6.3).
//! Counting and all measurements now come from skein's one allocator. This
//! adapter keeps the old public fields and by-value diagnostic argument;
//! it adds no allocator, heap state, clocks or memory accounting algorithm.

use std::fmt::Debug;

pub use skein_world::domain::heap::Counting;
use skein_world::domain::heap::{Measured as SharedMeasured, Meter as SharedMeter};

/// One shared-allocator step measurement, with legacy field access preserved.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Measured {
    /// Maximum live bytes above the meter base while the step ran.
    pub peak: u64,
    /// Live bytes above the base when the step ended, before outputs were dropped.
    pub held: u64,
    shared: SharedMeasured,
}

/// Delegates measurement to skein, retaining the existing world call signatures.
#[derive(Debug)]
pub struct Meter {
    shared: SharedMeter,
}

impl Meter {
    /// Records the allocator baseline; later allocations belong to this fixture.
    #[must_use]
    pub fn new() -> Meter {
        Meter { shared: SharedMeter::new() }
    }

    /// Bytes allocated since the baseline and still retained.
    #[must_use]
    pub fn held(&self) -> u64 {
        self.shared.held()
    }

    /// Starts one step's peak measurement, before the domain entry point runs.
    pub fn start(&self) {
        self.shared.start();
    }

    /// Ends the step, before its handed-out outputs are freed by the receiver.
    #[must_use]
    pub fn end(&self) -> Measured {
        let shared = self.shared.end();
        Measured { peak: shared.peak(), held: shared.held(), shared }
    }

    /// Checks retained ownership against `bound`, after dropping only handed-out
    /// outputs and before the next step starts. `what` labels any failure.
    pub fn check(&self, step: Measured, bound: u64, what: impl Debug) {
        self.shared.check(step.shared, bound, &what);
    }
}

impl Default for Meter {
    fn default() -> Meter {
        Meter::new()
    }
}
