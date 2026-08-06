// SPDX-License-Identifier: MPL-2.0

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use temper_benchmark_cli::{
    AnalyzeOptions, BenchmarkAnnotationsV1, BenchmarkModeV1, BenchmarkRunV1, ComparisonInput,
    DiffStatisticsV1, DistributionV1, GraphDecisionKindV1, GraphDecisionTargetV1, RunSummaryV1,
    ValidationEvidenceV1, aggregate_run_summaries, analyze_trace, collect_environment_metadata,
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

fn set_mutation_turn_metrics(
    summary: &mut RunSummaryV1,
    mutations: u64,
    values: Option<(u64, u64, u64)>,
) {
    let structure = summary.metrics.structure.as_mut().unwrap();
    structure.mutations = Some(mutations);
    structure.mutation_turns = values.map(|values| values.0);
    structure.single_mutation_turns = values.map(|values| values.1);
    structure.max_mutations_per_turn = values.map(|values| values.2);
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
fn mutation_turn_metrics_flow_through_aggregate_and_comparison_artifacts() {
    let mut base_available = summary("base-available", 4, 40);
    set_mutation_turn_metrics(&mut base_available, 3, Some((2, 1, 2)));
    let mut base_unavailable = summary("base-unavailable", 5, 50);
    set_mutation_turn_metrics(&mut base_unavailable, 3, None);
    let base = aggregate_run_summaries([base_available, base_unavailable]).unwrap();

    for (name, expected) in [
        ("mutation_turns", 2),
        ("single_mutation_turns", 1),
        ("max_mutations_per_turn", 2),
    ] {
        assert_eq!(
            base.metrics[name],
            DistributionV1 {
                count: 1,
                min: expected,
                p25: expected,
                median: expected,
                p75: expected,
                max: expected,
            }
        );
    }
    let aggregate_json = serde_json::to_value(&base).unwrap();
    assert_eq!(
        aggregate_json["metrics"]["mutation_turns"]["count"],
        serde_json::json!(1)
    );
    assert_eq!(
        aggregate_json["metrics"]["mutation_turns"]["median"],
        serde_json::json!(2)
    );
    let aggregate_markdown = render_aggregate_markdown(&base);
    assert!(aggregate_markdown.contains("| mutation turns | 1 | 2 | 2 | 2 | 2 | 2 |"));
    assert!(aggregate_markdown.contains("| single mutation turns | 1 | 1 | 1 | 1 | 1 | 1 |"));
    assert!(aggregate_markdown.contains("| max mutations per turn | 1 | 2 | 2 | 2 | 2 | 2 |"));

    let mut head_available = summary("head-available", 4, 40);
    set_mutation_turn_metrics(&mut head_available, 6, Some((4, 3, 3)));
    let mut head_unavailable = summary("head-unavailable", 5, 50);
    set_mutation_turn_metrics(&mut head_unavailable, 6, None);
    let head = aggregate_run_summaries([head_available, head_unavailable]).unwrap();
    let comparison = compare_benchmarks(
        &ComparisonInput::Aggregate(base),
        &ComparisonInput::Aggregate(head),
    )
    .unwrap();

    for (name, expected_delta) in [
        ("mutation_turns", 2),
        ("single_mutation_turns", 2),
        ("max_mutations_per_turn", 1),
    ] {
        let metric = comparison
            .primary
            .iter()
            .find(|metric| metric.metric == name)
            .unwrap();
        assert_eq!(metric.base.as_ref().unwrap().count, 1);
        assert_eq!(metric.head.as_ref().unwrap().count, 1);
        assert_eq!(metric.median_delta, Some(expected_delta));
    }
    let comparison_json = serde_json::to_value(&comparison).unwrap();
    let primary = comparison_json["primary"].as_array().unwrap();
    for name in [
        "mutation_turns",
        "single_mutation_turns",
        "max_mutations_per_turn",
    ] {
        let metric = primary
            .iter()
            .find(|metric| metric["metric"] == name)
            .unwrap();
        assert_eq!(metric["base"]["count"], serde_json::json!(1));
        assert_eq!(metric["head"]["count"], serde_json::json!(1));
    }
    let comparison_markdown = render_comparison_markdown(&comparison);
    assert!(comparison_markdown.contains("| mutation turns | 2 | 4 | +2 |"));
    assert!(comparison_markdown.contains("| single mutation turns | 1 | 3 | +2 |"));
    assert!(comparison_markdown.contains("| max mutations per turn | 2 | 3 | +1 |"));
}

#[test]
fn correctness_and_host_validation_distributions_retain_unavailable_trials() {
    let mut passing = summary("passing", 1, 10);
    passing.validation = Some(ValidationEvidenceV1 {
        command_count: 2,
        succeeded: 2,
        failed: 0,
    });
    let mut failing = summary("failing", 1, 10);
    failing.validation = Some(ValidationEvidenceV1 {
        command_count: 2,
        succeeded: 1,
        failed: 1,
    });
    let unavailable = summary("unavailable", 1, 10);

    let aggregate = aggregate_run_summaries([passing, failing, unavailable]).unwrap();
    assert_eq!(aggregate.outcomes.total, 3);
    assert_eq!(aggregate.metrics["task_correct"].count, 2);
    assert_eq!(aggregate.metrics["task_correct"].min, 0);
    assert_eq!(aggregate.metrics["task_correct"].max, 1);
    assert_eq!(aggregate.metrics["host_validation_commands"].count, 2);
    assert_eq!(aggregate.metrics["host_validation_failures"].count, 2);
}

#[test]
fn graph_distributions_and_pairwise_comparison_retain_partial_trial_coverage() {
    let targets = vec![GraphDecisionTargetV1 {
        target: "src/lib.rs".to_string(),
        kind: GraphDecisionKindV1::Implementation,
        result_contains: None,
    }];
    let mut complete = analyze_trace(
        &ingest_trace(fixture("graph-metrics-events.jsonl")).unwrap(),
        &AnalyzeOptions {
            discovery_command_prefixes: vec!["git grep".to_string()],
            graph_decision_targets: targets.clone(),
            ..AnalyzeOptions::default()
        },
    );
    complete.identity.run_id = "graph-complete".to_string();
    let mut partial = complete.clone();
    partial.identity.run_id = "graph-partial".to_string();
    partial
        .metrics
        .graph
        .as_mut()
        .unwrap()
        .readiness_wait_coverage
        .observed = 6;
    partial
        .metrics
        .graph
        .as_mut()
        .unwrap()
        .discovery_duration_coverage
        .observed = 6;
    let mut missing = analyze_trace(
        &ingest_trace(fixture("graph-missing-evidence-events.jsonl")).unwrap(),
        &AnalyzeOptions {
            graph_decision_targets: targets,
            ..AnalyzeOptions::default()
        },
    );
    missing.identity.run_id = "graph-missing".to_string();
    let base = aggregate_run_summaries([complete.clone(), partial, missing]).unwrap();

    assert_eq!(base.outcomes.total, 3);
    assert_eq!(base.metrics["graph_calls"].count, 3);
    assert_eq!(base.metrics["graph_relevant_results"].count, 2);
    assert_eq!(base.metrics["graph_readiness_wait_ms"].count, 1);
    assert_eq!(
        base.metrics["conventional_discovery_calls_before_selection"].count,
        2
    );
    let markdown = render_aggregate_markdown(&base);
    assert!(markdown.contains("| graph relevant results | 2 |"));
    assert!(markdown.contains("| graph readiness wait ms | 1 |"));

    let graph = complete.metrics.graph.as_mut().unwrap();
    graph.relevant_results = Some(2);
    graph.irrelevant_successes = Some(2);
    let head = aggregate_run_summaries([complete]).unwrap();
    let comparison = compare_benchmarks(
        &ComparisonInput::Aggregate(base),
        &ComparisonInput::Aggregate(head),
    )
    .unwrap();
    let relevance = comparison
        .primary
        .iter()
        .find(|metric| metric.metric == "graph_relevant_results")
        .unwrap();
    assert_eq!(relevance.base.as_ref().unwrap().count, 2);
    assert_eq!(relevance.head.as_ref().unwrap().count, 1);
    assert_eq!(relevance.median_delta, Some(1));
    assert!(
        render_comparison_markdown(&comparison)
            .contains("| graph relevant results | 1 (n=2) | 2 | +1 |")
    );
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
    assert!(markdown.contains("## Primary correctness, discovery, and structural metrics"));
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
        condition: None,
    });
    let mut head = summary("head", 1, 1);
    head.benchmark = Some(BenchmarkRunV1 {
        name: "benchmark-b".to_string(),
        mode: BenchmarkModeV1::Harness,
        repetition: 1,
        condition: None,
    });
    let error =
        compare_benchmarks(&ComparisonInput::Run(base), &ComparisonInput::Run(head)).unwrap_err();
    assert!(error.to_string().contains("benchmark names differ"));
}

#[test]
fn controlled_conditions_are_recorded_and_remain_pairwise_comparable() {
    let mut disabled = summary("disabled", 3, 30);
    disabled.benchmark = Some(BenchmarkRunV1 {
        name: "controlled".to_string(),
        mode: BenchmarkModeV1::Live,
        repetition: 1,
        condition: Some(temper_benchmark_cli::BenchmarkConditionV1::CodebaseMemoryDisabled),
    });
    let mut enabled = summary("enabled", 2, 20);
    enabled.benchmark = Some(BenchmarkRunV1 {
        name: "controlled".to_string(),
        mode: BenchmarkModeV1::Live,
        repetition: 1,
        condition: Some(temper_benchmark_cli::BenchmarkConditionV1::CodebaseMemoryEnabled),
    });

    let base = aggregate_run_summaries([disabled]).unwrap();
    let head = aggregate_run_summaries([enabled]).unwrap();
    assert_eq!(
        base.condition,
        Some(temper_benchmark_cli::BenchmarkConditionV1::CodebaseMemoryDisabled)
    );
    let comparison = compare_benchmarks(
        &ComparisonInput::Aggregate(base),
        &ComparisonInput::Aggregate(head),
    )
    .unwrap();
    assert_eq!(
        comparison.base.condition,
        Some(temper_benchmark_cli::BenchmarkConditionV1::CodebaseMemoryDisabled)
    );
    assert_eq!(
        comparison.head.condition,
        Some(temper_benchmark_cli::BenchmarkConditionV1::CodebaseMemoryEnabled)
    );
    let markdown = render_comparison_markdown(&comparison);
    assert!(markdown.contains("condition codebase_memory_disabled"));
    assert!(markdown.contains("condition codebase_memory_enabled"));
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
