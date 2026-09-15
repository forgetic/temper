fn run_implementation_correction_inspection(
    reverse_preview_completion: bool,
    retain_worker_slot: bool,
) {
    let server = crate::codebase_memory::tests::test_support::fake_server_script();
    let workspace = tempfile::tempdir().unwrap();
    let log_path = workspace.path().join("mcp.log");
    let context = crate::codebase_memory::tests::test_support::workspace_context(
        workspace.path(),
        &[("acme", "demo", "demo")],
    );
    std::fs::create_dir_all(workspace.path().join("demo/src")).unwrap();
    std::fs::create_dir_all(workspace.path().join("demo/tests")).unwrap();
    std::fs::write(
        workspace.path().join("demo/src/route.rs"),
        "fn worker_for() {}\nfn worker_slot() {}\n",
    )
    .unwrap();
    std::fs::write(
        workspace.path().join("demo/src/model.rs"),
        "fn affinity_topic() {}\n",
    )
    .unwrap();
    std::fs::write(
        workspace.path().join("demo/tests/route.rs"),
        "fn test_affinity_2() {}\n",
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
        let mut tools = toolset.into_tools();
        tools.push(Box::new(OrdinaryReadTool));
        tools.push(Box::new(BlockedMutationTool));
        let registry = ToolRegistry::from_tools(tools);
        let catalog = Arc::new(ToolInvocationCatalog::from_registry(&registry).unwrap());
        let mut machine = AgentMachine::with_invocation_catalog(
            vec![Message::User(UserMessage {
                content: UserContent::Text("repair routing".to_string()),
                timestamp: 0,
            })],
            20,
            catalog,
        )
        .with_lineage_admission(admission.clone());
        let _ = machine.on_start(EngineTime::ZERO);

        let root_requests = complete_llm(
            &mut machine,
            assistant(vec![(
                "root",
                "codebase_memory_search_graph",
                serde_json::json!({"query":"active routing implementation"}),
            )]),
        );
        let root_call = dispatched_call(&root_requests, "root");
        let root_output = registry
            .get("codebase_memory_search_graph")
            .unwrap()
            .execute(&root_call.id, root_call.arguments, None)
            .await
            .unwrap();
        let root_lineage = lineage(&root_output);
        let initial_references = references(&root_output);
        assert_eq!(initial_references.len(), 4);
        let initial_handoff = complete_tool(&mut machine, "root", root_output, None);
        assert_eq!(
            handoff_references(active_handoff(&initial_handoff)),
            initial_references
        );

        let provisional_reference = initial_references[if retain_worker_slot { 2 } else { 0 }].clone();
        let preview_requests = complete_llm(
            &mut machine,
            assistant(vec![(
                "preview-provisional",
                "codebase_memory_get_code_snippet",
                serde_json::json!({"qualified_name": provisional_reference}),
            )]),
        );
        let preview_call = dispatched_call(&preview_requests, "preview-provisional");
        let source = registry
            .get("codebase_memory_get_code_snippet")
            .unwrap();
        let preview_output = source
            .execute(&preview_call.id, preview_call.arguments, None)
            .await
            .unwrap();
        let provisional_text =
            crate::codebase_memory::tests::test_support::output_text(&preview_output);
        assert!(provisional_text.contains(if retain_worker_slot {
            "worker_slot"
        } else {
            "affinity_topic"
        }));
        let preview_done = complete_tool(&mut machine, "preview-provisional", preview_output, None);
        assert!(active_handoff(&preview_done).contains("explicit commit call"));

        let provisional_requests = complete_llm(
            &mut machine,
            assistant(vec![(
                "provisional-source",
                "codebase_memory_get_code_snippet",
                serde_json::json!({
                    "qualified_name": provisional_reference,
                    "decision_evidence_kind": "implementation",
                }),
            )]),
        );
        let provisional_call = dispatched_call(&provisional_requests, "provisional-source");
        let provisional_output = source
            .execute(
                &provisional_call.id,
                provisional_call.arguments,
                None,
            )
            .await
            .unwrap();
        assert_eq!(
            lineage(&provisional_output).decision_evidence_kind,
            Some(DecisionEvidenceKindV1::Implementation)
        );
        let trace_handoff = complete_tool(
            &mut machine,
            "provisional-source",
            provisional_output,
            None,
        );
        let trace_handoff_text = active_handoff(&trace_handoff);
        let trace_reference = opaque_references(trace_handoff_text)
            .into_iter()
            .next()
            .expect("provisional implementation trace reference");

        let trace_requests = complete_llm(
            &mut machine,
            assistant(vec![(
                "trace-provisional",
                "codebase_memory_trace_path",
                serde_json::json!({
                    "function_name": trace_reference,
                    "mode": "calls",
                    "direction": "inbound",
                    "include_tests": false,
                }),
            )]),
        );
        let trace_call = dispatched_call(&trace_requests, "trace-provisional");
        let trace_output = registry
            .get("codebase_memory_trace_path")
            .unwrap()
            .execute(&trace_call.id, trace_call.arguments, None)
            .await
            .unwrap();
        let trace_lineage = lineage(&trace_output);
        assert_eq!(trace_lineage.root_binding, root_lineage.root_binding);
        assert!(trace_lineage.implementation_correction_available);
        let caller_handoff = complete_tool(&mut machine, "trace-provisional", trace_output, None);
        let caller_reference = opaque_references(active_handoff(&caller_handoff))
            .into_iter()
            .next()
            .expect("typed caller reference");

        let caller_requests = complete_llm(
            &mut machine,
            assistant(vec![(
                "caller-source",
                "codebase_memory_get_code_snippet",
                serde_json::json!({
                    "qualified_name": caller_reference,
                    "decision_evidence_kind": "caller",
                }),
            )]),
        );
        let caller_call = dispatched_call(&caller_requests, "caller-source");
        let caller_output = source
            .execute(&caller_call.id, caller_call.arguments, None)
            .await
            .unwrap();
        let focused_handoff = complete_tool(&mut machine, "caller-source", caller_output, None);
        let focused_reference = opaque_references(active_handoff(&focused_handoff))
            .into_iter()
            .next()
            .expect("focused-test source reference");

        let focused_requests = complete_llm(
            &mut machine,
            assistant(vec![(
                "focused-source",
                "codebase_memory_get_code_snippet",
                serde_json::json!({
                    "qualified_name": focused_reference,
                    "decision_evidence_kind": "focused_test",
                }),
            )]),
        );
        let focused_call = dispatched_call(&focused_requests, "focused-source");
        let focused_output = source
            .execute(&focused_call.id, focused_call.arguments, None)
            .await
            .unwrap();
        let correction_handoff = complete_tool(&mut machine, "focused-source", focused_output, None);
        assert!(correction_handoff.iter().all(|request| match request {
            AgentRequest::CallLlm { messages, .. } => messages.iter().all(|message| {
                !matches!(message, Message::User(message)
                    if matches!(&message.content, UserContent::Text(text)
                        if text.starts_with("graph exploration complete: stop codebase-memory")))
            }),
            _ => true,
        }));
        let correction_text = active_handoff(&correction_handoff);
        assert!(correction_text.contains("implementation correction inspection"));
        assert!(correction_text.contains("atomically replaces provisional"));
        let correction_references = handoff_references(correction_text);
        assert_eq!(correction_references.len(), 2);
        for private in ["affinity_topic", "worker_slot", "worker_for", "src/route.rs"] {
            assert!(!correction_text.contains(private));
        }

        let provisional_path = if retain_worker_slot {
            "demo/src/route.rs"
        } else {
            "demo/src/model.rs"
        };
        let premature_read = complete_llm(
            &mut machine,
            assistant(vec![(
                "premature-provisional-read",
                "read",
                serde_json::json!({"path":provisional_path}),
            )]),
        );
        assert!(premature_read.iter().any(|request| matches!(
            request,
            AgentRequest::RunTool {
                call,
                denial: Some(ToolCallDenial::DecisionAnchorCorrectionInspection),
                ..
            } if call.id == "premature-provisional-read"
        )));
        let premature_failure = local_failure(&premature_read, "premature-provisional-read");
        let _ = complete_tool(
            &mut machine,
            "premature-provisional-read",
            failed_output(),
            Some(premature_failure),
        );

        fail_correction_preview(&mut machine, &correction_references);

        let correction_previews = complete_llm(
            &mut machine,
            assistant(
                correction_references
                    .iter()
                    .enumerate()
                    .map(|(index, reference)| {
                        (
                            if index == 0 {
                                "correction-preview-one"
                            } else {
                                "correction-preview-two"
                            },
                            "codebase_memory_get_code_snippet",
                            serde_json::json!({"qualified_name": reference}),
                        )
                    })
                    .collect(),
            ),
        );
        for id in ["correction-preview-one", "correction-preview-two"] {
            assert!(correction_previews.iter().any(|request| matches!(
                request,
                AgentRequest::RunTool { call, denial: None, rejection: None, .. }
                    if call.id == id
            )));
        }
        let mut preview_results = Vec::new();
        for id in ["correction-preview-one", "correction-preview-two"] {
            let call = dispatched_call(&correction_previews, id);
            let output = source
                .execute(&call.id, call.arguments, None)
                .await
                .unwrap();
            preview_results.push((id, output));
        }
        let selected_index = if retain_worker_slot {
            0
        } else {
            preview_results
                .iter()
                .position(|(_, output)| {
                    crate::codebase_memory::tests::test_support::output_text(output)
                        .contains("worker_slot")
                })
                .expect("one typed correction preview is worker_slot")
        };
        let selected_reference = correction_references[selected_index].clone();
        if reverse_preview_completion {
            preview_results.reverse();
        }
        let (first_id, first_output) = preview_results.remove(0);
        assert!(complete_tool(&mut machine, first_id, first_output, None).is_empty());
        let (second_id, second_output) = preview_results.remove(0);
        let after_previews = complete_tool(&mut machine, second_id, second_output, None);
        assert_eq!(
            handoff_references(active_handoff(&after_previews)),
            correction_references
        );

        if retain_worker_slot {
            let retained_read = complete_llm(
                &mut machine,
                assistant(vec![(
                    "retained-target-read",
                    "read",
                    serde_json::json!({"path":"demo/src/route.rs"}),
                )]),
            );
            assert!(retained_read.iter().any(|request| matches!(
                request,
                AgentRequest::RunTool { call, denial: None, rejection: None, .. }
                    if call.id == "retained-target-read"
            )));
            let _ = complete_tool(
                &mut machine,
                "retained-target-read",
                successful_output(),
                None,
            );

            let stale_correction = complete_llm(
                &mut machine,
                assistant(vec![(
                    "correction-after-retain",
                    "codebase_memory_get_code_snippet",
                    serde_json::json!({
                        "qualified_name": correction_references[0],
                        "decision_evidence_kind": "implementation",
                    }),
                )]),
            );
            assert!(stale_correction.iter().any(|request| matches!(
                request,
                AgentRequest::Emit(AgentEvent::ToolStart {
                    recovery_reference_disposition: Some(
                        GraphRecoveryReferenceDispositionV1::Rejected
                    ),
                    ..
                })
            )));
            let stale_failure = local_failure(&stale_correction, "correction-after-retain");
            let _ = complete_tool(
                &mut machine,
                "correction-after-retain",
                failed_output(),
                Some(stale_failure),
            );

            let unchosen_mutation = complete_llm(
                &mut machine,
                assistant(vec![(
                    "unchosen-target-mutation",
                    "write",
                    serde_json::json!({"path":"demo/src/model.rs","content":"changed"}),
                )]),
            );
            let unchosen_disposition = unchosen_mutation.iter().find_map(|request| match request {
                AgentRequest::RunTool {
                    call,
                    denial,
                    rejection,
                    ..
                } if call.id == "unchosen-target-mutation" => {
                    Some((denial.clone(), rejection.clone()))
                }
                _ => None,
            });
            assert_eq!(
                unchosen_disposition,
                Some((Some(ToolCallDenial::DecisionAnchorMutation), None)),
            );
            let _ = complete_tool(
                &mut machine,
                "unchosen-target-mutation",
                failed_output(),
                Some(ToolFailureDiagnostic::policy_denial()),
            );

            // The rejected implementation gains no mutation authority from its preview.
            // Only this later independent read can admit it as an exact companion.
            let unchosen_read = complete_llm(
                &mut machine,
                assistant(vec![(
                    "unchosen-target-read",
                    "read",
                    serde_json::json!({"path":"demo/src/model.rs"}),
                )]),
            );
            let _ = complete_tool(
                &mut machine,
                "unchosen-target-read",
                successful_output(),
                None,
            );
            let retired_selector = complete_llm(
                &mut machine,
                assistant(vec![(
                    "correction-after-companion-read",
                    "codebase_memory_get_code_snippet",
                    serde_json::json!({
                        "qualified_name": correction_references[0],
                        "decision_evidence_kind": "implementation",
                    }),
                )]),
            );
            assert!(retired_selector.iter().any(|request| matches!(
                request,
                AgentRequest::Emit(AgentEvent::ToolStart {
                    recovery_reference_disposition: Some(
                        GraphRecoveryReferenceDispositionV1::Rejected
                    ),
                    ..
                })
            )));
            let retired_failure =
                local_failure(&retired_selector, "correction-after-companion-read");
            let _ = complete_tool(
                &mut machine,
                "correction-after-companion-read",
                failed_output(),
                Some(retired_failure),
            );

            let companion_mutation = complete_llm(
                &mut machine,
                assistant(vec![(
                    "companion-target-mutation",
                    "write",
                    serde_json::json!({"path":"demo/src/model.rs","content":"changed"}),
                )]),
            );
            assert!(companion_mutation.iter().any(|request| matches!(
                request,
                AgentRequest::RunTool { call, denial: None, rejection: None, .. }
                    if call.id == "companion-target-mutation"
            )));
            let _ = complete_tool(
                &mut machine,
                "companion-target-mutation",
                successful_output(),
                None,
            );

            let retained_mutation = complete_llm(
                &mut machine,
                assistant(vec![(
                    "retained-target-mutation",
                    "write",
                    serde_json::json!({"path":"demo/src/route.rs","content":"changed"}),
                )]),
            );
            assert!(retained_mutation.iter().any(|request| matches!(
                request,
                AgentRequest::RunTool { call, denial: None, rejection: None, .. }
                    if call.id == "retained-target-mutation"
            )));
            assert!(unchosen_read.iter().any(|request| matches!(
                request,
                AgentRequest::RunTool { call, denial: None, rejection: None, .. }
                    if call.id == "unchosen-target-read"
            )));
            return;
        }

        let correction_requests = complete_llm(
            &mut machine,
            assistant(vec![(
                "corrected-source",
                "codebase_memory_get_code_snippet",
                serde_json::json!({
                    "qualified_name": selected_reference,
                    "decision_evidence_kind": "implementation",
                }),
            )]),
        );
        let correction_call = dispatched_call(&correction_requests, "corrected-source");
        let correction_output = source
            .execute(&correction_call.id, correction_call.arguments, None)
            .await
            .unwrap();
        let correction_lineage = lineage(&correction_output);
        assert!(correction_lineage.implementation_authority_corrected);
        assert_eq!(correction_lineage.root_binding, root_lineage.root_binding);
        let TargetAdmissionOutcome::Eligible(corrected_source_target) =
            admission.resolve_source_target(&correction_lineage)
        else {
            panic!("corrected source target must replace the provisional target");
        };
        let InvocationTargetAdmission::Read(TargetAdmissionOutcome::Eligible(route_read_target)) =
            admission.resolve_invocation_targets(
                "read",
                &serde_json::json!({"path":"demo/src/route.rs"}),
            )
        else {
            panic!("corrected route read target");
        };
        assert!(corrected_source_target.matches(&route_read_target));
        let after_correction = complete_tool(
            &mut machine,
            "corrected-source",
            correction_output,
            None,
        );
        assert!(after_correction.iter().any(|request| match request {
            AgentRequest::CallLlm { messages, .. } => messages.iter().any(|message| matches!(
                message,
                Message::User(message) if matches!(&message.content, UserContent::Text(text)
                    if text.contains("implementation_authority_correction")
                        && text.contains("ordinary exact read"))
            )),
            _ => false,
        }));

        let unchosen_reference = correction_references
            .iter()
            .find(|reference| *reference != &selected_reference)
            .unwrap()
            .clone();
        let stale_correction = complete_llm(
            &mut machine,
            assistant(vec![
                (
                    "second-correction",
                    "codebase_memory_get_code_snippet",
                    serde_json::json!({
                        "qualified_name": selected_reference,
                        "decision_evidence_kind": "implementation",
                    }),
                ),
                (
                    "unchosen-correction",
                    "codebase_memory_get_code_snippet",
                    serde_json::json!({
                        "qualified_name": unchosen_reference,
                        "decision_evidence_kind": "implementation",
                    }),
                ),
            ]),
        );
        let stale_failures = ["second-correction", "unchosen-correction"]
            .map(|id| (id, local_failure(&stale_correction, id)));
        for (id, failure) in stale_failures {
            let _ = complete_tool(&mut machine, id, failed_output(), Some(failure));
        }

        let old_read = complete_llm(
            &mut machine,
            assistant(vec![(
                "old-target-read",
                "read",
                serde_json::json!({"path":"demo/src/model.rs"}),
            )]),
        );
        assert!(old_read.iter().any(|request| matches!(
            request,
            AgentRequest::RunTool { call, denial: None, rejection: None, .. }
                if call.id == "old-target-read"
        )));
        let _ = complete_tool(&mut machine, "old-target-read", successful_output(), None);
        let old_mutation = complete_llm(
            &mut machine,
            assistant(vec![(
                "old-target-mutation",
                "write",
                serde_json::json!({"path":"demo/src/model.rs","content":"changed"}),
            )]),
        );
        assert!(old_mutation.iter().any(|request| matches!(
            request,
            AgentRequest::RunTool {
                call,
                denial: Some(ToolCallDenial::DecisionAnchorMutation),
                ..
            } if call.id == "old-target-mutation"
        )));
        let _ = complete_tool(
            &mut machine,
            "old-target-mutation",
            failed_output(),
            Some(ToolFailureDiagnostic::policy_denial()),
        );

        let exact_read = complete_llm(
            &mut machine,
            assistant(vec![(
                "corrected-target-read",
                "read",
                serde_json::json!({"path":"demo/src/route.rs"}),
            )]),
        );
        let _ = complete_tool(
            &mut machine,
            "corrected-target-read",
            successful_output(),
            None,
        );
        let minimal_mutation = complete_llm(
            &mut machine,
            assistant(vec![(
                "minimal-route-mutation",
                "write",
                serde_json::json!({"path":"demo/src/route.rs","content":"changed"}),
            )]),
        );
        assert!(minimal_mutation.iter().any(|request| matches!(
            request,
            AgentRequest::RunTool { call, denial: None, rejection: None, .. }
                if call.id == "minimal-route-mutation"
        )));
        assert!(exact_read.iter().any(|request| matches!(
            request,
            AgentRequest::RunTool { call, denial: None, rejection: None, .. }
                if call.id == "corrected-target-read"
        )));
    });
}

#[test]
fn corrects_affinity_when_first_inspection_finishes_first() {
    run_implementation_correction_inspection(false, false);
}

#[test]
fn corrects_affinity_when_inspection_completion_reverses() {
    run_implementation_correction_inspection(true, false);
}

#[test]
fn retains_worker_slot_when_first_inspection_finishes_first() {
    run_implementation_correction_inspection(false, true);
}

#[test]
fn retains_worker_slot_when_inspection_completion_reverses() {
    run_implementation_correction_inspection(true, true);
}
