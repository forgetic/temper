// SPDX-License-Identifier: MPL-2.0

//! The unified single-process mode (`temper run`): daemon + worker + agent as
//! concurrent tasks on one single-threaded skein event loop.
//!
//! This module is the only place all three planes' crates coexist, so the
//! in-process carriers that bridge them live here:
//! - [`InProcessTransport`] — worker→daemon over an in-memory call to a
//!   co-resident `Daemon`, instead of HTTP.
//!
//! Available only in builds with the `daemon` + `worker` features (the
//! `unified` profile).

mod transport;

pub use transport::InProcessTransport;

#[cfg(feature = "agent")]
mod agent_runner;

#[cfg(feature = "agent")]
pub use agent_runner::InProcessAgentRunner;
