fn run_parallel_overlapping_roots_execute_same_batch_candidate_selection(
    reverse_preview_completion: bool,
) {
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
        let toolset = crate::codebase_memory::tests::build_codebase_memory_toolset(
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
        assert_eq!(presented_handoff, sibling_references);
        let active_reference = sibling_references[2].clone();
        let first_reference = sibling_references[0].clone();
        let winner_reference = &active_reference;
        let winner_provider_selector = PARALLEL_ACTIVE_PROVIDER_SELECTOR;
        let winner_path = "demo/src/route.rs";
        let loser_reference = &first_reference;
        let loser_path = "demo/src/model.rs";
        assert!(handoff.contains("bounded candidate inspection call"));
        assert!(handoff.contains("explicit commit call"));
        assert!(handoff.contains("decision_evidence_kind"));
        assert!(!active_references.contains(&active_reference));

        let fabricated = "temper-recovery-selector:00000000-0000-4000-8000-000000000099";
        let negative_calls = [
            (
                "sibling",
                "codebase_memory_get_code_snippet",
                serde_json::json!({"qualified_name": active_references[0]}),
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
            sibling_references
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
            sibling_references
        );

        let previews = complete_llm(
            &mut machine,
            assistant(vec![
                (
                    "wrong-preview",
                    "codebase_memory_get_code_snippet",
                    serde_json::json!({"qualified_name": loser_reference}),
                ),
                (
                    "chosen-preview",
                    "codebase_memory_get_code_snippet",
                    serde_json::json!({"qualified_name": winner_reference}),
                ),
            ]),
        );
        for id in ["wrong-preview", "chosen-preview"] {
            assert!(previews.iter().any(|request| matches!(
                request,
                AgentRequest::Emit(AgentEvent::ToolStart {
                    id: emitted_id,
                    recovery_reference_disposition: Some(
                        GraphRecoveryReferenceDispositionV1::Recognized
                    ),
                    ..
                }) if emitted_id == id
            )));
            assert!(previews.iter().any(|request| matches!(
                request,
                AgentRequest::RunTool {
                    call,
                    denial: None,
                    rejection: None,
                    ..
                } if call.id == id
            )));
        }
        let wrong_preview_call = dispatched_call(&previews, "wrong-preview");
        let chosen_preview_call = dispatched_call(&previews, "chosen-preview");
        let source = registry
            .get("codebase_memory_get_code_snippet")
            .unwrap();
        let (chosen_preview_output, wrong_preview_output) = if reverse_preview_completion {
            let chosen = source
                .execute(
                    &chosen_preview_call.id,
                    chosen_preview_call.arguments,
                    None,
                )
                .await
                .unwrap();
            let wrong = source
                .execute(&wrong_preview_call.id, wrong_preview_call.arguments, None)
                .await
                .unwrap();
            (chosen, wrong)
        } else {
            let wrong = source
                .execute(&wrong_preview_call.id, wrong_preview_call.arguments, None)
                .await
                .unwrap();
            assert_eq!(
                admission.recovery_reference_disposition(
                    "codebase_memory_get_code_snippet",
                    &serde_json::json!({"qualified_name": winner_reference}),
                    Some(&sibling_root.root_binding),
                ),
                Some(GraphRecoveryReferenceDispositionV1::Recognized),
                "the wrong preview cannot poison the later chosen candidate",
            );
            let chosen = source
                .execute(
                    &chosen_preview_call.id,
                    chosen_preview_call.arguments,
                    None,
                )
                .await
                .unwrap();
            (chosen, wrong)
        };
        assert!(!wrong_preview_output.is_error);
        assert!(!chosen_preview_output.is_error);
        assert!(
            crate::codebase_memory::tests::test_support::output_text(&wrong_preview_output)
                .contains(PARALLEL_FIRST_PROVIDER_SELECTOR)
        );
        assert!(
            crate::codebase_memory::tests::test_support::output_text(&chosen_preview_output)
                .contains(PARALLEL_ACTIVE_PROVIDER_SELECTOR)
        );
        let (repeated_preview, repeated_disposition) =
            admission.resolve_for_active_root_with_recovery(
                "codebase_memory_get_code_snippet",
                &serde_json::json!({"qualified_name": loser_reference}),
                Some(&sibling_root.root_binding),
            );
        assert!(matches!(
            repeated_preview,
            LineageAdmissionOutcome::Ineligible(_)
        ));
        assert_eq!(
            repeated_disposition,
            Some(GraphRecoveryReferenceDispositionV1::Rejected),
            "each visible candidate has at most one bounded preview",
        );
        assert_eq!(
            admission.recovery_reference_disposition(
                "codebase_memory_get_code_snippet",
                &serde_json::json!({
                    "qualified_name": winner_reference,
                    "decision_evidence_kind": "implementation",
                }),
                Some(&sibling_root.root_binding),
            ),
            Some(GraphRecoveryReferenceDispositionV1::Recognized),
            "rejecting a repeated preview cannot poison the explicit commit",
        );
        for (preview, path) in [
            (&wrong_preview_output, loser_path),
            (&chosen_preview_output, winner_path),
        ] {
            let preview_lineage = lineage(preview);
            assert_eq!(preview_lineage.root_binding, sibling_root.root_binding);
            assert_eq!(preview_lineage.decision_evidence_kind, None);
            assert_eq!(
                admission.resolve_source_target(&preview_lineage),
                TargetAdmissionOutcome::Ineligible(TargetAdmissionStatus::UnknownTarget),
                "a preview cannot authorize its source target",
            );
            assert_eq!(
                admission.resolve_invocation_targets(
                    "write",
                    &serde_json::json!({"path": path, "content": "changed"}),
                ),
                InvocationTargetAdmission::Mutation(vec![TargetAdmissionOutcome::Ineligible(
                    TargetAdmissionStatus::UnknownTarget,
                )]),
                "a preview cannot authorize mutation",
            );
        }
        let preview_details = serde_json::to_string(&[
            wrong_preview_output.details.as_ref(),
            chosen_preview_output.details.as_ref(),
        ])
        .unwrap();
        for private in [
            PARALLEL_FIRST_PROVIDER_SELECTOR,
            PARALLEL_ACTIVE_PROVIDER_SELECTOR,
            "src/model.rs",
            "src/route.rs",
            "fn temper-v1-production",
        ] {
            assert!(!preview_details.contains(private));
        }
        let after_previews = if reverse_preview_completion {
            assert!(complete_tool(
                &mut machine,
                "chosen-preview",
                chosen_preview_output,
                None,
            )
            .is_empty());
            complete_tool(
                &mut machine,
                "wrong-preview",
                wrong_preview_output,
                None,
            )
        } else {
            assert!(complete_tool(
                &mut machine,
                "wrong-preview",
                wrong_preview_output,
                None,
            )
            .is_empty());
            complete_tool(
                &mut machine,
                "chosen-preview",
                chosen_preview_output,
                None,
            )
        };
        let handoff_after_previews = active_handoff(&after_previews);
        assert_eq!(
            handoff_references(handoff_after_previews),
            sibling_references,
        );
        assert!(handoff_after_previews.contains("explicit commit call"));
        assert!(handoff_after_previews.contains("decision_evidence_kind"));
        assert_eq!(
            crate::codebase_memory::tests::test_support::calls_named(
                &log_path,
                "get_code_snippet",
            )
            .len(),
            2,
        );
        let preview_calls = crate::codebase_memory::tests::test_support::calls_named(
            &log_path,
            "get_code_snippet",
        );
        assert!(preview_calls.iter().any(|call| {
            call["arguments"]["qualified_name"] == PARALLEL_FIRST_PROVIDER_SELECTOR
        }));
        assert!(preview_calls.iter().any(|call| {
            call["arguments"]["qualified_name"] == PARALLEL_ACTIVE_PROVIDER_SELECTOR
        }));

        let committed = complete_llm(
            &mut machine,
            assistant(vec![(
                "committed-source",
                "codebase_memory_get_code_snippet",
                serde_json::json!({
                    "qualified_name": winner_reference,
                    "decision_evidence_kind": "implementation",
                }),
            )]),
        );
        assert!(committed.iter().any(|request| matches!(
            request,
            AgentRequest::Emit(AgentEvent::ToolStart {
                id,
                recovery_reference_disposition: Some(
                    GraphRecoveryReferenceDispositionV1::Recognized
                ),
                ..
            }) if id == "committed-source"
        )));
        let source_call = committed
            .iter()
            .find_map(|request| match request {
                AgentRequest::RunTool {
                    call,
                    denial: None,
                    rejection: None,
                    ..
                } if call.id == "committed-source" => Some(call.clone()),
                _ => None,
            })
            .expect("the explicit worker_slot commit reaches the provider");
        let source_output = source
            .execute(&source_call.id, source_call.arguments, None)
            .await
            .unwrap();
        assert!(!source_output.is_error);
        let source_lineage = lineage(&source_output);
        assert_eq!(source_lineage.root_binding, sibling_root.root_binding);
        assert_eq!(
            source_lineage.decision_evidence_kind,
            Some(DecisionEvidenceKindV1::Implementation)
        );
        let TargetAdmissionOutcome::Eligible(source_target) =
            admission.resolve_source_target(&source_lineage)
        else {
            panic!("the explicitly committed provider source has an exact ordinary target");
        };
        let InvocationTargetAdmission::Read(TargetAdmissionOutcome::Eligible(read_target)) =
            admission.resolve_invocation_targets(
                "read",
                &serde_json::json!({"path": winner_path}),
            )
        else {
            panic!("the committed source authorizes its exact ordinary read");
        };
        assert!(source_target.matches(&read_target));
        let InvocationTargetAdmission::Mutation(mutation_targets) =
            admission.resolve_invocation_targets(
                "write",
                &serde_json::json!({"path": winner_path, "content": "changed"}),
            )
        else {
            panic!("the committed source authorizes its exact ordinary mutation target");
        };
        assert!(matches!(
            mutation_targets.as_slice(),
            [TargetAdmissionOutcome::Eligible(target)] if source_target.matches(target)
        ));
        assert_eq!(
            admission.resolve_invocation_targets(
                "write",
                &serde_json::json!({"path": loser_path, "content": "changed"}),
            ),
            InvocationTargetAdmission::Mutation(vec![TargetAdmissionOutcome::Ineligible(
                TargetAdmissionStatus::UnknownTarget,
            )]),
            "the wrong preview receives no mutation target authority",
        );
        let source_calls = crate::codebase_memory::tests::test_support::calls_named(
            &log_path,
            "get_code_snippet",
        );
        assert_eq!(source_calls.len(), 3);
        assert_eq!(
            source_calls.last().unwrap()["arguments"]["qualified_name"],
            winner_provider_selector
        );
        assert!(source_calls.iter().all(|call| {
            call["arguments"].get("decision_evidence_kind").is_none()
        }));

        let advanced = complete_tool(&mut machine, "committed-source", source_output, None);
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
                    "losing-source-again",
                    "codebase_memory_get_code_snippet",
                    serde_json::json!({"qualified_name": loser_reference}),
                ),
                (
                    "winning-source-again",
                    "codebase_memory_get_code_snippet",
                    serde_json::json!({"qualified_name": winner_reference}),
                ),
            ]),
        );
        let mut stale_completed = Vec::new();
        for id in ["losing-source-again", "winning-source-again"] {
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
            3
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
