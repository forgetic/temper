use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::*;

#[test]
fn exact_source_selection_bundle_maps_feature_1139_without_rewriting_history() {
    let scenario_path = scenarios_root().join("mapped-live-exact-source-selection");
    let bundle = ScenarioBundle::load(&scenario_path).expect("exact source-selection bundle");
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
        } if fixture == "mapped-live-exact-source-selection" && safe_tools == &vec![
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

    let checked = temper_scenario_core::check_scenario(&scenario_path);
    assert!(checked.is_valid(), "{:#?}", checked.diagnostics);
    let mapping = checked
        .manifest
        .as_ref()
        .and_then(|scenario| scenario.feature_mapping.as_ref())
        .expect("feature mapping");
    assert_eq!(mapping.feature.to_string(), "ai/temper#1139");
    assert_eq!(
        mapping.plan.as_ref().map(ToString::to_string).as_deref(),
        Some("ai/temper#1140")
    );
    assert_eq!(mapping.source_branch, "agent/pr-for-feature-1139");
    assert_eq!(mapping.change.as_str(), "new");

    let manifest = fs::read_to_string(scenario_path.join("scenario.toml")).expect("manifest");
    let readme = fs::read_to_string(scenario_path.join("README.md")).expect("README");
    let corpus_readme = fs::read_to_string(scenarios_root().join("README.md")).expect("README");
    let jig = fs::read_to_string(bundle.jig_script_path()).expect("Jig");
    for expected in [
        "ten-successful-complete-v1-graph-results",
        "typed-sources-precede-interleaved-selection-and-repair",
        "malformed-selection-read-precedes-exact-selection",
        "exact-route-selection-read",
        "three-competing-generic-results",
        "selection-competing-forest-traversal",
        "graph.lineage.decision_evidence_kind",
    ] {
        assert!(manifest.contains(expected), "manifest omitted {expected}");
    }
    for expected in [
        "Privacy boundary",
        "10/10",
        "nine relevant",
        "selection` / `read",
        "repo/src/route.rs",
        "remain frozen",
        "mapped-live-focused-test-source-relevance",
    ] {
        assert!(readme.contains(expected), "README omitted {expected}");
    }
    assert!(
        corpus_readme.contains("Mapped live exact source-selection mapping"),
        "corpus README omitted the dedicated mapping"
    );
    assert!(jig.contains("mapped-live-exact-source-selection-runtime"));
    for forbidden in [
        "crate::",
        "opaque-",
        "qualified_name",
        "source\"",
        "credential",
        "diagnostic trace",
    ] {
        assert!(!jig.contains(forbidden), "Jig retained {forbidden}");
    }
    assert!(bundle.repo.ci_source.contains("cargo test --quiet"));
    assert!(bundle.repo.seed_path.join("src/route.rs").is_file());
    assert!(bundle.repo.seed_path.join("tests/alias_retry.rs").is_file());
}

#[test]
fn exact_source_selection_keeps_routing_benchmark_contract_frozen() {
    let benchmark_root = scenarios_root()
        .parent()
        .expect("scenarios has repository root")
        .join("benchmarks/agent-sessions/codebase-memory-routing-repair");
    let manifest = fs::read_to_string(benchmark_root.join("benchmark.toml"))
        .expect("routing-repair benchmark manifest");
    let expected_patch = fs::read_to_string(benchmark_root.join("expected.patch"))
        .expect("routing-repair expected patch");
    assert!(manifest.contains("expected_patch = \"expected.patch\""));
    assert!(manifest.contains("aggregate_privacy_forbidden_fragments"));
    assert!(expected_patch.contains("+    let routing_topic = attempt.affinity_topic();"));
}

fn scenarios_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("temper-testing lives under crates/temper-testing")
        .join("scenarios")
}
