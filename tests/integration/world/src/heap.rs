//! A counting allocator for the memory tests (testing-pyramid.md, 6): it
//! records the live heap of each thread and its peak, so that a test can check
//! the most a model held at once in each step against its worst case
//! (programming-style.md, 6.4).
//! Each memory test binary declares it its global allocator:
//!
//! ```ignore
//! #[global_allocator]
//! static HEAP: temper_world::heap::Counting = temper_world::heap::Counting;
//! ```
//!
//! Counts are kept for each thread, so that tests running side by side do not
//! see each other's.
//!
//! What a step hands out in its requests is their receivers' to count, so a
//! step is checked at the most it held of its own: at each moment, what was
//! live less what it had handed out by then. Which blocks it handed out is
//! known only once the test drops its requests, after the step; so the meter
//! numbers every allocation, in a header before the block, and keeps the
//! moments the step's heap reached a new high, by the allocation that reached
//! it (no other moment can hold more of the step's own, as what it hands out
//! only grows). A block freed after the step was handed out by each of those
//! moments that its number is no later than.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::{Cell, RefCell};
use std::fmt::Debug;
use std::mem::size_of;
use std::ptr;

/// The allocator: System's, counted.
#[derive(Debug)]
pub struct Counting;

/// A moment a step's heap reached a new high: the last allocation made by
/// then, by number, what was live, and of what the step handed out, how much
/// was allocated after the high before it and by this one (so that what it
/// handed out by a high is the sum of these up to it). A high that stands
/// for several moments (see [`Highs`]) is the first's, with the heap of the
/// last, and `low` the first's.
#[derive(Clone, Copy)]
struct High {
    made: u64,
    low: i64,
    live: i64,
    handed: u64,
}

/// The highs of the step measured last. Beyond its room, two adjacent highs
/// become one that counts the later's heap at the earlier's moment, the two
/// whose moments it then spans the least: it holds no less of the step's own
/// than any moment it stands for, so a check stays sound, and errs by no
/// more than the heap its moments span.
struct Highs {
    len: usize,
    at: [High; HIGHS],
}

const HIGHS: usize = 256;

const NONE: High = High { made: 0, low: 0, live: 0, handed: 0 };

/// Whether a step is being measured, or its handed out blocks counted.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Idle,
    /// A step runs: its highs are kept.
    Stepping,
    /// A step has ended: what is freed from now on that was allocated since
    /// the meter's base, numbered after `before`, was handed out.
    Handing {
        before: u64,
    },
}

thread_local! {
    // Const-initialised and without a destructor: reading them never
    // allocates, so the allocator can use them.
    static LIVE: Cell<i64> = const { Cell::new(0) };
    static PEAK: Cell<i64> = const { Cell::new(0) };
    // Allocations made so far, which numbers them from one.
    static MADE: Cell<u64> = const { Cell::new(0) };
    static PHASE: Cell<Phase> = const { Cell::new(Phase::Idle) };
    static HIGH_POINTS: RefCell<Highs> = const { RefCell::new(Highs { len: 0, at: [NONE; HIGHS] }) };
}

/// Counts a block of `layout` allocated, and numbers it.
fn allocated(layout: Layout) -> u64 {
    let made = MADE.with(|made| {
        made.set(made.get().wrapping_add(1));
        made.get()
    });
    let live = LIVE.with(|live| {
        live.set(live.get().wrapping_add(size(layout)));
        live.get()
    });
    let high = PEAK.with(|peak| {
        let high = live > peak.get();
        if high {
            peak.set(live);
        }
        high
    });
    if high && PHASE.with(Cell::get) == Phase::Stepping {
        HIGH_POINTS.with_borrow_mut(|highs| highs.push(High { made, low: live, live, handed: 0 }));
    }
    made
}

/// Counts the block of `layout` numbered `number` freed.
fn freed(layout: Layout, number: u64) {
    LIVE.with(|live| live.set(live.get().wrapping_sub(size(layout))));
    let Phase::Handing { before } = PHASE.with(Cell::get) else {
        return;
    };
    if number > before {
        let bytes = u64::try_from(layout.size()).unwrap_or(u64::MAX);
        HIGH_POINTS.with_borrow_mut(|highs| highs.hand(number, bytes));
    }
}

fn size(layout: Layout) -> i64 {
    i64::try_from(layout.size()).unwrap_or(i64::MAX)
}

impl Highs {
    fn push(&mut self, high: High) {
        if self.len == HIGHS {
            self.merge();
        }
        if let Some(slot) = self.at.get_mut(self.len) {
            *slot = high;
            self.len += 1;
        }
    }

    /// Makes room: two adjacent highs become one.
    fn merge(&mut self) {
        let mut first = 0;
        let mut least = i64::MAX;
        for at in 0..self.len - 1 {
            let span = self.at[at + 1].live - self.at[at].low;
            if span < least {
                (first, least) = (at, span);
            }
        }
        self.at[first].live = self.at[first + 1].live;
        self.at.copy_within(first + 2..self.len, first + 1);
        self.len -= 1;
    }

    /// The block numbered `number`, of `bytes`, was handed out: by each high
    /// it was allocated by, from the first on, as the highs are in the order
    /// of their numbers.
    fn hand(&mut self, number: u64, bytes: u64) {
        let highs = &mut self.at[..self.len];
        let first = highs.partition_point(|high| high.made < number);
        if let Some(high) = highs.get_mut(first) {
            high.handed = high.handed.saturating_add(bytes);
        }
    }
}

/// The block System allocates for one of `layout`: room before it for its
/// number, as far as its alignment puts it and no less than a number takes,
/// and that far.
fn padded(layout: Layout) -> Option<(Layout, usize)> {
    let offset = layout.align().max(size_of::<u64>());
    let padded = Layout::from_size_align(layout.size().checked_add(offset)?, offset).ok()?;
    Some((padded, offset))
}

// SAFETY: each block is `offset` bytes into one of System's, aligned to
// `offset`, a multiple of the block's alignment; its number is in the bytes
// just before it. Counting touches no memory the caller sees. realloc and
// alloc_zeroed keep their default bodies, which call these two: a realloc
// counts the old block and the new at once, as they are both live while it
// copies.
#[expect(unsafe_code, reason = "a global allocator is an unsafe impl; it only counts, and System allocates")]
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let Some((padded, offset)) = padded(layout) else {
            return ptr::null_mut();
        };
        // SAFETY: `padded` is larger than `layout`, which is not empty.
        let base = unsafe { System.alloc(padded) };
        if base.is_null() {
            return base;
        }
        let number = allocated(layout);
        let number = number.to_ne_bytes();
        // SAFETY: `offset` is within System's block and no less than a number
        // takes, so the number's bytes are the block's.
        unsafe {
            let block = base.add(offset);
            ptr::copy_nonoverlapping(number.as_ptr(), block.sub(number.len()), number.len());
            block
        }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // It was allocated with `layout`, which padded then.
        let Some((padded, offset)) = padded(layout) else {
            return;
        };
        let mut number = [0; size_of::<u64>()];
        // SAFETY: `ptr` came from alloc with `layout`, above: its number is
        // just before it, and System's block `offset` bytes before it.
        unsafe {
            ptr::copy_nonoverlapping(ptr.sub(number.len()), number.as_mut_ptr(), number.len());
            System.dealloc(ptr.sub(offset), padded);
        }
        freed(layout, u64::from_ne_bytes(number));
    }
}

fn live() -> i64 {
    LIVE.with(Cell::get)
}

fn made() -> u64 {
    MADE.with(Cell::get)
}

/// What one step held, measured by a [`Meter`]: the most at once while it
/// ran, and what it held when it ended.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Measured {
    pub peak: u64,
    pub held: u64,
    /// The allocations made before it started, which names it.
    started: u64,
}

/// Measures the heap of what a test builds after it: a model, a container.
#[derive(Debug)]
pub struct Meter {
    base: i64,
    /// The allocations made before it.
    before: u64,
}

impl Meter {
    /// Measures from now: what is allocated after this, and not freed, is
    /// the measured's.
    #[must_use]
    pub fn new() -> Meter {
        Meter { base: live(), before: made() }
    }

    /// Bytes allocated since the base and not freed.
    #[must_use]
    pub fn held(&self) -> u64 {
        self.since(live())
    }

    /// Starts a step: the highs from now on are the step's, the first being
    /// what is live now.
    pub fn start(&self) {
        let (made, live) = (made(), live());
        PEAK.with(|peak| peak.set(live));
        HIGH_POINTS.with_borrow_mut(|highs| {
            highs.len = 0;
            highs.push(High { made, low: live, live, handed: 0 });
        });
        PHASE.with(|phase| phase.set(Phase::Stepping));
    }

    /// Ends a step: the most held at once since it started, and what is held
    /// now. What is freed from now on was handed out by it, until the next
    /// starts.
    #[must_use]
    pub fn end(&self) -> Measured {
        assert!(PHASE.with(Cell::get) == Phase::Stepping, "a step ends after it starts");
        PHASE.with(|phase| phase.set(Phase::Handing { before: self.before }));
        let started = HIGH_POINTS.with_borrow(|highs| highs.at[0].made);
        Measured { peak: self.since(PEAK.with(Cell::get)), held: self.held(), started }
    }

    /// Checks `step` against `bound`, a worst case: the most it held of its
    /// own at once, which is what was live less what it had handed out by
    /// then and its receivers count (6.4: what travels in requests is the
    /// layer's that holds it). What it handed out is what has been freed
    /// since it ended, as the test took its requests and dropped them: the
    /// test frees nothing else in between.
    pub fn check(&self, step: Measured, bound: u64, what: impl Debug) {
        let own = HIGH_POINTS.with_borrow(|highs| {
            assert!(
                highs.len > 0 && highs.at[0].made == step.started && PHASE.with(Cell::get) != Phase::Stepping,
                "a step is checked once it has ended, before the next starts"
            );
            let mut own = 0;
            let mut handed: u64 = 0;
            for high in &highs.at[..highs.len] {
                handed = handed.saturating_add(high.handed);
                let held = self.since(high.live).checked_sub(handed);
                own = own.max(held.expect("what a step handed out by a high was live at it"));
            }
            own
        });
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
