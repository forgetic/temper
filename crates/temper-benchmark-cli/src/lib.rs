// SPDX-License-Identifier: MPL-2.0

//! Offline contracts and trace normalization for `temper-benchmark`.
//!
//! Trace ingestion deliberately depends only on the shared activity protocol.
//! It accepts the durable journal representation and the public export
//! representation, then produces one validated in-memory stream. Later runner
//! and reporting layers can consume that stream without knowing where it came
//! from.

mod ingest;
mod summary;

pub use ingest::{NormalizedTrace, TraceIngestError, ingest_trace, write_canonical_export};
pub use summary::*;
