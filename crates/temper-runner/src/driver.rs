//! Worker drivers.
//!
//! [`FixpointDriver`] is the deterministic scheduler used by in-process stages
//! and tests. It ticks every worker round-robin until one full pass makes no
//! progress. [`PollLoop`] is the per-process cadence driver: it ticks one
//! worker, waits one poll interval, and repeats until a caller-supplied stop
//! signal fires. [`WakeablePollLoop`] keeps the same polling backstop but lets
//! companion hint sources shorten the wait. All drivers use the same
//! [`Worker`](crate::Worker) trait.

mod clock;
mod fixpoint;
mod poll;
mod report;

pub use clock::ManualClock;
pub use fixpoint::FixpointDriver;
pub use poll::{PollLoop, WakeablePollLoop};
pub use report::{DriveError, RunReport, WorkerRunReport};
