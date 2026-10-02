//! The meter checks a step at its peak: what it allocated and freed within
//! the step counts, and what it handed out and the test dropped does not.

use std::hint::black_box;

use temper_world::heap::{self, Measured, Meter};

#[global_allocator]
static HEAP: heap::Counting = heap::Counting;

/// A step that keeps 100 bytes, uses 1000 for a while, and hands out 50.
fn step(meter: &Meter) -> (Measured, Vec<u8>, Vec<u8>) {
    meter.start();
    let kept = black_box(vec![0_u8; 100]);
    drop(black_box(vec![0_u8; 1000]));
    let handed = black_box(vec![0_u8; 50]);
    (meter.end(), kept, handed)
}

#[test]
fn a_step_is_measured_at_its_peak_less_what_it_handed_out() {
    let meter = Meter::new();
    let (measured, kept, handed) = step(&meter);
    assert_eq!(measured, Measured { peak: 1100, held: 150 });
    drop(handed);
    meter.check(measured, 1050, "a step");
    assert_eq!(meter.held(), 100);
    drop(kept);
}

#[test]
#[should_panic(expected = "1050 bytes held at the peak of a step, more than the worst case of 1049")]
fn a_step_whose_peak_passes_the_worst_case_fails_its_check() {
    let meter = Meter::new();
    let (measured, _kept, handed) = step(&meter);
    drop(handed);
    meter.check(measured, 1049, "a step");
}
