use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use super::*;

#[test]
fn ordinary_convergence_bundle_maps_feature_1041_without_rewriting_history() {
    let scenario_path = scenarios_root().join("mapped-live-ordinary-tool-convergence");
    let bundle = ScenarioBundle::load(&scenario_path).expect("ordinary convergence bundle");
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
        } if fixture == "mapped-live-ordinary-tool-convergence" && safe_tools == &vec![
            "search_graph".to_string(),
            "search_code".to_string(),
            "trace_path".to_string(),
            "get_code_snippet".to_string(),
            "list_projects".to_string(),
            "index_status".to_string(),
        ]
    ));
    assert_eq!(bundle.agent_provider, "anthropic");
    assert_eq!(bundle.ci_poll_cadence, Duration::from_secs(1));
    assert_eq!(bundle.poll_cadence, Duration::from_secs(1));
    assert_eq!(bundle.mechanical_cadence, Duration::from_secs(1));

    let manifest = fs::read_to_string(scenario_path.join("scenario.toml")).expect("manifest");
    let readme = fs::read_to_string(scenario_path.join("README.md")).expect("README");
    let jig = fs::read_to_string(bundle.jig_script_path()).expect("Jig");
    assert!(manifest.contains("feature = \"ai/temper#1041\""));
    assert!(manifest.contains("plan = \"ai/temper#1042\""));
    assert!(manifest.contains("source_branch = \"agent/pr-for-feature-1041\""));
    assert!(manifest.contains("graph-closure-ordinary-repair-and-submission-converge"));
    assert!(manifest.contains("schema_argument_mismatch"));
    assert!(manifest.contains("repeated_non_retryable"));
    assert!(manifest.contains("two-source-reads-before-one-local-closure-with-no-provider-retry"));
    assert!(manifest.contains("host-submission-remains-available-and-succeeds"));
    assert!(!manifest.contains("ordinary-tool-attempts"));
    assert!(!manifest.contains("if attempt == 0"));
    assert!(readme.contains("feature\n`ai/temper#1041`"));
    assert!(readme.contains("historical\n`mapped-live-graph-consumption`"));
    assert!(readme.contains("exactly one underlying execution"));
    assert!(readme.contains("Privacy-safe evidence"));
    assert!(jig.contains("mapped-live-ordinary-tool-convergence-runtime"));
    for forbidden in ["tool_call", "file_path", "command", "crate::", "opaque-"] {
        assert!(!jig.contains(forbidden), "Jig retained {forbidden}");
    }
    assert!(bundle.repo.ci_source.contains("cargo test --quiet"));
}

fn scenarios_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("temper-testing lives under crates/temper-testing")
        .join("scenarios")
}
