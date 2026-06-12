//! io_uring-style completion engine: the imperative shell that drives Smith's
//! pure logic layers.
//!
//! # Architecture
//!
//! Every Smith engine service is split into two halves:
//!
//! - a **functional core** — a deterministic state machine implementing
//!   [`Machine`]: `(state, completion) -> (new state, [requests])`. No sockets,
//!   no clocks, no spawning, no side effects. Time only enters as data carried
//!   by completions.
//! - an **imperative shell** — an [`Executor`] that performs the actual I/O
//!   for each request on the asupersync runtime and eventually feeds an
//!   `<io-event-completion>` back into the engine's completion queue.
//!
//! The arrow loops:
//!
//! ```text
//!   <io-event-completion> ──▶ Machine::on_completion (pure)
//!            ▲                          │
//!            │                          ▼
//!     Executor (asupersync I/O) ◀── <io-event-request>
//! ```
//!
//! [`drive`] is the only loop: it receives one completion at a time, runs the
//! pure transition, and submits each produced request to the executor. The
//! executor never calls back into the machine; it only enqueues completions,
//! which keeps the core single-owner and deterministic — feeding a recorded
//! completion sequence into a fresh machine replays the exact same behavior,
//! with no runtime involved.
//!
//! # Testing
//!
//! Because the core is pure, machines are unit-testable with no runtime:
//! [`drive_sync`] feeds a synthetic completion sequence with synthetic
//! [`EngineTime`] stamps and lets a test executor record the emitted requests.
//! The same property is what lets the asupersync **lab** runtime replay a
//! recorded schedule under a virtual clock and explore interleavings with
//! chaos injection — the long-term simulation/fuzz-testing goal.
//!
//! This crate deliberately depends on nothing from temper: it is Smith's own
//! copy of the sans-IO driver pattern, kept minimal (no HTTP-server / process /
//! cadence modules — Smith's worker is an HTTP *client* that spawns in-process
//! agent jobs). Some duplication with `temper-io-engine` is accepted in
//! exchange for an independent dependency surface.

pub mod engine;
pub mod http_client;
pub mod machine;
pub mod queue;
pub mod runtime;
pub mod timer;

pub use engine::{drive, drive_sync, Executor};
pub use http_client::{build_http_client, http_call, HttpCall, HttpCallResult, HttpResponseData};
pub use machine::{EngineTime, Machine};
pub use queue::{channel, oneshot, CqReceiver, CqSender, OneshotReceiver, OneshotSender};
pub use runtime::{
    block_on, block_on_runtime, build_runtime, current_cx, engine_now, sleep_for, timer_now,
    EngineRuntime,
};
pub use timer::arm_timer;
