use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use super::*;

#[test]
fn mapped_graph_consumption_bundle_maps_feature_1009_without_rewriting_history() {
    let scenario_path = scenarios_root().join("mapped-live-graph-consumption");
    let bundle = ScenarioBundle::load(&scenario_path).expect("mapped graph-consumption bundle");
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
        } if fixture == "mapped-live-graph-consumption" && safe_tools == &vec![
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
    let jig = fs::read_to_string(bundle.jig_script_path()).expect("Jig");
    assert!(manifest.contains("feature = \"ai/temper#1009\""));
    assert!(manifest.contains("plan = \"ai/temper#1010\""));
    assert!(manifest.contains("source_branch = \"agent/pr-for-feature-1009\""));
    assert!(manifest.contains("mapped-multi-part-lineage-before-minimal-repair"));
    assert!(manifest.contains("five-complete-v1-correlations-and-lineages"));
    assert!(manifest.contains("one-expected-unavailable-descendant"));
    assert!(manifest.contains("one-conventional-fallback-read"));
    assert!(manifest.contains("focused denial regressions"));
    assert!(manifest.contains("graph.lineage.stage"));
    assert!(!manifest.contains("crate::"));
    assert!(!manifest.contains("opaque-"));
    assert!(readme.contains("feature `ai/temper#1009`"));
    assert!(readme.contains("historical feature `#991` or `#1000`"));
    assert!(readme.contains("unavailable descendant"));
    assert!(readme.contains("does not bypass an active anchor"));
    assert!(readme.contains("Privacy-safe evidence"));
    assert!(jig.contains("mapped-live-graph-consumption-runtime"));
    assert!(!jig.contains("crate::"));
    assert!(!jig.contains("opaque-"));
    assert!(bundle.repo.ci_source.contains("cargo test --quiet"));
}

fn scenarios_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("temper-testing lives under crates/temper-testing")
        .join("scenarios")
}
