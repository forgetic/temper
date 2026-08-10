use super::*;

#[test]
fn provider_result_anchor_bundle_maps_feature_991_without_rewriting_prior_graph_scenarios() {
    let scenario_path = scenarios_root().join("provider-result-anchor");
    let bundle = ScenarioBundle::load(&scenario_path).expect("provider-result-anchor bundle");
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
        } if fixture == "provider-result-anchor" && safe_tools == &vec![
            "search_graph".to_string(),
            "search_code".to_string(),
            "trace_path".to_string(),
            "get_code_snippet".to_string(),
            "list_projects".to_string(),
            "index_status".to_string(),
        ]
    ));
    let manifest = fs::read_to_string(scenario_path.join("scenario.toml"))
        .expect("provider-result-anchor manifest");
    let readme =
        fs::read_to_string(scenario_path.join("README.md")).expect("provider-result-anchor README");
    let jig = fs::read_to_string(bundle.jig_script_path()).expect("provider-result-anchor Jig");

    assert!(manifest.contains("feature = \"ai/temper#991\""));
    assert!(manifest.contains("plan = \"ai/temper#992\""));
    assert!(manifest.contains("source_branch = \"agent/pr-for-feature-991\""));
    assert!(
        manifest.contains("result-derived-anchor-trace-and-source-evidence-before-minimal-patch")
    );
    assert!(manifest.contains("complete-typed-v1-correlations"));
    assert!(!manifest.contains("file.path"));
    assert!(!manifest.contains("opaque-"));
    assert!(readme.contains("Unrelated or conventional substitution"));
    assert!(readme.contains("incomplete source evidence"));
    assert!(readme.contains("historical `codebase-memory-graph-consumption`"));
    assert!(readme.contains("active `sequential-graph-evidence`"));
    assert!(readme.contains("Privacy-safe evidence"));
    assert!(jig.contains("provider-result-anchor-runtime"));
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
