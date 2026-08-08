// SPDX-License-Identifier: MPL-2.0

//! Offline contracts and trace normalization for `temper-benchmark`.
//!
//! Trace ingestion deliberately depends only on the shared activity protocol.
//! It accepts the durable journal representation and the public export
//! representation, then produces one validated in-memory stream. Later runner
//! and reporting layers can consume that stream without knowing where it came
//! from.

mod aggregate;
mod analyze;
mod artifacts;
mod comparison;
mod ingest;
mod manifest;
mod metadata;
mod report;
mod runner;
mod summary;
mod workspace;

pub use aggregate::{
    ADVISORY_METRICS, AggregateError, AggregateRunV1, BENCHMARK_AGGREGATE_VERSION,
    BenchmarkAggregateV1, DistributionV1, PRIMARY_METRICS, RunOutcomeCountsV1,
    aggregate_run_summaries, render_aggregate_markdown,
};
pub use analyze::{AnalyzeOptions, analyze_trace};
pub use artifacts::{
    ArtifactLayoutError, BASELINE_SNAPSHOT_VERSION, BenchmarkArtifactLayout,
    RepetitionArtifactPaths,
};
pub use comparison::{
    BENCHMARK_COMPARISON_VERSION, BenchmarkComparisonV1, ComparisonError, ComparisonInput,
    ComparisonInputKindV1, ComparisonSubjectV1, MetricComparisonV1, compare_benchmarks,
    load_comparison_input, render_comparison_markdown, write_comparison_artifacts,
};
pub use ingest::{NormalizedTrace, TraceIngestError, ingest_trace, write_canonical_export};
pub use manifest::{
    BENCHMARK_MANIFEST_SCHEMA, BenchmarkAnnotationsV1, BenchmarkConditionProfileKindV1,
    BenchmarkConditionProfileV1, BenchmarkManifestError, BenchmarkManifestV1,
    GraphDecisionCorrelationV1, GraphDecisionTargetV1, ResolvedBenchmarkManifest,
    load_benchmark_manifest,
};
pub use metadata::{
    best_effort_temper_commit, collect_environment_metadata, observed_model_identities,
    temper_build_metadata,
};
pub use report::{
    RUN_SUMMARY_JSON_FILE, RUN_SUMMARY_MARKDOWN_FILE, ReportWriteError, render_run_summary_json,
    render_run_summary_markdown, write_run_summary,
};
pub use runner::{
    AcceptedSubmitEvidenceV1, BenchmarkRunError, DIFF_ARTIFACT_VERSION, DiffArtifactV1,
    DiffFileEvidenceV1, ExactPatchEvidenceV1, HarnessRunOptions, LIVE_OPT_IN_ENV, LiveRunOptions,
    RepositoryDiffEvidenceV1, VALIDATION_ARTIFACT_VERSION, ValidationArtifactV1,
    ValidationCommandEvidenceV1, run_harness, run_live,
};
pub use summary::*;
pub use workspace::{
    PreparedBenchmarkWorkspace, RepositoryBaselineV1, WorkspacePreparationError,
    prepare_benchmark_workspace,
};
