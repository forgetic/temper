//! A provider's stable root metadata is not a source snapshot fence.
use super::*;

#[test]
fn wrapped_snippets_reject_foreign_bytes_under_unchanged_root_metadata_and_recover() {
    let fixture = fake_server_script();
    let path = script_path(&fixture);
    let source = fs::read_to_string(&path).unwrap().replace(
        "        elif name == \"get_code_snippet\" and mode == \"cold-warm\":",
        r#"        elif name == "get_code_snippet" and mode == "source-aba":
            with open("response.json", encoding="utf-8") as handle:
                payload = json.load(handle)
            tool_result(request["id"], json.dumps(payload), structured=payload)
        elif name == "get_code_snippet" and mode == "cold-warm":"#,
    );
    assert!(source.contains("source-aba"));
    fs::write(path, source).unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let context = workspace_context(workspace.path(), &[("acme", "demo", "demo")]);
    let root = workspace.path().join("demo");
    let local = "def same_symbol():\n    return 'CHECKOUT_A_1280'\n";
    let foreign = "def same_symbol():\n    return 'CHECKOUT_B_1280'\n";
    fs::write(root.join("same.py"), local).unwrap();
    let log = workspace.path().join("aba.log");
    let config = config(
        &fixture,
        CodebaseMemoryMode::Required,
        CodebaseMemoryIndex::Blocking,
        "source-aba",
        &log,
        json!({}),
    );
    temper_agent_io::block_on(async move {
        let toolset =
            build_codebase_memory_toolset(Some(&config), "engineer", &context, workspace.path())
                .await
                .unwrap();
        let tools = toolset.into_tools();
        let snippet = tools
            .iter()
            .find(|tool| tool.name() == "codebase_memory_get_code_snippet")
            .unwrap();
        for (sequence, bytes) in [("local", local), ("foreign", foreign), ("restored", local)] {
            fs::write(
                root.join("response.json"),
                json!({
                    "qualified_name":"fixture.same.same_symbol", "file_path":root.join("same.py"),
                    "start_line":1,"end_line":2,"source":bytes,
                })
                .to_string(),
            )
            .unwrap();
            let output = snippet
                .execute(
                    sequence,
                    json!({"qualified_name":"fixture.same.same_symbol"}),
                    None,
                )
                .await
                .unwrap();
            assert_eq!(
                output.is_error,
                sequence != "local",
                "{sequence}: {}",
                output_text(&output)
            );
            let rendered = format!(
                "{} {}",
                output_text(&output),
                serde_json::to_string(&output.details).unwrap()
            );
            assert!(!rendered.contains("CHECKOUT_B_1280"));
            if sequence == "local" {
                assert!(rendered.contains("CHECKOUT_A_1280"));
            } else {
                assert!(!rendered.contains("[Decision anchor:"));
                assert!(
                    output
                        .details
                        .as_ref()
                        .unwrap()
                        .get(temper_agent_core::SAFE_DECISION_ANCHOR_LINEAGE_DETAIL_KEY)
                        .is_none()
                );
            }
        }
        // ProjectNotReady closes the old circuit. Restoring source cannot
        // resurrect it; a fresh admitted toolset is the recovery boundary.
        drop(tools);
        let recovered =
            build_codebase_memory_toolset(Some(&config), "engineer", &context, workspace.path())
                .await
                .unwrap()
                .into_tools();
        let snippet = recovered
            .iter()
            .find(|tool| tool.name() == "codebase_memory_get_code_snippet")
            .unwrap();
        let output = snippet
            .execute(
                "new-toolset",
                json!({"qualified_name":"fixture.same.same_symbol"}),
                None,
            )
            .await
            .unwrap();
        assert!(!output.is_error, "{}", output_text(&output));
        assert!(output_text(&output).contains("CHECKOUT_A_1280"));
        // The provider's project binding stayed A throughout, including the
        // foreign result: before/after root checks alone would accept it.
        let state = provider_state(&log);
        let projects = state["projects"].as_object().unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(
            projects.values().next().unwrap()["repo_path"],
            root.display().to_string()
        );
    });
}
