// SPDX-License-Identifier: MPL-2.0

//! Feed adapters into the read-model.
//!
//! - [`logtail`] — feed 1b: parse `temper-log` JSON lines into board deltas.
//! - [`snapshot_source`] — feed 1a: the cold-start snapshot source seam (the
//!   live daemon `GET /v1/state`, a fixture, or empty), kept injectable so the
//!   server runs and tests pass without a live daemon.
//!
//! Out of scope for PR D: feed 2 (conversation SSE proxy, PR E) and feed 3 (live
//! agent token streaming, a later follow-up).

pub mod logtail;
pub mod snapshot_source;

pub use logtail::{LogLineSource, LogTailAdapter};
pub use snapshot_source::{EmptySnapshotSource, FixtureSnapshotSource, SnapshotSource};
