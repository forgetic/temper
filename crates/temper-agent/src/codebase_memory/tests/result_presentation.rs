use super::super::result_presentation::DECISION_ANCHOR;
use super::test_support::*;
use super::*;
use serde_json::json;

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
        assert!(
            !serde_json::to_string(&targeted.details)
                .expect("details serialize")
                .contains(FIXTURE_TARGET),
            "details must retain only the existing opaque correlation"
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
        assert!(output_text(&truncated).len() <= MAX_CODEBASE_MEMORY_OUTPUT_BYTES);
    });
}
