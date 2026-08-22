use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::*;

#[test]
fn denied_shell_classification_bundle_maps_feature_1082_without_rewriting_history() {
    let scenario_path = scenarios_root().join("mapped-live-denied-shell-classification");
    let bundle = ScenarioBundle::load(&scenario_path).expect("denied-shell classification bundle");
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
        } if fixture == "mapped-live-denied-shell-classification" && safe_tools == &vec![
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
    for expected in [
        "feature = \"ai/temper#1082\"",
        "plan = \"ai/temper#1083\"",
        "source_branch = \"agent/pr-for-feature-1082\"",
        "excluded_never_executed_local_policy_denial",
        "tool.shell_discovery_disposition.matching_discovery_segments",
        "policy_denial",
        "policy_precondition",
        "five-complete-v1-correlations-and-lineages",
    ] {
        assert!(manifest.contains(expected), "manifest omitted {expected}");
    }
    assert!(readme.contains("historical mappings"));
    assert!(readme.contains("DecisionAnchorMutation"));
    assert!(readme.contains("Privacy-safe evidence"));
    assert!(jig.contains("mapped-live-denied-shell-classification-runtime"));
    for forbidden in [
        "command",
        "arguments",
        "crate::",
        "provider output",
        "diagnostic trace",
        "denied-shell-process-canary",
    ] {
        assert!(!jig.contains(forbidden), "Jig retained {forbidden}");
    }
    assert!(bundle.repo.ci_source.contains("cargo test --quiet"));
    let gitignore =
        fs::read_to_string(bundle.repo.seed_path.join(".gitignore")).expect("fixture gitignore");
    for generated in ["/target/", "/Cargo.lock"] {
        assert!(
            gitignore.lines().any(|line| line == generated),
            "fixture validation must keep {generated} out of the repair diff"
        );
    }
}

fn scenarios_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("temper-testing lives under crates/temper-testing")
        .join("scenarios")
}
