//! Deterministic, runtime-free unit tests for [`AgentMachine`](crate::machine::AgentMachine).
//!
//! Each test feeds a synthetic completion sequence and asserts on the emitted
//! requests — the call/tool/stop cycle the pi loop hides behind async/await is
//! here a plain, replayable function from `(state, completion)` to `[request]`.
//!
//! Split by domain responsibility, mirroring the module under test:
//! - [`loop_lifecycle`] — start/complete/error/budget/abort/steering control flow.
//! - [`batching`] — effect-compatible concurrency and tool-result ordering.

mod common;

mod batching;
mod decision_anchor;
mod invocation;
mod loop_lifecycle;
mod ordinary_failure;
