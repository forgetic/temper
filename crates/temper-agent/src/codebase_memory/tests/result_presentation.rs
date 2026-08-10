use super::super::result_presentation::DECISION_ANCHOR;
use super::test_support::*;
use super::*;
use serde_json::json;
use temper_agent_core::{
    DecisionAnchorLineageStageV1, DecisionAnchorLineageV1, DecisionAnchorTargetKindV1,
    SAFE_DECISION_ANCHOR_LINEAGE_DETAIL_KEY,
};

fn lineage(output: &tongs::tools::ToolOutput) -> DecisionAnchorLineageV1 {
    serde_json::from_value(
        output
            .details
            .as_ref()
            .and_then(|details| details.get(SAFE_DECISION_ANCHOR_LINEAGE_DETAIL_KEY))
            .cloned()
            .expect("targeted result carries bounded anchor lineage"),
    )
    .expect("anchor lineage is typed")
}

#[test]
fn successful_targeted_results_present_only_a_bounded_provider_neutral_decision_anchor() {
    const FIXTURE_TARGET: &str = "PRIVATE-FIXTURE-TARGET-993";
    let dir = fake_server_script();
    let workspace = tempfile::tempdir().expect("workspace");
    let log_path = workspace.path().join("decision-anchor.log");
    let context = workspace_context(workspace.path(), &[("acme", "demo", "demo")]);

    temper_agent_io::block_on(async move {
        let tools = build_codebase_memory_toolset(
            Some(&config(
                &dir,
                CodebaseMemoryMode::Required,
                CodebaseMemoryIndex::Off,
                "anchor-cases",
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
        let search = tools
            .iter()
            .find(|tool| tool.name() == "codebase_memory_search_code")
            .expect("search wrapper");
        let graph = tools
            .iter()
            .find(|tool| tool.name() == "codebase_memory_search_graph")
            .expect("graph wrapper");
        let architecture = tools
            .iter()
            .find(|tool| tool.name() == "codebase_memory_get_architecture")
            .expect("architecture wrapper");

        let targeted = search
            .execute(
                "targeted",
                json!({"query": "target", "pattern": FIXTURE_TARGET}),
                None,
            )
            .await
            .expect("targeted result");
        let targeted_text = output_text(&targeted);
        assert!(!targeted.is_error);
        assert!(targeted_text.ends_with(DECISION_ANCHOR));
        assert!(targeted_text.contains("PROVIDER-RESULT-SENTINEL"));
        assert!(
            !DECISION_ANCHOR.contains(FIXTURE_TARGET),
            "the anchor must not retain a model target"
        );
        assert!(
            !DECISION_ANCHOR.contains("PROVIDER-RESULT-SENTINEL"),
            "the anchor must not retain provider output"
        );
        let lineage = lineage(&targeted);
        assert!(lineage.is_valid());
        assert_eq!(lineage.stage, DecisionAnchorLineageStageV1::Root);
        let details = serde_json::to_string(&targeted.details).expect("details serialize");
        assert!(
            !details.contains(FIXTURE_TARGET) && !details.contains("PROVIDER-RESULT-SENTINEL"),
            "details retain only privacy-safe lineage type facts"
        );
        assert!(targeted_text.len() <= MAX_CODEBASE_MEMORY_OUTPUT_BYTES);

        let unrelated = architecture
            .execute("unrelated", json!({}), None)
            .await
            .expect("unrelated discovery result");
        assert!(!unrelated.is_error);
        assert!(!output_text(&unrelated).contains(DECISION_ANCHOR));

        let ambiguous = graph
            .execute(
                "ambiguous",
                json!({"query": "one", "name_pattern": "two"}),
                None,
            )
            .await
            .expect("ambiguous targeted result");
        assert!(!ambiguous.is_error);
        assert!(!output_text(&ambiguous).contains(DECISION_ANCHOR));

        let truncated = search
            .execute(
                "truncated",
                json!({"query": "large", "pattern": FIXTURE_TARGET}),
                None,
            )
            .await
            .expect("truncated targeted result");
        assert!(!truncated.is_error);
        assert!(output_text(&truncated).contains("output truncated"));
        assert!(!output_text(&truncated).contains(DECISION_ANCHOR));
        assert!(
            truncated
                .details
                .as_ref()
                .and_then(|details| details.get(SAFE_GRAPH_CORRELATION_DETAIL_KEY))
                .is_some(),
            "successful targeted calls always retain their closed V1 correlation"
        );
        assert!(
            truncated
                .details
                .as_ref()
                .and_then(|details| details.get(SAFE_DECISION_ANCHOR_LINEAGE_DETAIL_KEY))
                .is_none(),
            "truncated results retain V1 correlation but cannot create lineage"
        );
        assert!(output_text(&truncated).len() <= MAX_CODEBASE_MEMORY_OUTPUT_BYTES);
    });
}

#[test]
fn wrapper_carries_only_typed_equivalent_provider_identities_under_one_opaque_root() {
    let dir = fake_server_script();
    let workspace = tempfile::tempdir().expect("workspace");
    let log_path = workspace.path().join("lineage.log");
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
        let graph = tools
            .iter()
            .find(|tool| tool.name() == "codebase_memory_search_graph")
            .unwrap();
        let trace = tools
            .iter()
            .find(|tool| tool.name() == "codebase_memory_trace_path")
            .unwrap();
        let source = tools
            .iter()
            .find(|tool| tool.name() == "codebase_memory_get_code_snippet")
            .unwrap();

        let root_output = graph
            .execute("root", json!({"query": "start"}), None)
            .await
            .unwrap();
        let root = lineage(&root_output);
        assert_eq!(root.stage, DecisionAnchorLineageStageV1::Root);
        assert_eq!(
            root.result_target_kinds,
            vec![
                DecisionAnchorTargetKindV1::Pattern,
                DecisionAnchorTargetKindV1::FunctionName,
                DecisionAnchorTargetKindV1::QualifiedName,
            ]
        );
        let trace_output = trace
            .execute("trace", json!({"function_name": "run"}), None)
            .await
            .unwrap();
        let trace_lineage = lineage(&trace_output);
        assert_eq!(
            trace_lineage.stage,
            DecisionAnchorLineageStageV1::CarryForward
        );
        assert_eq!(trace_lineage.root_binding, root.root_binding);
        let source_output = source
            .execute(
                "source",
                json!({"qualified_name": "crate::engine::run"}),
                None,
            )
            .await
            .unwrap();
        let source_lineage = lineage(&source_output);
        assert_eq!(
            source_lineage.stage,
            DecisionAnchorLineageStageV1::CarryForward
        );
        assert_eq!(source_lineage.root_binding, root.root_binding);

        let rendered = serde_json::to_string(&[
            root_output.details,
            trace_output.details,
            source_output.details,
        ])
        .unwrap();
        for raw in [
            "crate::engine::run",
            "/private/src/lib.rs",
            "PRIVATE-SOURCE",
        ] {
            assert!(!rendered.contains(raw), "details retained {raw:?}");
        }
    });
}
