//! The functional-core contract.

/// Monotonic engine time: nanoseconds since the runtime's clock started.
///
/// The drive loop snapshots the runtime's monotonic clock exactly once per
/// delivery — at the moment the engine yields a completion to the machine —
/// and hands it to the transition, which records it as part of its state.
/// Machines never read a clock; time is plain data, so an `EngineTime` can be
/// constructed at any value in tests and serialized into recorded completion
/// logs for exact replay. Delivery-time stamping from a single clock also
/// guarantees the machine observes time monotonically, regardless of which
/// shell task produced each completion.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct EngineTime(u64);

impl EngineTime {
    pub const ZERO: Self = Self(0);

    pub const fn from_nanos(nanos: u64) -> Self {
        Self(nanos)
    }

    pub const fn as_nanos(self) -> u64 {
        self.0
    }

    /// Whole milliseconds since the clock started (saturating). Convenient for
    /// observability lines and coarse deadline math; machines keep the full
    /// nanosecond value as state.
    pub const fn as_millis(self) -> u64 {
        self.0 / 1_000_000
    }
}

impl std::ops::Add<std::time::Duration> for EngineTime {
    type Output = Self;

    fn add(self, delta: std::time::Duration) -> Self {
        Self(
            self.0
                .saturating_add(u64::try_from(delta.as_nanos()).unwrap_or(u64::MAX)),
        )
    }
}

impl From<asupersync::types::Time> for EngineTime {
    fn from(time: asupersync::types::Time) -> Self {
        Self(time.as_nanos())
    }
}

/// A deterministic state machine: the pure logic half of a Smith service.
///
/// `on_completion` is the single transition function
/// `(state, now, completion) -> (new state, [requests])`. Implementations
/// must be pure with respect to the outside world:
///
/// - no I/O (sockets, files, processes, channels),
/// - no clocks — `now` is the engine's once-per-delivery snapshot of the
///   runtime clock; machines keep it as a state field if they need it,
/// - no spawning or blocking.
///
/// Everything the machine learns arrives as a completion; everything it wants
/// done leaves as a request. This makes machines unit-testable by feeding a
/// completion sequence (with synthetic times) and asserting on the produced
/// requests — no runtime, no sleeps, no race conditions. The same property is
/// what lets the asupersync lab runtime replay a recorded schedule and explore
/// interleavings under chaos.
pub trait Machine {
    /// `<io-event-completion>`: one finished I/O event, delivered by the engine.
    type Completion;
    /// `<io-event-request>`: one I/O request for the engine to execute.
    type Request;

    /// Requests to submit before the first completion is delivered
    /// (initial timers, listeners that should start armed, ...).
    fn on_start(&mut self, _now: EngineTime) -> Vec<Self::Request> {
        Vec::new()
    }

    /// The pure transition function.
    fn on_completion(
        &mut self,
        now: EngineTime,
        completion: Self::Completion,
    ) -> Vec<Self::Request>;

    /// When this returns true after a transition, the engine loop exits and
    /// hands control back to the shell for drain/teardown. Services enter the
    /// stopped state by handling a shutdown completion.
    fn is_stopped(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn engine_time_adds_durations_saturating() {
        let t = EngineTime::from_nanos(1_000);
        assert_eq!((t + Duration::from_nanos(500)).as_nanos(), 1_500);
        // Saturates rather than overflowing.
        let max = EngineTime::from_nanos(u64::MAX);
        assert_eq!((max + Duration::from_secs(1)).as_nanos(), u64::MAX);
    }

    #[test]
    fn engine_time_millis_truncates() {
        assert_eq!(EngineTime::from_nanos(1_500_000).as_millis(), 1);
        assert_eq!(EngineTime::from_nanos(2_000_000).as_millis(), 2);
    }

    #[test]
    fn engine_time_orders_and_roundtrips_serde() {
        assert!(EngineTime::from_nanos(1) < EngineTime::from_nanos(2));
        let value = EngineTime::from_nanos(42);
        let json = serde_json::to_string(&value).expect("serialize");
        let back: EngineTime = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(value, back);
    }
}
