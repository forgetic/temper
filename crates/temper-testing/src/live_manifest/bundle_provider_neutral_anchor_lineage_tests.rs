use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use super::*;

#[test]
fn provider_neutral_lineage_bundle_maps_feature_1000_without_rewriting_feature_991() {
    let scenario_path = scenarios_root().join("provider-neutral-anchor-lineage");
    let bundle = ScenarioBundle::load(&scenario_path).expect("provider-neutral lineage bundle");
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
        } if fixture == "provider-neutral-anchor-lineage" && safe_tools == &vec![
            "search_graph".to_string(),
            "search_code".to_string(),
            "trace_path".to_string(),
            "get_code_snippet".to_string(),
            "list_projects".to_string(),
            "index_status".to_string(),
        ]
    ));
    let manifest = fs::read_to_string(scenario_path.join("scenario.toml"))
        .expect("provider-neutral lineage manifest");
    let readme = fs::read_to_string(scenario_path.join("README.md"))
        .expect("provider-neutral lineage README");
    let jig = fs::read_to_string(bundle.jig_script_path()).expect("provider-neutral lineage Jig");

    assert!(manifest.contains("feature = \"ai/temper#1000\""));
    assert_eq!(bundle.ci_poll_cadence, Duration::from_secs(1));
    assert_eq!(bundle.poll_cadence, Duration::from_secs(1));
    assert_eq!(bundle.mechanical_cadence, Duration::from_secs(1));
    assert!(manifest.contains("plan = \"ai/temper#1001\""));
    assert!(manifest.contains("source_branch = \"agent/pr-for-feature-1000\""));
    assert!(manifest.contains("typed-lineage-current-root-evidence-before-minimal-patch"));
    assert!(manifest.contains("complete-typed-lineage-correlations"));
    assert!(manifest.contains("graph_consumption_profile = \"provider-neutral typed lineage"));
    assert!(manifest.contains("model_forbidden_tools = [\"codebase_memory_index_repository\", \"codebase_memory_delete_project\"]"));
    assert!(manifest.contains("producer-turn calls, malformed or cross-root lineage"));
    assert!(!manifest.contains("file.path"));
    assert!(!manifest.contains("opaque-"));
    assert!(readme.contains("transformed typed representation"));
    assert!(readme.contains("bounded recovery exhaustion"));
    assert!(readme.contains("unavailable or systemic fallback"));
    assert!(readme.contains("Privacy-safe evidence"));
    assert!(jig.contains("provider-neutral-anchor-lineage-runtime"));
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
