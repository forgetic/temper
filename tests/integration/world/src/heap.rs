//! A counting allocator for the memory tests (programming-model.md, 11): it
//! records the live heap of each thread and its peak, so that a test can check
//! the most a model held at once in each step against its worst case (6.4).
//! Each memory test binary declares it its global allocator:
//!
//! ```ignore
//! #[global_allocator]
//! static HEAP: temper_world::heap::Counting = temper_world::heap::Counting;
//! ```
//!
//! Counts are kept for each thread, so that tests running side by side do not
//! see each other's.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::fmt::Debug;

/// The allocator: System's, counted.
#[derive(Debug)]
pub struct Counting;

thread_local! {
    // Const-initialised and without a destructor: reading them never
    // allocates, so the allocator can use them.
    static LIVE: Cell<i64> = const { Cell::new(0) };
    static PEAK: Cell<i64> = const { Cell::new(0) };
}

fn count(layout: Layout, sign: i64) {
    let size = i64::try_from(layout.size()).unwrap_or(i64::MAX);
    let live = LIVE.with(|live| {
        live.set(live.get().wrapping_add(size.wrapping_mul(sign)));
        live.get()
    });
    PEAK.with(|peak| peak.set(peak.get().max(live)));
}

// SAFETY: every call is passed to System unchanged; counting touches no
// memory the caller sees. realloc and alloc_zeroed keep their default bodies,
// which call these two: a realloc counts the old block and the new at once, as
// they are both live while it copies.
#[expect(unsafe_code, reason = "a global allocator is an unsafe impl; it only counts, and System allocates")]
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count(layout, 1);
        // SAFETY: the caller upholds alloc's contract, which is System's.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        count(layout, -1);
        // SAFETY: `ptr` came from System.alloc with `layout`, above.
        unsafe { System.dealloc(ptr, layout) }
    }
}

fn live() -> i64 {
    LIVE.with(Cell::get)
}

/// What one step held, measured by a [`Meter`]: the most at once while it
/// ran, and what it held when it ended.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Measured {
    pub peak: u64,
    pub held: u64,
}

/// Measures the heap of what a test builds after it: a model, a container.
#[derive(Debug)]
pub struct Meter {
    base: i64,
}

impl Meter {
    /// Measures from now: what is allocated after this, and not freed, is
    /// the measured's.
    #[must_use]
    pub fn new() -> Meter {
        Meter { base: live() }
    }

    /// Bytes allocated since the base and not freed.
    #[must_use]
    pub fn held(&self) -> u64 {
        self.since(live())
    }

    /// Starts a step: the peak from now on is the step's.
    pub fn start(&self) {
        PEAK.with(|peak| peak.set(live()));
    }

    /// Ends a step: the most held at once since it started, and what is held
    /// now.
    #[must_use]
    pub fn end(&self) -> Measured {
        Measured { peak: self.since(PEAK.with(Cell::get)), held: self.held() }
    }

    /// Checks the peak of `step` against `bound`, a worst case, less what
    /// the step handed out and its receivers count (6.4: what travels in
    /// requests is the layer's that holds it): what has been freed since the
    /// step ended, as the test took its requests and dropped them.
    pub fn check(&self, step: Measured, bound: u64, what: impl Debug) {
        let handed = step.held.saturating_sub(self.held());
        let own = step.peak - handed;
        assert!(own <= bound, "{what:?}: {own} bytes held at the peak of a step, more than the worst case of {bound}");
    }

    fn since(&self, count: i64) -> u64 {
        u64::try_from(count - self.base).expect("nothing freed that was not allocated since the base")
    }
}

impl Default for Meter {
    fn default() -> Meter {
        Meter::new()
    }
}
