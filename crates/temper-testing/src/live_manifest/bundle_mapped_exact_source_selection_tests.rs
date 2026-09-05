use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::*;

#[test]
fn exact_source_selection_maps_feature_1263_and_retains_prior_audit() {
    let scenario_path = scenarios_root().join("mapped-live-exact-source-selection");
    let bundle = ScenarioBundle::load(&scenario_path).expect("post-correction bundle");
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
            "get_architecture".to_string(),
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
    assert_eq!(mapping.feature.to_string(), "ai/temper#1263");
    assert_eq!(
        mapping.plan.as_ref().map(ToString::to_string).as_deref(),
        Some("ai/temper#1264")
    );
    assert_eq!(mapping.source_branch, "agent/pr-for-feature-1263");
    assert_eq!(mapping.change.as_str(), "new");

    let manifest = fs::read_to_string(scenario_path.join("scenario.toml")).expect("manifest");
    let readme = fs::read_to_string(scenario_path.join("README.md")).expect("README");
    let corpus_readme = fs::read_to_string(scenarios_root().join("README.md")).expect("README");
    let jig = fs::read_to_string(bundle.jig_script_path()).expect("Jig");
    for expected in [
        "introduced_by = \"#1144\"",
        "ten-successful-complete-v1-graph-results",
        "two-retained-independent-roots",
        "four-local-initial-noncredit-attempts-share-snapshot",
        "initial-sibling-root-caller-denied",
        "initial-broad-search-denied",
        "initial-malformed-selector-denied",
        "initial-unpresented-selector-denied",
        "read-provisional-model-before-inspection",
        "graph.lineage.implementation_correction_available",
        "graph.lineage.implementation_authority_corrected",
        "stale-selected-correction-denied",
        "old-model-authority-mutation-denied",
        "read-exact-corrected-route",
        "one-matching-minimal-mutation",
        "one-workspace-diff-after-authorized-mutation",
    ] {
        assert!(manifest.contains(expected), "manifest omitted {expected}");
    }
    for historical in [
        "ai/temper#1210",
        "ai/temper#1211",
        "agent/pr-for-feature-1210",
        "ai/temper#1151",
        "ai/temper#1152",
        "agent/pr-for-feature-1151",
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
    for expected in [
        "Privacy boundary",
        "provisional `src/model.rs`",
        "src/route.rs::worker_slot",
        "correction_inspection_required",
        "repo/src/route.rs",
        "cargo dev-benchmark-harness",
        "cargo dev-scenario-check",
        "cargo dev-scenario-run",
        "broader #1210 acceptance matrix",
        "do not address repetition",
        "one wholly fresh",
    ] {
        assert!(readme.contains(expected), "README omitted {expected}");
    }
    assert!(
        corpus_readme.contains("Mapped live post-correction authorization mapping"),
        "corpus README omitted the updated mapping"
    );
    assert!(jig.contains("mapped-live-post-correction-authorization-runtime"));
    for forbidden in [
        "crate::",
        "opaque-",
        "qualified_name",
        "source\"",
        "mutation arguments",
        "diagnostic trace",
    ] {
        assert!(!jig.contains(forbidden), "Jig retained {forbidden}");
    }
    assert!(bundle.repo.ci_source.contains("cargo test --quiet"));
    assert!(bundle.repo.seed_path.join("src/model.rs").is_file());
    assert!(bundle.repo.seed_path.join("src/route.rs").is_file());
    assert!(bundle.repo.seed_path.join("tests/alias_retry.rs").is_file());
}

#[test]
fn exact_source_selection_keeps_routing_benchmark_and_controls_frozen() {
    let benchmark_root = scenarios_root()
        .parent()
        .expect("scenarios has repository root")
        .join("benchmarks/agent-sessions/codebase-memory-routing-repair");
    let benchmark = fs::read_to_string(benchmark_root.join("benchmark.toml"))
        .expect("routing-repair benchmark manifest");
    let enabled = fs::read_to_string(benchmark_root.join("jig.json")).expect("enabled Jig");
    let disabled =
        fs::read_to_string(benchmark_root.join("jig-disabled.json")).expect("disabled Jig");
    let unavailable =
        fs::read_to_string(benchmark_root.join("jig-unavailable.json")).expect("unavailable Jig");
    let expected_patch = fs::read_to_string(benchmark_root.join("expected.patch"))
        .expect("routing-repair expected patch");

    for expected in [
        "expected_patch = \"expected.patch\"",
        "matrix_repetitions = 5",
        "minimum_relevance_percent = 50",
        "minimum_improvement_percent = 20",
        "exact_source_selection_target = \"repo/src/route.rs\"",
        "required_decision_kinds = [\"implementation\", \"caller\", \"focused_test\"]",
        "required_consumption_modes = [\"source\", \"selection\"]",
        "aggregate_privacy_forbidden_fragments",
    ] {
        assert!(benchmark.contains(expected), "benchmark omitted {expected}");
    }
    for expected in [
        "recovery_cross_root_caller_denied",
        "recovery_active_root_trace",
        "recovery_active_root_implementation",
        "read_route_after_source_chain",
        "patch_retry_affinity",
    ] {
        assert!(enabled.contains(expected), "enabled Jig omitted {expected}");
    }
    assert!(!disabled.contains("codebase_memory_"));
    let unavailable_graph = unavailable
        .find("graph_find_affinity_unavailable")
        .expect("one unavailable graph attempt");
    let unavailable_shell = unavailable
        .find("compound_shell_fallback_after_unavailable")
        .expect("conventional fallback after unavailability");
    assert!(unavailable_graph < unavailable_shell);
    assert_eq!(unavailable.matches("codebase_memory_").count(), 1);
    assert!(expected_patch.contains("+    let routing_topic = attempt.affinity_topic();"));
}

fn scenarios_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("temper-testing lives under crates/temper-testing")
        .join("scenarios")
}
