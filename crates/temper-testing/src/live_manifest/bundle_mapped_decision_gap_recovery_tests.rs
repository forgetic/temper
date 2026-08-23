use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::*;
use temper_protocol_activity::{
    GraphExplorationClosedReasonV1, GraphExplorationClosedV1, GraphRecoveryEvidenceKindV1,
    GraphRecoveryPermittedActionV1,
};

#[test]
fn decision_gap_recovery_bundle_maps_feature_1091_and_retains_1069_audit() {
    let scenario_path = scenarios_root().join("mapped-live-decision-gap-recovery");
    let bundle = ScenarioBundle::load(&scenario_path).expect("decision-gap recovery bundle");
    let mcp = bundle
        .execution
        .steps
        .iter()
        .find(|step| step.id == "start-fake-codebase-memory-mcp")
        .expect("MCP fixture action");
    assert!(matches!(
        &mcp.action,
        ManifestAction::StartCodebaseMemoryMcp {
            fixture: Some(fixture),
            safe_tools,
            readiness_delay_ms: 750,
            forced_systemic_failure: None,
            ..
        } if fixture == "mapped-live-decision-gap-recovery" && safe_tools == &vec![
            "search_graph".to_string(),
            "search_code".to_string(),
            "trace_path".to_string(),
            "get_code_snippet".to_string(),
            "list_projects".to_string(),
            "index_status".to_string(),
        ]
    ));
    assert_eq!(bundle.ci_poll_cadence, Duration::from_secs(1));
    assert_eq!(bundle.poll_cadence, Duration::from_secs(1));
    assert_eq!(bundle.mechanical_cadence, Duration::from_secs(1));

    let manifest = fs::read_to_string(scenario_path.join("scenario.toml")).expect("manifest");
    let readme = fs::read_to_string(scenario_path.join("README.md")).expect("README");
    let corpus_readme = fs::read_to_string(scenarios_root().join("README.md")).expect("README");
    let jig = fs::read_to_string(bundle.jig_script_path()).expect("Jig");
    let checked = temper_scenario_core::check_scenario(&scenario_path);
    assert!(checked.is_valid(), "{:#?}", checked.diagnostics);
    let mapping = checked
        .manifest
        .as_ref()
        .and_then(|scenario| scenario.feature_mapping.as_ref())
        .expect("feature mapping");
    assert_eq!(mapping.feature.to_string(), "ai/temper#1091");
    assert_eq!(
        mapping.plan.as_ref().map(ToString::to_string).as_deref(),
        Some("ai/temper#1092")
    );
    assert_eq!(mapping.source_branch, "agent/pr-for-feature-1091");
    assert_eq!(mapping.change.as_str(), "updated");
    for expected in [
        "introduced_by = \"#1075\"",
        "immutable-cross-root-recovery-diagnostic",
        "trace-progress-reports-actual-remaining-kinds",
        "two-cross-root-local-denials-share-pre-batch-snapshot",
        "one-satisfied-trace-local-denial-after-progress",
        "one-minimal-mutation",
        "excluded_never_executed_local_policy_denial",
        "tool.failure.graph.missing_evidence",
        "graph.lineage.decision_evidence_kind",
        "no-compatible-action stop_without_product",
    ] {
        assert!(manifest.contains(expected), "manifest omitted {expected}");
    }
    for historical in [
        "ai/temper#1069",
        "ai/temper#1070",
        "agent/pr-for-feature-1069",
    ] {
        assert!(
            manifest.contains(historical),
            "manifest omitted historical {historical}"
        );
        assert!(
            readme.contains(historical),
            "README omitted historical {historical}"
        );
        assert!(
            corpus_readme.contains(historical),
            "corpus README omitted historical {historical}"
        );
    }
    assert!(readme.contains("historical"));
    assert!(readme.contains("`mapped-live-graph-consumption`"));
    assert!(readme.contains("`mapped-live-graph-convergence`"));
    assert!(readme.contains("`mapped-live-ordinary-tool-convergence`"));
    assert!(readme.contains("Privacy-safe evidence"));
    assert!(jig.contains("mapped-live-decision-gap-recovery-runtime"));
    assert!(readme.contains("one wholly fresh enabled smoke"));
    assert!(readme.contains("--feature ai/temper#1091"));
    assert!(readme.contains("--source-branch agent/pr-for-feature-1091"));
    assert!(corpus_readme.contains("`ai/temper#1091` and plan `ai/temper#1092`"));
    assert!(readme.contains("forced-unavailable repetitions"));
    assert!(readme.contains("at least 50% typed relevance"));
    assert!(readme.contains("at least 20% enabled median discovery improvement"));
    assert!(readme.contains("byte-exact patch"));
    assert!(readme.contains("exact-commit gates"));
    for forbidden in ["crate::", "opaque-", "provider output", "diagnostic trace"] {
        assert!(!jig.contains(forbidden), "Jig retained {forbidden}");
    }

    let benchmark_root = scenarios_root()
        .parent()
        .expect("scenarios has repository root")
        .join("benchmarks/agent-sessions/codebase-memory-routing-repair");
    let benchmark = fs::read_to_string(benchmark_root.join("benchmark.toml"))
        .expect("routing-repair benchmark manifest");
    let unavailable = fs::read_to_string(benchmark_root.join("jig-unavailable.json"))
        .expect("unavailable benchmark Jig");
    let expected_patch = fs::read_to_string(benchmark_root.join("expected.patch"))
        .expect("byte-exact benchmark patch");
    for acceptance in [
        "minimum_relevance_percent = 50",
        "minimum_improvement_percent = 20",
        "expected_patch = \"expected.patch\"",
        "post_run_commands",
        "aggregate_privacy_forbidden_fragments",
    ] {
        assert!(
            benchmark.contains(acceptance),
            "benchmark omitted {acceptance}"
        );
    }
    let unavailable_graph = unavailable
        .find("graph_find_affinity_unavailable")
        .expect("one unavailable graph attempt");
    let unavailable_shell = unavailable
        .find("compound_shell_fallback_after_unavailable")
        .expect("conventional fallback after unavailability");
    assert!(unavailable_graph < unavailable_shell);
    assert_eq!(unavailable.matches("codebase_memory_").count(), 1);
    assert!(expected_patch.contains("+    let routing_topic = attempt.affinity_topic();"));
    assert!(bundle.repo.ci_source.contains("cargo test --quiet"));
    assert!(
        fs::read_to_string(bundle.repo.seed_path.join(".gitignore"))
            .expect("fixture gitignore")
            .lines()
            .any(|line| line == "/Cargo.lock"),
        "fixture validation must not add generated lock evidence to the repair diff"
    );
}

#[test]
fn decision_gap_recovery_bundle_retains_closed_no_compatible_action_contract() {
    let details = GraphExplorationClosedV1::exhausted([
        GraphRecoveryEvidenceKindV1::Implementation,
        GraphRecoveryEvidenceKindV1::Caller,
        GraphRecoveryEvidenceKindV1::FocusedTest,
    ])
    .expect("safe stop details");
    assert_eq!(
        details.reason,
        GraphExplorationClosedReasonV1::RecoveryExhausted
    );
    assert_eq!(
        details.missing_evidence,
        [
            GraphRecoveryEvidenceKindV1::Implementation,
            GraphRecoveryEvidenceKindV1::Caller,
            GraphRecoveryEvidenceKindV1::FocusedTest,
        ]
    );
    assert_eq!(
        details.permitted_action,
        GraphRecoveryPermittedActionV1::StopWithoutProduct
    );
    assert_eq!(details.remaining_allowance, 0);
    assert!(details.compatible_actions.is_empty());
    assert!(details.model_message().contains("stop_without_product"));
    assert!(
        temper_agent::CodingAgentError::DecisionAnchorRecoveryExhausted
            .to_string()
            .contains("nothing to land")
    );
}

fn scenarios_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("temper-testing lives under crates/temper-testing")
        .join("scenarios")
}
