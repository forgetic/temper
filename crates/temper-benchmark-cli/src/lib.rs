// SPDX-License-Identifier: MPL-2.0

//! Offline contracts and trace normalization for `temper-benchmark`.
//!
//! Trace ingestion deliberately depends only on the shared activity protocol.
//! It accepts the durable journal representation and the public export
//! representation, then produces one validated in-memory stream. Later runner
//! and reporting layers can consume that stream without knowing where it came
//! from.

mod artifacts;
mod ingest;
mod manifest;
mod summary;
mod workspace;

pub use artifacts::{
    ArtifactLayoutError, BASELINE_SNAPSHOT_VERSION, BenchmarkArtifactLayout,
    RepetitionArtifactPaths,
};
pub use ingest::{NormalizedTrace, TraceIngestError, ingest_trace, write_canonical_export};
pub use manifest::{
    BENCHMARK_MANIFEST_SCHEMA, BenchmarkAnnotationsV1, BenchmarkManifestError, BenchmarkManifestV1,
    ResolvedBenchmarkManifest, load_benchmark_manifest,
};
pub use summary::*;
pub use workspace::{
    PreparedBenchmarkWorkspace, RepositoryBaselineV1, WorkspacePreparationError,
    prepare_benchmark_workspace,
};
