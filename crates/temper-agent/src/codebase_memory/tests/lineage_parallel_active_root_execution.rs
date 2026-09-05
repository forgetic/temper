const PARALLEL_ACTIVE_PROVIDER_SELECTOR: &str =
    "temper-v1-production.src.route.worker_slot";

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
    std::fs::write(
        workspace.path().join("demo/src/model.rs"),
        "fn affinity_topic() {}\n",
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
        .with_lineage_admission(admission.clone());
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
        let sibling_call = dispatched_call(&roots, "sibling-root");
        let sibling_output = search
            .execute(&sibling_call.id, sibling_call.arguments, None)
            .await
            .unwrap();
        let sibling_root = lineage(&sibling_output);
        assert_eq!(sibling_root.focused_test_discovery, None);
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
        let sibling_references = references(&sibling_output);
        let active_references = references(&active_output);
        assert_eq!(
            active_references.len(),
            4,
            "active provider guidance: {}",
            crate::codebase_memory::tests::test_support::output_text(&active_output),
        );
        let active_guidance =
            crate::codebase_memory::tests::test_support::output_text(&active_output);
        assert!(active_guidance.contains("provider_result_order=1"));
        for test_order in 2..=6 {
            assert!(!active_guidance.contains(&format!(
                "provider_result_order={test_order}"
            )));
        }
        assert!(active_guidance.contains("provider_result_order=7"));
        assert!(active_guidance.contains("provider_result_order=8"));
        assert!(complete_tool(&mut machine, "active-root", active_output, None).is_empty());
        let selected = complete_tool(&mut machine, "sibling-root", sibling_output, None);
        assert_ne!(active_root.root_binding, sibling_root.root_binding);
        let handoff = active_handoff(&selected);
        let presented_handoff = handoff_references(handoff);
        assert_eq!(presented_handoff, active_references);
        let active_reference = active_references[2].clone();
        let first_reference = active_references[0].clone();
        assert!(!handoff.contains("decision_evidence_kind"));
        assert!(!sibling_references.contains(&active_reference));

        let fabricated = "temper-recovery-selector:00000000-0000-4000-8000-000000000099";
        let negative_calls = [
            (
                "sibling",
                "codebase_memory_get_code_snippet",
                serde_json::json!({"qualified_name": sibling_references[0]}),
            ),
            (
                "raw",
                "codebase_memory_get_code_snippet",
                serde_json::json!({"qualified_name":"temper-v1-production.src.model.affinity_topic"}),
            ),
            (
                "ambiguous",
                "codebase_memory_get_code_snippet",
                serde_json::json!({"qualified_name": PARALLEL_ACTIVE_PROVIDER_SELECTOR}),
            ),
            (
                "wrong-tool",
                "codebase_memory_trace_path",
                serde_json::json!({"function_name": active_reference}),
            ),
            (
                "wrong-purpose",
                "codebase_memory_get_code_snippet",
                serde_json::json!({
                    "qualified_name": active_reference,
                    "decision_evidence_kind": "caller"
                }),
            ),
            (
                "fabricated",
                "codebase_memory_get_code_snippet",
                serde_json::json!({"qualified_name": fabricated}),
            ),
        ];
        let rejected = complete_llm(
            &mut machine,
            assistant(
                negative_calls
                    .iter()
                    .map(|(id, tool, arguments)| (*id, *tool, arguments.clone()))
                    .collect(),
            ),
        );
        let mut failures = Vec::new();
        for (id, tool_name, _) in &negative_calls {
            if let Some(failure) = maybe_local_failure(&rejected, id) {
                failures.push((*id, failed_output(), failure));
                continue;
            }
            let call = dispatched_call(&rejected, id);
            let output = registry
                .get(tool_name)
                .expect("negative graph wrapper")
                .execute(&call.id, call.arguments, None)
                .await
                .unwrap();
            assert!(output.is_error, "negative selector {id} escaped its wrapper");
            failures.push((
                *id,
                output,
                ToolFailureDiagnostic::codebase_memory(ToolFailureCategory::InvalidModelInput),
            ));
        }
        let mut corrected = Vec::new();
        for (id, output, failure) in failures {
            corrected.extend(complete_tool(&mut machine, id, output, Some(failure)));
        }
        assert_eq!(
            handoff_references(active_handoff(&corrected)),
            active_references
        );
        assert!(
            crate::codebase_memory::tests::test_support::calls_named(
                &log_path,
                "get_code_snippet"
            )
            .is_empty()
        );

        assert!(
            crate::codebase_memory::tests::test_support::calls_named(&log_path, "trace_path")
                .is_empty()
        );

        let malformed = complete_llm(
            &mut machine,
            assistant(vec![(
                "malformed",
                "codebase_memory_get_code_snippet",
                serde_json::json!({
                    "qualified_name": active_reference,
                    "function_name": "worker_slot"
                }),
            )]),
        );
        let malformed_failure = local_failure(&malformed, "malformed");
        let corrected = complete_tool(
            &mut machine,
            "malformed",
            failed_output(),
            Some(malformed_failure),
        );
        assert_eq!(
            handoff_references(active_handoff(&corrected)),
            active_references
        );

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
        let TargetAdmissionOutcome::Eligible(source_target) =
            admission.resolve_source_target(&source_lineage)
        else {
            panic!("the selected provider source has an exact ordinary target");
        };
        let InvocationTargetAdmission::Read(TargetAdmissionOutcome::Eligible(read_target)) =
            admission.resolve_invocation_targets(
                "read",
                &serde_json::json!({"path": "demo/src/route.rs"}),
            )
        else {
            panic!("the selected source authorizes its exact ordinary read");
        };
        assert!(source_target.matches(&read_target));
        let InvocationTargetAdmission::Mutation(mutation_targets) =
            admission.resolve_invocation_targets(
                "write",
                &serde_json::json!({"path": "demo/src/route.rs", "content": "changed"}),
            )
        else {
            panic!("the selected source authorizes its exact ordinary mutation target");
        };
        assert!(matches!(
            mutation_targets.as_slice(),
            [TargetAdmissionOutcome::Eligible(target)] if source_target.matches(target)
        ));
        assert_eq!(
            admission.resolve_invocation_targets(
                "write",
                &serde_json::json!({"path": "demo/src/model.rs", "content": "changed"}),
            ),
            InvocationTargetAdmission::Mutation(vec![TargetAdmissionOutcome::Ineligible(
                TargetAdmissionStatus::UnknownTarget,
            )]),
            "the unchosen first candidate receives no mutation target authority",
        );
        let source_calls = crate::codebase_memory::tests::test_support::calls_named(
            &log_path,
            "get_code_snippet",
        );
        assert_eq!(source_calls.len(), 1);
        assert_eq!(
            source_calls[0]["arguments"]["qualified_name"],
            PARALLEL_ACTIVE_PROVIDER_SELECTOR
        );
        assert_eq!(source_calls[0]["arguments"].as_object().unwrap().len(), 2);
        assert!(source_calls[0]["arguments"]["project"].is_string());
        assert!(source_calls[0]["arguments"].get("repo").is_none());
        assert!(
            source_calls[0]["arguments"]
                .get("include_neighbors")
                .is_none()
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

        let stale = complete_llm(
            &mut machine,
            assistant(vec![
                (
                    "unchosen-first-source",
                    "codebase_memory_get_code_snippet",
                    serde_json::json!({"qualified_name": first_reference}),
                ),
                (
                    "consumed-source",
                    "codebase_memory_get_code_snippet",
                    serde_json::json!({"qualified_name": active_reference}),
                ),
            ]),
        );
        let mut stale_completed = Vec::new();
        for id in ["unchosen-first-source", "consumed-source"] {
            let (stale_output, stale_failure) =
                if let Some(failure) = maybe_local_failure(&stale, id) {
                    (failed_output(), failure)
                } else {
                    let call = dispatched_call(&stale, id);
                    let output = registry
                        .get("codebase_memory_get_code_snippet")
                        .unwrap()
                        .execute(&call.id, call.arguments, None)
                        .await
                        .unwrap();
                    assert!(output.is_error);
                    (
                        output,
                        ToolFailureDiagnostic::codebase_memory(
                            ToolFailureCategory::InvalidModelInput,
                        ),
                    )
                };
            stale_completed.extend(complete_tool(
                &mut machine,
                id,
                stale_output,
                Some(stale_failure),
            ));
        }
        assert_eq!(
            crate::codebase_memory::tests::test_support::calls_named(
                &log_path,
                "get_code_snippet"
            )
            .len(),
            1
        );
        assert!(!stale_completed.iter().any(|request| matches!(
            request,
            AgentRequest::Finished {
                stop: AgentStop::DecisionAnchorRecoveryExhausted,
                ..
            }
        )));
    });
}
