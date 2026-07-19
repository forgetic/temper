// SPDX-License-Identifier: MPL-2.0

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use temper_benchmark_cli::{
    BenchmarkAnnotationsV1, BenchmarkModeV1, BenchmarkRunV1, ComparisonInput, DiffStatisticsV1,
    DistributionV1, RunSummaryV1, aggregate_run_summaries, collect_environment_metadata,
    compare_benchmarks, ingest_trace, load_comparison_input, render_aggregate_markdown,
    render_comparison_markdown,
};
use temper_protocol_activity::{AgentActivityEventV1, ModelCallStartedV1};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(name)
}

fn summary(run_id: &str, turns: u64, wall_time_ms: u64) -> RunSummaryV1 {
    let mut summary = ingest_trace(fixture("journal-complete"))
        .unwrap()
        .run_summary();
    summary.identity.run_id = run_id.to_string();
    summary.metrics.turns = Some(turns);
    summary.wall_time_ms = Some(wall_time_ms);
    summary.diff = Some(DiffStatisticsV1 {
        files_changed: turns,
        insertions: turns * 2,
        deletions: turns / 2,
        tracked_files: turns,
        untracked_files: 0,
    });
    summary
}

#[test]
fn aggregation_uses_deterministic_nearest_rank_quartiles_and_keeps_every_run() {
    let runs = [40, 10, 30, 20]
        .into_iter()
        .enumerate()
        .map(|(index, value)| summary(&format!("run-{index}"), value, value * 10))
        .collect::<Vec<_>>();
    let aggregate = aggregate_run_summaries(runs).unwrap();

    assert_eq!(aggregate.outcomes.total, 4);
    assert_eq!(aggregate.outcomes.succeeded, 4);
    assert_eq!(aggregate.runs.len(), 4);
    assert_eq!(aggregate.runs[0].summary.identity.run_id, "run-0");
    assert_eq!(
        aggregate.metrics["turns"],
        DistributionV1 {
            count: 4,
            min: 10,
            p25: 10,
            median: 20,
            p75: 30,
            max: 40,
        }
    );

    let markdown = render_aggregate_markdown(&aggregate);
    assert!(markdown.contains("| turns | 4 | 10 | 10 | 20 | 30 | 40 |"));
    assert!(markdown.contains("## Advisory timings"));
}

#[test]
fn quartiles_are_stable_for_single_and_odd_sample_sets() {
    assert_eq!(
        DistributionV1::from_values(vec![99]).unwrap(),
        DistributionV1 {
            count: 1,
            min: 99,
            p25: 99,
            median: 99,
            p75: 99,
            max: 99,
        }
    );
    assert_eq!(
        DistributionV1::from_values(vec![50, 10, 40, 20, 30]).unwrap(),
        DistributionV1 {
            count: 5,
            min: 10,
            p25: 20,
            median: 30,
            p75: 40,
            max: 50,
        }
    );
    assert_eq!(DistributionV1::from_values(Vec::new()), None);
}

#[test]
fn aggregate_and_comparison_contracts_reject_unknown_outer_fields() {
    let aggregate = aggregate_run_summaries([summary("run-a", 1, 10)]).unwrap();
    let mut aggregate_json = serde_json::to_value(&aggregate).unwrap();
    aggregate_json["surprise"] = serde_json::json!(true);
    let error =
        serde_json::from_value::<temper_benchmark_cli::BenchmarkAggregateV1>(aggregate_json)
            .unwrap_err();
    assert!(error.to_string().contains("unknown field `surprise`"));

    let input = ComparisonInput::Aggregate(aggregate.clone());
    let comparison = compare_benchmarks(&input, &input).unwrap();
    let mut comparison_json = serde_json::to_value(comparison).unwrap();
    comparison_json["surprise"] = serde_json::json!(true);
    let error =
        serde_json::from_value::<temper_benchmark_cli::BenchmarkComparisonV1>(comparison_json)
            .unwrap_err();
    assert!(error.to_string().contains("unknown field `surprise`"));
}

#[test]
fn aggregate_and_comparison_contracts_reject_inconsistent_counts() {
    let aggregate = aggregate_run_summaries([summary("run-a", 1, 10)]).unwrap();
    let mut aggregate_json = serde_json::to_value(&aggregate).unwrap();
    aggregate_json["outcomes"]["succeeded"] = serde_json::json!(0);
    aggregate_json["outcomes"]["failed"] = serde_json::json!(1);
    let error =
        serde_json::from_value::<temper_benchmark_cli::BenchmarkAggregateV1>(aggregate_json)
            .unwrap_err();
    assert!(error.to_string().contains("outcome counts"));

    let input = ComparisonInput::Aggregate(aggregate);
    let comparison = compare_benchmarks(&input, &input).unwrap();
    let mut comparison_json = serde_json::to_value(comparison).unwrap();
    comparison_json["primary"][0]["base"]["count"] = serde_json::json!(2);
    let error =
        serde_json::from_value::<temper_benchmark_cli::BenchmarkComparisonV1>(comparison_json)
            .unwrap_err();
    assert!(error.to_string().contains("more samples than subject runs"));
}

#[test]
fn comparison_preserves_unknown_metrics_and_separates_advisory_timings() {
    let mut base =
        aggregate_run_summaries([summary("base-1", 10, 100), summary("base-2", 20, 200)]).unwrap();
    base.metrics.insert(
        "future_score".to_string(),
        DistributionV1::from_values(vec![7, 9]).unwrap(),
    );
    let mut head =
        aggregate_run_summaries([summary("head-1", 30, 300), summary("head-2", 40, 400)]).unwrap();
    head.metrics.insert(
        "future_score".to_string(),
        DistributionV1::from_values(vec![11, 13]).unwrap(),
    );

    let comparison = compare_benchmarks(
        &ComparisonInput::Aggregate(base),
        &ComparisonInput::Aggregate(head),
    )
    .unwrap();
    assert_eq!(
        comparison
            .primary
            .iter()
            .find(|metric| metric.metric == "turns")
            .unwrap()
            .median_delta,
        Some(20)
    );
    assert_eq!(comparison.other[0].metric, "future_score");
    assert_eq!(comparison.other[0].median_delta, Some(4));

    let markdown = render_comparison_markdown(&comparison);
    assert!(markdown.contains("## Primary structural metrics"));
    assert!(markdown.contains("## Advisory timings"));
    assert!(markdown.contains("not pass/fail gates"));
    assert!(markdown.contains("## Additional metrics"));
    assert!(markdown.contains("| turns | 10 (n=2) | 30 (n=2) | +20 |"));
}

#[test]
fn comparison_input_resolves_run_and_aggregate_artifact_directories() {
    let temporary = tempfile::tempdir().unwrap();
    let run = summary("run", 2, 20);
    let run_dir = temporary.path().join("repetition");
    fs::create_dir(&run_dir).unwrap();
    fs::write(
        run_dir.join("run.json"),
        serde_json::to_vec_pretty(&run).unwrap(),
    )
    .unwrap();
    assert!(matches!(
        load_comparison_input(&run_dir).unwrap(),
        ComparisonInput::Run(_)
    ));

    let aggregate = aggregate_run_summaries([run]).unwrap();
    let aggregate_dir = temporary.path().join("artifact");
    fs::create_dir(&aggregate_dir).unwrap();
    fs::write(
        aggregate_dir.join("aggregate.json"),
        serde_json::to_vec_pretty(&aggregate).unwrap(),
    )
    .unwrap();
    assert!(matches!(
        load_comparison_input(&aggregate_dir).unwrap(),
        ComparisonInput::Aggregate(_)
    ));

    let missing = temporary.path().join("missing-artifacts");
    fs::create_dir(&missing).unwrap();
    assert!(
        load_comparison_input(missing)
            .unwrap_err()
            .to_string()
            .contains("neither aggregate.json nor run.json")
    );
}

#[test]
fn incompatible_benchmark_identities_fail_clearly() {
    let mut base = summary("base", 1, 1);
    base.benchmark = Some(BenchmarkRunV1 {
        name: "benchmark-a".to_string(),
        mode: BenchmarkModeV1::Harness,
        repetition: 1,
    });
    let mut head = summary("head", 1, 1);
    head.benchmark = Some(BenchmarkRunV1 {
        name: "benchmark-b".to_string(),
        mode: BenchmarkModeV1::Harness,
        repetition: 1,
    });
    let error =
        compare_benchmarks(&ComparisonInput::Run(base), &ComparisonInput::Run(head)).unwrap_err();
    assert!(error.to_string().contains("benchmark names differ"));
}

#[test]
fn environment_metadata_is_allowlisted_and_models_are_deduplicated() {
    let trace = ingest_trace(fixture("journal-complete")).unwrap();
    let mut first = trace.events[0].clone();
    first.event = AgentActivityEventV1::ModelCallStarted(ModelCallStartedV1 {
        call_id: "call-2".to_string(),
        provider: "provider-b".to_string(),
        model: "model-z".to_string(),
        attempt: 1,
    });
    let mut second = first.clone();
    second.event = AgentActivityEventV1::ModelCallStarted(ModelCallStartedV1 {
        call_id: "call-1".to_string(),
        provider: "provider-a".to_string(),
        model: "model-a".to_string(),
        attempt: 1,
    });
    let metadata = collect_environment_metadata(
        &[first, second.clone(), second],
        &BenchmarkAnnotationsV1 {
            provider_region: Some("test-region".to_string()),
            cache_warmth: Some("cold".to_string()),
        },
    );

    assert_eq!(metadata.temper.package_version, env!("CARGO_PKG_VERSION"));
    assert_eq!(metadata.observed_models.len(), 2);
    assert_eq!(metadata.observed_models[0].provider, "provider-a");
    assert_eq!(metadata.provider_region.as_deref(), Some("test-region"));
    let serialized = serde_json::to_string(&metadata).unwrap();
    assert!(!serialized.contains("hostname"));
    assert!(!serialized.contains("username"));
}

#[test]
fn compare_cli_is_report_only_but_rejects_malformed_input() {
    let temporary = tempfile::tempdir().unwrap();
    let base = temporary.path().join("base.json");
    let head = temporary.path().join("head.json");
    fs::write(
        &base,
        serde_json::to_vec_pretty(&summary("base", 1, 10)).unwrap(),
    )
    .unwrap();
    fs::write(
        &head,
        serde_json::to_vec_pretty(&summary("head", 1_000, 100_000)).unwrap(),
    )
    .unwrap();
    let output = temporary.path().join("comparison");

    let valid = Command::new(env!("CARGO_BIN_EXE_temper-benchmark"))
        .args([
            "compare",
            "--base",
            base.to_str().unwrap(),
            "--head",
            head.to_str().unwrap(),
            "--output-dir",
            output.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        valid.status.success(),
        "{}",
        String::from_utf8_lossy(&valid.stderr)
    );
    assert!(String::from_utf8_lossy(&valid.stdout).contains("+999"));
    assert!(output.join("comparison.json").is_file());
    assert!(output.join("comparison.md").is_file());

    let malformed = temporary.path().join("malformed.json");
    fs::write(&malformed, b"{not-json").unwrap();
    let invalid = Command::new(env!("CARGO_BIN_EXE_temper-benchmark"))
        .args([
            "compare",
            "--base",
            malformed.to_str().unwrap(),
            "--head",
            head.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(!invalid.status.success());
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("malformed JSON"));
}
