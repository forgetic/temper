//! Released source-less metadata cannot poison a usable search or mint selectors.
use super::*;

#[test]
fn mixed_search_omits_decorator_and_keeps_source_lineage_and_followup_snippet() {
    let fixture = provider();
    let workspace = tempfile::tempdir().unwrap();
    let context = workspace_context(workspace.path(), &[("acme", "demo", "demo")]);
    let root = workspace.path().join("demo");
    fs::write(root.join("same.py"), "def same_symbol():\n    return 42\n").unwrap();
    let log = workspace.path().join("provider.log");
    let config = config(
        &fixture,
        CodebaseMemoryMode::Required,
        CodebaseMemoryIndex::Blocking,
        "source-less-decorator",
        &log,
        json!({}),
    );
    temper_agent_io::block_on(async move {
        let tools =
            build_codebase_memory_toolset(Some(&config), "engineer", &context, workspace.path())
                .await
                .unwrap()
                .into_tools();
        let search = tools
            .iter()
            .find(|tool| tool.name() == "codebase_memory_search_graph")
            .unwrap();
        let snippet = tools
            .iter()
            .find(|tool| tool.name() == "codebase_memory_get_code_snippet")
            .unwrap();
        let output = search
            .execute("mixed", json!({"query": "test same symbol"}), None)
            .await
            .unwrap();
        assert!(!output.is_error, "{}", output_text(&output));
        let text = output_text(&output);
        assert!(text.contains("fixture.same.same_symbol"));
        assert!(text.contains("temper-recovery-selector:"));
        assert!(!text.contains("<decorator:test>"));
        assert!(text.contains("\"omitted_non_source_nodes\":1"));
        let root_lineage = lineage(&output);
        assert_eq!(root_lineage["stage"], "root");
        let source = snippet
            .execute(
                "followup",
                json!({"qualified_name": "fixture.same.same_symbol"}),
                None,
            )
            .await
            .unwrap();
        assert!(!source.is_error, "{}", output_text(&source));
        assert!(output_text(&source).contains("return 42"));
        assert_eq!(
            lineage(&source)["root_binding"],
            root_lineage["root_binding"]
        );

        let metadata = search
            .execute("metadata-only", json!({"query": "decorator only"}), None)
            .await
            .unwrap();
        assert!(!metadata.is_error, "{}", output_text(&metadata));
        assert!(!output_text(&metadata).contains("<decorator:test>"));
        assert!(!output_text(&metadata).contains("temper-recovery-selector:"));
        if let Some(evidence) = metadata
            .details
            .as_ref()
            .unwrap()
            .get(temper_agent_core::SAFE_DECISION_ANCHOR_LINEAGE_DETAIL_KEY)
        {
            assert!(evidence.get("result_target_kinds").is_none());
            assert!(evidence.get("decision_evidence_kind").is_none());
        }
        assert_eq!(calls_named(&log, "get_code_snippet").len(), 1);
    });
}

fn lineage(output: &ToolOutput) -> &serde_json::Value {
    output
        .details
        .as_ref()
        .unwrap()
        .get(temper_agent_core::SAFE_DECISION_ANCHOR_LINEAGE_DETAIL_KEY)
        .expect("usable source lineage")
}

fn provider() -> tempfile::TempDir {
    let fixture = fake_server_script();
    let path = script_path(&fixture);
    let source = fs::read_to_string(&path).unwrap().replace(
        "        elif name == \"get_code_snippet\" and mode == \"cold-warm\":",
        r#"        elif mode == "source-less-decorator" and name == "search_graph":
            rows = [["<decorator:test>", "Decorator", "", "", -7.252591004539256]]
            if args.get("query") != "decorator only":
                rows.insert(0, ["fixture.same.same_symbol", "Function", "same.py", "1-2", -16.897694357538438])
            payload = {"cols": ["qn", "label", "file", "lines", "rank"],
                       "rows": rows, "total": len(rows), "has_more": False, "search_mode": "bm25"}
            tool_result(request["id"], json.dumps(payload), structured=payload)
        elif mode == "source-less-decorator" and name == "get_code_snippet":
            payload = {"qualified_name": "fixture.same.same_symbol", "file_path": "same.py",
                       "source": "def same_symbol():\n    return 42\n", "start_line": 1, "end_line": 2}
            tool_result(request["id"], json.dumps(payload), structured=payload)
        elif name == "get_code_snippet" and mode == "cold-warm":"#,
    );
    assert!(source.contains("source-less-decorator"));
    fs::write(path, source).unwrap();
    fixture
}
