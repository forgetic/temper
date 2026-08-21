// SPDX-License-Identifier: MPL-2.0

use std::fs;
use std::path::{Path, PathBuf};
use temper_benchmark_cli::{
    BenchmarkAcceptanceOptions, BenchmarkConditionV1, BenchmarkModeV1, BenchmarkRunV1,
    ConventionalDiscoveryMetricsV1, DiffStatisticsV1, ExactPatchEvidenceV1, GraphConsumptionModeV1,
    GraphDecisionEvidenceV1, GraphDecisionKindV1, GraphEvidenceToolV1, GraphMetricsV1,
    HostMetadataV1, MetricCoverageV1, ObservedModelIdentityV1, TemperBuildMetadataV1,
    ValidationArtifactV1, ValidationCommandEvidenceV1, ValidationEvidenceV1,
    aggregate_run_summaries, ingest_trace, verify_benchmark_acceptance,
};
use temper_protocol_agent::WorkspaceResult;

const CANDIDATE: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const BENCHMARK: &str = "codebase-memory-routing-repair";
const PROVIDER: &str = "openai-codex";
const MODEL: &str = "gpt-5.6-sol";
const EXACT_SOURCE: &str = "repo/src/route.rs";

fn benchmark_manifest() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../benchmarks/agent-sessions/codebase-memory-routing-repair/benchmark.toml")
}

#[derive(Clone)]
pub(super) struct MatrixConfig {
    pub(super) enabled_relevance: [u64; 5],
    pub(super) enabled_succeeded: u64,
    pub(super) incomplete_relevance: bool,
    pub(super) incorrect_enabled: bool,
    pub(super) duplicate_trial: bool,
    pub(super) identity_drift: bool,
    pub(super) unavailable_retry: bool,
    pub(super) incomplete_classification: bool,
    pub(super) missing_improvement_sample: bool,
    pub(super) fail_exact_patch: bool,
    pub(super) fail_host_validation: bool,
}

impl Default for MatrixConfig {
    fn default() -> Self {
        Self {
            enabled_relevance: [10; 5],
            enabled_succeeded: 20,
            incomplete_relevance: false,
            incorrect_enabled: false,
            duplicate_trial: false,
            identity_drift: false,
            unavailable_retry: false,
            incomplete_classification: false,
            missing_improvement_sample: false,
            fail_exact_patch: false,
            fail_host_validation: false,
        }
    }
}

pub(super) struct EvidenceFixture {
    _temporary: tempfile::TempDir,
    pub(super) options: BenchmarkAcceptanceOptions,
}

impl EvidenceFixture {
    pub(super) fn new(config: MatrixConfig) -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let smoke = temporary.path().join("smoke");
        let enabled = temporary.path().join("enabled");
        let disabled = temporary.path().join("disabled");
        let unavailable = temporary.path().join("unavailable");
        let manifest = benchmark_manifest();

        write_set(
            &smoke,
            &manifest,
            BenchmarkConditionV1::CodebaseMemoryEnabled,
            1,
            |index| {
                let mut run = enabled_run("smoke", index, 6, 3, 4);
                if config.fail_exact_patch {
                    run.validation.exact_patch.as_mut().unwrap().status = "failed".to_string();
                    set_validation_summary(&mut run.summary, &run.validation);
                }
                if config.fail_host_validation {
                    run.validation.post_run_commands[0].status = "failed".to_string();
                    run.validation.post_run_commands[0].exit_code = Some(1);
                    set_validation_summary(&mut run.summary, &run.validation);
                }
                run
            },
        );
        write_set(
            &enabled,
            &manifest,
            BenchmarkConditionV1::CodebaseMemoryEnabled,
            5,
            |index| {
                let mut run = enabled_run(
                    "enabled",
                    index,
                    config.enabled_succeeded,
                    config.enabled_relevance[index as usize - 1],
                    4,
                );
                if index == 1 && config.incomplete_relevance {
                    let graph = run.summary.metrics.graph.as_mut().unwrap();
                    graph.relevance_coverage.observed = graph.succeeded - 1;
                    graph.relevant_results = None;
                    graph.irrelevant_successes = None;
                }
                if index == 1 && config.incorrect_enabled {
                    run.summary.terminal.as_mut().unwrap().status =
                        temper_benchmark_cli::RunTerminalStatusV1::Failed;
                }
                if index == 1 && config.identity_drift {
                    run.summary.host.as_mut().unwrap().observed_models[0].model =
                        "different-model".to_string();
                }
                run
            },
        );
        write_set(
            &disabled,
            &manifest,
            BenchmarkConditionV1::CodebaseMemoryDisabled,
            5,
            |index| {
                let mut run = control_run("disabled", index, false, 5);
                if index == 1 && config.duplicate_trial {
                    run.summary.identity.run_id = "enabled-run-001".to_string();
                    run.summary.identity.agent_session_id = Some("enabled-session-001".to_string());
                }
                if index == 1 && config.incomplete_classification {
                    make_classification_incomplete(&mut run.summary);
                }
                if index == 1 && config.missing_improvement_sample {
                    run.summary
                        .metrics
                        .graph
                        .as_mut()
                        .unwrap()
                        .conventional_discovery_before_selection
                        .as_mut()
                        .unwrap()
                        .total_calls = None;
                }
                run
            },
        );
        write_set(
            &unavailable,
            &manifest,
            BenchmarkConditionV1::CodebaseMemoryUnavailable,
            5,
            |index| {
                let mut run = control_run("unavailable", index, true, 5);
                if index == 1 && config.unavailable_retry {
                    run.summary
                        .metrics
                        .graph
                        .as_mut()
                        .unwrap()
                        .immediate_repeated_attempts_after_systemic_failure = Some(1);
                }
                run
            },
        );

        let options = BenchmarkAcceptanceOptions {
            benchmark: manifest,
            candidate_commit: CANDIDATE.to_string(),
            smoke,
            enabled,
            disabled,
            unavailable,
            output_dir: temporary.path().join("acceptance"),
        };
        Self {
            _temporary: temporary,
            options,
        }
    }

    pub(super) fn verify(&self) -> temper_benchmark_cli::BenchmarkAcceptanceV1 {
        verify_benchmark_acceptance(&self.options).unwrap()
    }
}

struct TrialRun {
    summary: temper_benchmark_cli::RunSummaryV1,
    validation: ValidationArtifactV1,
}

fn enabled_run(
    prefix: &str,
    repetition: u32,
    succeeded: u64,
    relevant: u64,
    discovery: u64,
) -> TrialRun {
    let mut run = base_run(
        prefix,
        repetition,
        BenchmarkConditionV1::CodebaseMemoryEnabled,
    );
    run.summary.metrics.graph = Some(GraphMetricsV1 {
        calls: succeeded,
        succeeded,
        failed: 0,
        cancelled: 0,
        failures_by_category: Default::default(),
        failures_by_reason: Default::default(),
        status_coverage: coverage(succeeded, succeeded),
        failure_category_coverage: coverage(0, 0),
        failure_reason_coverage: Some(coverage(0, 0)),
        cumulative_readiness_wait_ms: Some(succeeded),
        readiness_wait_coverage: coverage(succeeded, succeeded),
        cumulative_discovery_duration_ms: Some(succeeded * 2),
        discovery_duration_coverage: coverage(succeeded, succeeded),
        immediate_repeated_attempts_after_systemic_failure: Some(0),
        immediate_repeat_coverage: coverage(0, 0),
        relevant_results: Some(relevant),
        irrelevant_successes: Some(succeeded - relevant),
        relevance_coverage: coverage(succeeded, succeeded),
        typed_correlation_coverage: Some(coverage(succeeded, succeeded)),
        typed_lineage_coverage: Some(coverage(succeeded, succeeded)),
        decision_evidence: decision_evidence(prefix, repetition),
        conventional_discovery_before_selection: Some(discovery_metrics(discovery)),
    });
    run
}

fn control_run(prefix: &str, repetition: u32, unavailable: bool, discovery: u64) -> TrialRun {
    let condition = if unavailable {
        BenchmarkConditionV1::CodebaseMemoryUnavailable
    } else {
        BenchmarkConditionV1::CodebaseMemoryDisabled
    };
    let mut run = base_run(prefix, repetition, condition);
    let calls = u64::from(unavailable);
    run.summary.metrics.graph = Some(GraphMetricsV1 {
        calls,
        succeeded: 0,
        failed: calls,
        cancelled: 0,
        failures_by_category: Default::default(),
        failures_by_reason: Default::default(),
        status_coverage: coverage(calls, calls),
        failure_category_coverage: coverage(calls, calls),
        failure_reason_coverage: Some(coverage(calls, calls)),
        cumulative_readiness_wait_ms: Some(0),
        readiness_wait_coverage: coverage(calls, calls),
        cumulative_discovery_duration_ms: Some(0),
        discovery_duration_coverage: coverage(calls, calls),
        immediate_repeated_attempts_after_systemic_failure: Some(0),
        immediate_repeat_coverage: coverage(calls, calls),
        relevant_results: Some(0),
        irrelevant_successes: Some(0),
        relevance_coverage: coverage(0, 0),
        typed_correlation_coverage: Some(coverage(0, 0)),
        typed_lineage_coverage: Some(coverage(0, 0)),
        decision_evidence: Vec::new(),
        conventional_discovery_before_selection: Some(discovery_metrics(discovery)),
    });
    run
}

fn base_run(prefix: &str, repetition: u32, condition: BenchmarkConditionV1) -> TrialRun {
    let mut summary =
        ingest_trace(Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/journal-complete"))
            .unwrap()
            .run_summary();
    summary.identity.run_id = format!("{prefix}-run-{repetition:03}");
    summary.identity.agent_session_id = Some(format!("{prefix}-session-{repetition:03}"));
    summary.benchmark = Some(BenchmarkRunV1 {
        name: BENCHMARK.to_string(),
        mode: BenchmarkModeV1::Live,
        repetition,
        condition: Some(condition),
    });
    summary.capture = Some(temper_protocol_activity::CaptureModeV1::Diagnostic);
    summary.host = Some(HostMetadataV1 {
        temper: TemperBuildMetadataV1 {
            package_version: "test".to_string(),
            commit: Some(CANDIDATE.to_string()),
        },
        observed_models: vec![ObservedModelIdentityV1 {
            provider: PROVIDER.to_string(),
            model: MODEL.to_string(),
        }],
        os: "test".to_string(),
        architecture: "test".to_string(),
        logical_cpu_count: None,
        cpu_model: None,
        load_average: None,
        provider_region: Some("loopback".to_string()),
        cache_warmth: Some("not-applicable".to_string()),
    });
    summary.workspace_result = Some(
        serde_json::from_value::<WorkspaceResult>(serde_json::json!({
            "title": "Benchmark result",
            "body": "# Implementation report",
            "summary": "Validated"
        }))
        .unwrap(),
    );
    summary.diff = Some(DiffStatisticsV1 {
        files_changed: 1,
        insertions: 1,
        deletions: 1,
        tracked_files: 1,
        untracked_files: 0,
    });
    summary.wall_time_ms = Some(100);
    let validation = passing_validation();
    set_validation_summary(&mut summary, &validation);
    TrialRun {
        summary,
        validation,
    }
}

fn passing_validation() -> ValidationArtifactV1 {
    ValidationArtifactV1 {
        version: 1,
        accepted_submit: None,
        exact_patch: Some(ExactPatchEvidenceV1 {
            expected_patch: "expected.patch".to_string(),
            status: "passed".to_string(),
            untracked_files: 0,
            diagnostic: None,
        }),
        post_run_commands: vec![ValidationCommandEvidenceV1 {
            argv: vec![
                "cargo".to_string(),
                "test".to_string(),
                "--quiet".to_string(),
                "--manifest-path".to_string(),
                "repo/Cargo.toml".to_string(),
                "--target-dir".to_string(),
                ".benchmark-target".to_string(),
            ],
            cwd: "workspace".to_string(),
            status: "passed".to_string(),
            exit_code: Some(0),
            timed_out: false,
            duration_ms: 1,
            stdout_tail: String::new(),
            stderr_tail: String::new(),
            stdout_dropped_bytes: 0,
            stderr_dropped_bytes: 0,
        }],
    }
}

fn set_validation_summary(
    summary: &mut temper_benchmark_cli::RunSummaryV1,
    validation: &ValidationArtifactV1,
) {
    let exact = validation
        .exact_patch
        .as_ref()
        .is_some_and(|patch| patch.status == "passed") as u64;
    let commands = validation
        .post_run_commands
        .iter()
        .filter(|command| command.status == "passed")
        .count() as u64;
    summary.validation = Some(ValidationEvidenceV1 {
        command_count: 2,
        succeeded: exact + commands,
        failed: 2 - exact - commands,
    });
}

fn decision_evidence(prefix: &str, repetition: u32) -> Vec<GraphDecisionEvidenceV1> {
    vec![
        evidence(
            prefix,
            repetition,
            1,
            GraphDecisionKindV1::Implementation,
            GraphConsumptionModeV1::Selection,
            GraphEvidenceToolV1::Read,
            EXACT_SOURCE,
        ),
        evidence(
            prefix,
            repetition,
            2,
            GraphDecisionKindV1::Caller,
            GraphConsumptionModeV1::Source,
            GraphEvidenceToolV1::GetCodeSnippet,
            "DeliveryAttempt",
        ),
        evidence(
            prefix,
            repetition,
            3,
            GraphDecisionKindV1::FocusedTest,
            GraphConsumptionModeV1::Source,
            GraphEvidenceToolV1::GetCodeSnippet,
            "repo/tests/alias_retry.rs",
        ),
    ]
}

fn evidence(
    prefix: &str,
    repetition: u32,
    order: u64,
    kind: GraphDecisionKindV1,
    consumption_mode: GraphConsumptionModeV1,
    consumer_tool: GraphEvidenceToolV1,
    target: &str,
) -> GraphDecisionEvidenceV1 {
    GraphDecisionEvidenceV1 {
        graph_call_id: format!("{prefix}-{repetition}-graph-{order}"),
        graph_finish_seq: order * 2,
        graph_tool: GraphEvidenceToolV1::SearchGraph,
        consumer_call_id: format!("{prefix}-{repetition}-consumer-{order}"),
        consumer_start_seq: order * 2 + 1,
        consumer_tool,
        consumption_mode,
        target: target.to_string(),
        kind,
    }
}

fn coverage(observed: u64, expected: u64) -> MetricCoverageV1 {
    MetricCoverageV1 {
        observed,
        expected: Some(expected),
    }
}

fn discovery_metrics(total: u64) -> ConventionalDiscoveryMetricsV1 {
    ConventionalDiscoveryMetricsV1 {
        grep_calls: 0,
        find_calls: 0,
        read_calls: total,
        classified_shell_segments: 0,
        total_calls: Some(total),
        shell_command_classification_coverage: coverage(1, 1),
    }
}

fn make_classification_incomplete(summary: &mut temper_benchmark_cli::RunSummaryV1) {
    let discovery = summary
        .metrics
        .graph
        .as_mut()
        .unwrap()
        .conventional_discovery_before_selection
        .as_mut()
        .unwrap();
    discovery.total_calls = None;
    discovery.shell_command_classification_coverage.observed = 0;
}

fn write_set(
    root: &Path,
    manifest: &Path,
    condition: BenchmarkConditionV1,
    repetitions: u32,
    mut run: impl FnMut(u32) -> TrialRun,
) {
    fs::create_dir_all(root.join("repetitions")).unwrap();
    let runs = (1..=repetitions).map(&mut run).collect::<Vec<_>>();
    let aggregate = aggregate_run_summaries(runs.iter().map(|run| run.summary.clone())).unwrap();
    assert_eq!(aggregate.condition, Some(condition));
    write_json(&root.join("aggregate.json"), &aggregate);
    fs::write(root.join("aggregate.md"), "# Aggregate\n").unwrap();

    let manifest_root = manifest.parent().unwrap();
    for (index, run) in runs.into_iter().enumerate() {
        let repetition = index as u32 + 1;
        let directory = root.join("repetitions").join(format!("{repetition:03}"));
        fs::create_dir(&directory).unwrap();
        fs::copy(manifest, directory.join("manifest.toml")).unwrap();
        fs::copy(
            manifest_root.join("workspace-context.json"),
            directory.join("workspace-context.json"),
        )
        .unwrap();
        fs::copy(
            manifest_root.join("expected.patch"),
            directory.join("expected.patch"),
        )
        .unwrap();
        write_json(
            &directory.join("baselines.json"),
            &serde_json::json!({
                "version": 1,
                "repetition": repetition,
                "repositories": [{
                    "id": "fixture",
                    "dir": "repo",
                    "sha": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                }]
            }),
        );
        write_json(&directory.join("run.json"), &run.summary);
        fs::write(directory.join("run.md"), "# Run\n").unwrap();
        fs::write(directory.join("trace.export.jsonl"), "{}\n").unwrap();
        write_json(&directory.join("validation.json"), &run.validation);
        write_json(
            &directory.join("diff.json"),
            &serde_json::json!({
                "version": 1,
                "statistics": run.summary.diff,
                "repositories": []
            }),
        );
        if let Some(result) = &run.summary.workspace_result {
            write_json(&directory.join("workspace-result.json"), result);
        }
    }
}

fn write_json(path: &Path, value: &impl serde::Serialize) {
    let mut bytes = serde_json::to_vec_pretty(value).unwrap();
    bytes.push(b'\n');
    fs::write(path, bytes).unwrap();
}
