#[test]
fn parallel_overlapping_roots_execute_the_next_turn_opaque_source_handoff() {
    let server = crate::codebase_memory::tests::test_support::fake_server_script();
    let workspace = tempfile::tempdir().unwrap();
    let log_path = workspace.path().join("mcp.log");
    let context = crate::codebase_memory::tests::test_support::workspace_context(
        workspace.path(),
        &[("acme", "demo", "demo")],
    );
    std::fs::create_dir_all(workspace.path().join("demo/src")).unwrap();
    std::fs::write(
        workspace.path().join("demo/src/route.rs"),
        "fn worker_slot() {}\n",
    )
    .unwrap();

    temper_agent_io::block_on(async move {
        let toolset = crate::codebase_memory::build_codebase_memory_toolset(
            Some(&crate::codebase_memory::tests::test_support::config(
                &server,
                CodebaseMemoryMode::Required,
                CodebaseMemoryIndex::Off,
                "active-root-overlap-handoff",
                &log_path,
                serde_json::json!({}),
            )),
            "engineer",
            &context,
            workspace.path(),
        )
        .await
        .unwrap();
        let admission = toolset.lineage_admission().unwrap();
        let registry = ToolRegistry::from_tools(toolset.into_tools());
        let catalog = Arc::new(ToolInvocationCatalog::from_registry(&registry).unwrap());
        let mut machine = AgentMachine::with_invocation_catalog(
            vec![Message::User(UserMessage {
                content: UserContent::Text("repair routing".to_string()),
                timestamp: 0,
            })],
            6,
            catalog,
        )
        .with_lineage_admission(admission);
        let _ = machine.on_start(EngineTime::ZERO);

        let roots = complete_llm(
            &mut machine,
            assistant(vec![
                (
                    "active-root",
                    "codebase_memory_search_graph",
                    serde_json::json!({"query":"active routing implementation"}),
                ),
                (
                    "sibling-root",
                    "codebase_memory_search_graph",
                    serde_json::json!({"query":"sibling routing implementation"}),
                ),
            ]),
        );
        let search = registry.get("codebase_memory_search_graph").unwrap();
        let active_call = dispatched_call(&roots, "active-root");
        let active_output = search
            .execute(&active_call.id, active_call.arguments, None)
            .await
            .unwrap();
        let active_root = lineage(&active_output);
        assert_eq!(
            active_root.focused_test_discovery,
            Some(FocusedTestDiscoveryOutcomeV1::EligibleSelectorReturned)
        );
        let sibling_call = dispatched_call(&roots, "sibling-root");
        let sibling_output = search
            .execute(&sibling_call.id, sibling_call.arguments, None)
            .await
            .unwrap();
        let sibling_root = lineage(&sibling_output);
        assert_eq!(
            sibling_root.focused_test_discovery,
            Some(FocusedTestDiscoveryOutcomeV1::EligibleSelectorReturned)
        );
        let sibling_references = references(&sibling_output);
        assert!(complete_tool(&mut machine, "active-root", active_output, None).is_empty());
        let selected = complete_tool(&mut machine, "sibling-root", sibling_output, None);
        assert_ne!(active_root.root_binding, sibling_root.root_binding);
        let handoff = active_handoff(&selected);
        let active_reference = handoff_reference(handoff).to_string();
        assert!(!handoff.contains("decision_evidence_kind"));
        assert!(!sibling_references.contains(&active_reference));

        let admitted = complete_llm(
            &mut machine,
            assistant(vec![(
                "active-source",
                "codebase_memory_get_code_snippet",
                serde_json::json!({
                    "qualified_name": active_reference
                }),
            )]),
        );
        assert!(admitted.iter().any(|request| matches!(
            request,
            AgentRequest::Emit(AgentEvent::ToolStart {
                id,
                recovery_reference_disposition: Some(
                    GraphRecoveryReferenceDispositionV1::Recognized
                ),
                ..
            }) if id == "active-source"
        )));
        let source_call = admitted
            .iter()
            .find_map(|request| match request {
                AgentRequest::RunTool {
                    call,
                    denial: None,
                    rejection: None,
                    ..
                } if call.id == "active-source" => Some(call.clone()),
                _ => None,
            })
            .expect("the active opaque handoff reaches the wrapper");
        let source = registry
            .get("codebase_memory_get_code_snippet")
            .unwrap();
        let source_output = source
            .execute(&source_call.id, source_call.arguments, None)
            .await
            .unwrap();
        assert!(!source_output.is_error);
        let source_lineage = lineage(&source_output);
        assert_eq!(source_lineage.root_binding, active_root.root_binding);
        assert_eq!(
            source_lineage.decision_evidence_kind,
            Some(DecisionEvidenceKindV1::Implementation)
        );
        let source_calls = crate::codebase_memory::tests::test_support::calls_named(
            &log_path,
            "get_code_snippet",
        );
        assert_eq!(source_calls.len(), 1);
        assert_eq!(
            source_calls[0]["arguments"]["qualified_name"],
            ACTIVE_PROVIDER_SELECTOR
        );
        assert!(
            source_calls[0]["arguments"]
                .get("decision_evidence_kind")
                .is_none()
        );

        let advanced = complete_tool(&mut machine, "active-source", source_output, None);
        assert!(advanced.iter().any(|request| match request {
            AgentRequest::CallLlm { messages, .. } => messages.iter().any(|message| matches!(
                message,
                Message::User(message) if matches!(&message.content, UserContent::Text(text)
                    if text.contains("accepted evidence=[implementation]"))
            )),
            _ => false,
        }));
        assert!(!advanced.iter().any(|request| matches!(
            request,
            AgentRequest::Finished {
                stop: AgentStop::DecisionAnchorRecoveryExhausted,
                ..
            }
        )));
    });
}
