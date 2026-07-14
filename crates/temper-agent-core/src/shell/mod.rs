//! The agent loop's imperative shell.
//!
//! [`AgentShell`] implements [`temper_agent_io::Executor`] for
//! [`AgentMachine`](crate::machine::AgentMachine): it performs the two I/O
//! seams the loop has — streaming a model response and executing a tool — by
//! reusing tongs `Provider`s and `Tool`s, and feeds every result back into the
//! completion queue. Observability events the machine emits as data are
//! forwarded to a sink; the terminal `Finished` request resolves the run's
//! outcome through a oneshot.
//!
//! The shell never calls into the machine; it only spawns I/O and enqueues
//! completions, keeping the loop's logic single-owner and deterministic.
//!
//! Split by domain responsibility:
//! - [`executor`] — the public shell types and the `Executor` dispatch.
//! - [`streaming`] — model streaming with liveness timeouts and retry.

mod executor;
mod streaming;

pub use executor::{
    AgentOutcome, AgentShell, EventClock, EventSink, ModelIdentity, NullEventSink,
    RunObservability, SystemEventClock, TurnHook,
};
#[cfg(feature = "test-support")]
pub use streaming::{
    StreamRetryConfig, StreamRetryConfigOverrideGuard, install_stream_retry_config_override,
};
