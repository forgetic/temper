use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::*;
use temper_protocol_activity::{
    GraphExplorationClosedReasonV1, GraphExplorationClosedV1, GraphRecoveryEvidenceKindV1,
    GraphRecoveryPermittedActionV1,
};

#[test]
fn decision_gap_recovery_bundle_maps_feature_1069_without_rewriting_history() {
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
    let jig = fs::read_to_string(bundle.jig_script_path()).expect("Jig");
    for expected in [
        "feature = \"ai/temper#1069\"",
        "plan = \"ai/temper#1070\"",
        "source_branch = \"agent/pr-for-feature-1069\"",
        "recoverable-incomplete-caller-diagnostic",
        "targeted-caller-recovery-reaches-provider-once",
        "tool.failure.graph.missing_evidence",
        "graph.lineage.decision_evidence_kind",
        "safe stop_without_product",
    ] {
        assert!(manifest.contains(expected), "manifest omitted {expected}");
    }
    assert!(readme.contains("historical"));
    assert!(readme.contains("`mapped-live-graph-consumption`"));
    assert!(readme.contains("`mapped-live-graph-convergence`"));
    assert!(readme.contains("`mapped-live-ordinary-tool-convergence`"));
    assert!(readme.contains("Privacy-safe evidence"));
    assert!(jig.contains("mapped-live-decision-gap-recovery-runtime"));
    for forbidden in ["crate::", "opaque-", "provider output", "diagnostic trace"] {
        assert!(!jig.contains(forbidden), "Jig retained {forbidden}");
    }
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
fn decision_gap_recovery_bundle_retains_closed_safe_stop_contract() {
    let details = GraphExplorationClosedV1::exhausted([GraphRecoveryEvidenceKindV1::Caller])
        .expect("safe stop details");
    assert_eq!(
        details.reason,
        GraphExplorationClosedReasonV1::RecoveryExhausted
    );
    assert_eq!(
        details.missing_evidence,
        [GraphRecoveryEvidenceKindV1::Caller]
    );
    assert_eq!(
        details.permitted_action,
        GraphRecoveryPermittedActionV1::StopWithoutProduct
    );
    assert_eq!(details.remaining_allowance, 0);
    assert!(details.model_message().contains("stop_without_product"));
}

fn scenarios_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("temper-testing lives under crates/temper-testing")
        .join("scenarios")
}
