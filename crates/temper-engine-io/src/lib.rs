// SPDX-License-Identifier: MPL-2.0

//! io_uring-style completion engine: the imperative shell that drives temper's
//! pure logic layers.
//!
//! # Architecture
//!
//! Every temper service is split into two halves:
//!
//! - a **functional core** — a deterministic state machine implementing
//!   [`Machine`]: `(state, completion) -> (new state, [requests])`. No sockets,
//!   no clocks, no spawning, no side effects. Time only enters as data carried
//!   by completions.
//! - an **imperative shell** — an [`Executor`] that performs the actual I/O
//!   for each request on the skein runtime and eventually feeds an
//!   `<io-event-completion>` back into the engine's completion queue.
//!
//! The arrow loops:
//!
//! ```text
//!   <io-event-completion> ──▶ Machine::on_completion (pure)
//!            ▲                          │
//!            │                          ▼
//!     Executor (skein I/O) ◀── <io-event-request>
//! ```
//!
//! [`drive`] is the only loop: it receives one completion at a time, runs the
//! pure transition, and submits each produced request to the executor. The
//! executor never calls back into the machine; it only enqueues completions,
//! which keeps the core single-owner and deterministic — feeding a recorded
//! completion sequence into a fresh machine replays the exact same behavior,
//! with no runtime involved.

pub mod cadence;
pub mod engine;
pub mod http;
pub mod machine;
pub mod process;
pub mod queue;
pub mod runtime;
pub mod spawn;
pub mod timer;

/// The capability context type engine code passes around explicitly
/// (re-exported so dependents need no direct skein dependency).
pub use skein::cx::Cx;

pub use cadence::spawn_cadence_loop;
pub use engine::{Executor, drive};
pub use machine::{EngineTime, Machine};
pub use queue::{CqReceiver, CqSender, OneshotReceiver, OneshotSender, channel, oneshot};
pub use runtime::{EngineRuntime, block_on, block_on_with, build_runtime};
pub use spawn::{SpawnFactory, Spawner};
pub use timer::arm_timer;
