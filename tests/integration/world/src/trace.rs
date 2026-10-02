use std::fmt::{Debug, Display};

use temper_lib::Time;

/// What crossed a world's boundaries, in order, each line with the time it
/// crossed at: a seed replays to the same trace (programming-model.md, 11).
#[derive(Default, Debug)]
pub struct Trace {
    lines: Vec<String>,
}

impl Trace {
    pub fn log(&mut self, now: Time, line: impl Display) {
        self.lines.push(format!("{:>16} {line}", now.as_nanos()));
    }

    #[must_use]
    pub fn lines(&self) -> &[String] {
        &self.lines
    }
}

/// Checks that a world replays from its seed: `run` runs a world of a seed to
/// its end, and returns its trace and whatever else must come out the same
/// (its stats, the time it settled at). `seed` run twice comes out the same,
/// and `other` takes a course of its own. Returns the trace of `seed`, for the
/// caller to check that the world did something.
pub fn assert_replays<T: PartialEq + Debug>(
    seed: u64,
    other: u64,
    run: impl Fn(u64) -> (Vec<String>, T),
) -> Vec<String> {
    let (trace, end) = run(seed);
    let again = run(seed);
    assert!(again.0 == trace, "seed {seed} replays to the same trace");
    assert_eq!(again.1, end, "seed {seed} replays to the same end");
    assert!(run(other).0 != trace, "seeds {seed} and {other} take courses of their own");
    trace
}
