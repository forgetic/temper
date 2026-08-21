use super::test_support::*;
use super::*;
use serde_json::json;
use temper_agent_core::SAFE_DECISION_ANCHOR_LINEAGE_DETAIL_KEY;
use temper_protocol_activity::{
    DecisionAnchorLineageStageV1, DecisionAnchorLineageV1, DecisionEvidenceKindV1,
};

fn lineage(output: &tongs::tools::ToolOutput) -> DecisionAnchorLineageV1 {
    serde_json::from_value(
        output
            .details
            .as_ref()
            .and_then(|details| details.get(SAFE_DECISION_ANCHOR_LINEAGE_DETAIL_KEY))
            .cloned()
            .expect("trusted lineage detail"),
    )
    .expect("typed lineage")
}

#[test]
fn source_evidence_schema_and_lineage_are_closed_and_provider_private() {
    const PRIVATE_ARGUMENT: &str = "focused_test_PRIVATE_SENTINEL.rs";
    let dir = fake_server_script();
    let workspace = tempfile::tempdir().expect("workspace");
    let log_path = workspace.path().join("decision-evidence.log");
    let context = workspace_context(workspace.path(), &[("acme", "demo", "demo")]);

    temper_agent_io::block_on(async move {
        let tools = build_codebase_memory_toolset(
            Some(&config(
                &dir,
                CodebaseMemoryMode::Required,
                CodebaseMemoryIndex::Off,
                "lineage-cases",
                &log_path,
                json!({}),
            )),
            "engineer",
            &context,
            workspace.path(),
        )
        .await
        .expect("build toolset")
        .into_tools();
        let source = tools
            .iter()
            .find(|tool| tool.name() == "codebase_memory_get_code_snippet")
            .expect("source wrapper");
        let source_parameters = source.parameters();
        assert_eq!(
            source_parameters["properties"]["decision_evidence_kind"]["enum"],
            json!(["implementation", "caller", "focused_test"])
        );
        for name in [
            "codebase_memory_search_graph",
            "codebase_memory_search_code",
            "codebase_memory_trace_path",
        ] {
            let parameters = tools
                .iter()
                .find(|tool| tool.name() == name)
                .unwrap()
                .parameters();
            assert!(
                parameters["properties"]
                    .get("decision_evidence_kind")
                    .is_none(),
                "only source reads accept a semantic evidence declaration"
            );
        }

        let graph = tools
            .iter()
            .find(|tool| tool.name() == "codebase_memory_search_graph")
            .expect("graph wrapper");
        let root_output = graph
            .execute(
                "root",
                json!({
                    "query": "start",
                    "path": PRIVATE_ARGUMENT,
                    "purpose": "focused_test caller implementation"
                }),
                None,
            )
            .await
            .expect("root result");
        assert_eq!(lineage(&root_output).decision_evidence_kind, None);

        let declared = source
            .execute(
                "declared",
                json!({
                    "qualified_name": "crate::engine::run",
                    "decision_evidence_kind": "caller",
                    "path": PRIVATE_ARGUMENT,
                    "purpose": "focused_test"
                }),
                None,
            )
            .await
            .expect("declared source result");
        let declared_lineage = lineage(&declared);
        assert_eq!(
            declared_lineage.stage,
            DecisionAnchorLineageStageV1::CarryForward
        );
        assert_eq!(
            declared_lineage.decision_evidence_kind,
            Some(DecisionEvidenceKindV1::Caller)
        );
        let calls = calls_named(&log_path, "get_code_snippet");
        assert_eq!(calls.len(), 1);
        assert!(
            calls[0]["arguments"]
                .get("decision_evidence_kind")
                .is_none(),
            "wrapper-owned purpose must not reach the MCP provider"
        );

        let undeclared = source
            .execute(
                "undeclared",
                json!({
                    "qualified_name": "crate::engine::run",
                    "path": PRIVATE_ARGUMENT,
                    "purpose": "implementation focused_test caller"
                }),
                None,
            )
            .await
            .expect("ordinary source result");
        assert_eq!(lineage(&undeclared).decision_evidence_kind, None);
        let private_metadata =
            serde_json::to_string(&[root_output.details, declared.details, undeclared.details])
                .unwrap();
        assert!(!private_metadata.contains(PRIVATE_ARGUMENT));
        assert!(!private_metadata.contains("implementation focused_test caller"));

        let malformed = source
            .execute(
                "malformed",
                json!({
                    "qualified_name": "crate::engine::run",
                    "decision_evidence_kind": "behavioral prose"
                }),
                None,
            )
            .await
            .expect("malformed declaration is a typed local failure");
        assert!(malformed.is_error);
        assert_eq!(calls_named(&log_path, "get_code_snippet").len(), 2);
        assert!(
            malformed
                .details
                .as_ref()
                .and_then(|details| details.get(SAFE_DECISION_ANCHOR_LINEAGE_DETAIL_KEY))
                .is_none()
        );

        let ineligible = graph
            .execute(
                "ineligible",
                json!({
                    "query": "start",
                    "decision_evidence_kind": "implementation"
                }),
                None,
            )
            .await
            .expect("non-source declaration fails locally");
        assert!(ineligible.is_error);
        assert_eq!(calls_named(&log_path, "search_graph").len(), 1);
    });
}
