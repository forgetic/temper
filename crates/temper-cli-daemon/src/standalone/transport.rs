// SPDX-License-Identifier: MPL-2.0

//! Backwards-compatible standalone transport module.
//!
//! The implementation lives in the reusable `temper-daemon-transport` crate so
//! non-CLI consumers (simulation and hermetic real-stack tests) can share the
//! same in-process worker→daemon carrier without depending on CLI internals.

pub use temper_daemon_transport::InProcessTransport;
