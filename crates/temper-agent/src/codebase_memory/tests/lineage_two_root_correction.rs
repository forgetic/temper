fn run_two_root_correction_regression(
    reverse_preview_completion: bool,
    retain_route_authority: bool,
) {
    temper_agent_io::block_on(async move {
        let TwoRootCorrectionHarness {
            _server,
            _workspace,
            log_path,
            admission,
            registry,
            mut machine,
        } = two_root_correction_harness().await;
        let mut retained_metadata = Vec::new();
        let mut retained_diagnostics = Vec::new();

        let roots = complete_llm(
            &mut machine,
            assistant(vec![
                (
                    "implementation-root",
                    "codebase_memory_search_graph",
                    serde_json::json!({"query":"routing implementation authority"}),
                ),
                (
                    "focused-root",
                    "codebase_memory_search_graph",
                    serde_json::json!({"query":"focused routing regression"}),
                ),
            ]),
        );
        let search = registry.get("codebase_memory_search_graph").unwrap();
        let implementation_call = dispatched_call(&roots, "implementation-root");
        let implementation_output = search
            .execute(
                &implementation_call.id,
                implementation_call.arguments,
                None,
            )
            .await
            .unwrap();
        let implementation_root = lineage(&implementation_output);
        let implementation_references = references(&implementation_output);
        assert_eq!(implementation_references.len(), 3);
        retained_metadata.push(implementation_output.details.clone());

        let focused_call = dispatched_call(&roots, "focused-root");
        let focused_output = search
            .execute(&focused_call.id, focused_call.arguments, None)
            .await
            .unwrap();
        let focused_root = lineage(&focused_output);
        assert_ne!(
            implementation_root.root_binding,
            focused_root.root_binding
        );
        assert_eq!(
            focused_root.focused_test_discovery,
            Some(FocusedTestDiscoveryOutcomeV1::EligibleSelectorReturned)
        );
        retained_metadata.push(focused_output.details.clone());
        assert!(complete_tool(
            &mut machine,
            "implementation-root",
            implementation_output,
            None,
        )
        .is_empty());
        let after_roots = complete_tool(&mut machine, "focused-root", focused_output, None);
        assert_eq!(
            handoff_references(active_handoff(&after_roots)),
            implementation_references
        );

        let provisional_reference = implementation_references
            [if retain_route_authority { 2 } else { 0 }]
        .clone();
        let provisional_preview = complete_llm(
            &mut machine,
            assistant(vec![(
                "provisional-preview",
                "codebase_memory_get_code_snippet",
                serde_json::json!({"qualified_name": provisional_reference}),
            )]),
        );
        let source = registry
            .get("codebase_memory_get_code_snippet")
            .unwrap();
        let preview_call = dispatched_call(&provisional_preview, "provisional-preview");
        let preview_output = source
            .execute(&preview_call.id, preview_call.arguments, None)
            .await
            .unwrap();
        let expected_provisional_selector = if retain_route_authority {
            TWO_ROOT_ROUTE_SELECTOR
        } else {
            TWO_ROOT_MODEL_SELECTOR
        };
        assert!(
            crate::codebase_memory::tests::test_support::output_text(&preview_output)
                .contains(expected_provisional_selector)
        );
        retained_metadata.push(preview_output.details.clone());
        let preview_done = complete_tool(
            &mut machine,
            "provisional-preview",
            preview_output,
            None,
        );
        assert!(active_handoff(&preview_done).contains("explicit commit call"));

        let provisional_source = complete_llm(
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
        let provisional_call = dispatched_call(&provisional_source, "provisional-source");
        let provisional_output = source
            .execute(
                &provisional_call.id,
                provisional_call.arguments,
                None,
            )
            .await
            .unwrap();
        let provisional_lineage = lineage(&provisional_output);
        assert_eq!(
            provisional_lineage.root_binding,
            implementation_root.root_binding
        );
        assert_eq!(
            provisional_lineage.decision_evidence_kind,
            Some(DecisionEvidenceKindV1::Implementation)
        );
        retained_metadata.push(provisional_output.details.clone());
        let trace_handoff = complete_tool(
            &mut machine,
            "provisional-source",
            provisional_output,
            None,
        );
        let trace_reference = opaque_references(active_handoff(&trace_handoff))
            .into_iter()
            .next()
            .expect("provisional implementation trace reference");

        let trace_requests = complete_llm(
            &mut machine,
            assistant(vec![(
                "implementation-trace",
                "codebase_memory_trace_path",
                serde_json::json!({
                    "function_name": trace_reference,
                    "mode": "calls",
                    "direction": "inbound",
                    "include_tests": false,
                }),
            )]),
        );
        let trace_call = dispatched_call(&trace_requests, "implementation-trace");
        let trace_output = registry
            .get("codebase_memory_trace_path")
            .unwrap()
            .execute(&trace_call.id, trace_call.arguments, None)
            .await
            .unwrap();
        let trace_lineage = lineage(&trace_output);
        assert_eq!(
            trace_lineage.root_binding,
            implementation_root.root_binding
        );
        assert!(trace_lineage.implementation_correction_available);
        let trace_text = crate::codebase_memory::tests::test_support::output_text(&trace_output);
        assert!(trace_text.contains(TWO_ROOT_CALLER_SELECTOR));
        if !retain_route_authority {
            assert!(trace_text.contains(TWO_ROOT_ROUTE_SELECTOR));
        }
        retained_metadata.push(trace_output.details.clone());
        let caller_handoff = complete_tool(
            &mut machine,
            "implementation-trace",
            trace_output,
            None,
        );
        let caller_reference = opaque_references(active_handoff(&caller_handoff))
            .into_iter()
            .next()
            .expect("typed implementation-root caller reference");

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
        let caller_lineage = lineage(&caller_output);
        assert_eq!(
            caller_lineage.root_binding,
            implementation_root.root_binding
        );
        assert_eq!(
            caller_lineage.decision_evidence_kind,
            Some(DecisionEvidenceKindV1::Caller)
        );
        retained_metadata.push(caller_output.details.clone());
        let focused_handoff = complete_tool(&mut machine, "caller-source", caller_output, None);
        let focused_reference = opaque_references(active_handoff(&focused_handoff))
            .into_iter()
            .next()
            .expect("distinct retained focused-test-root source reference");

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
        let focused_lineage = lineage(&focused_output);
        assert_eq!(focused_lineage.root_binding, focused_root.root_binding);
        assert_ne!(
            focused_lineage.root_binding,
            implementation_root.root_binding
        );
        assert_eq!(
            focused_lineage.decision_evidence_kind,
            Some(DecisionEvidenceKindV1::FocusedTest)
        );
        assert!(
            crate::codebase_memory::tests::test_support::output_text(&focused_output)
                .contains(TWO_ROOT_TEST_SELECTOR)
        );
        retained_metadata.push(focused_output.details.clone());
        let correction_handoff =
            complete_tool(&mut machine, "focused-source", focused_output, None);
        let correction_text = active_handoff(&correction_handoff).to_string();
        let correction_references = handoff_references(&correction_text);
        assert_eq!(correction_references.len(), 2);
        assert!(correction_text.contains("implementation correction inspection"));

        let closed_graph = complete_llm(
            &mut machine,
            assistant(vec![(
                "closed-general-exploration",
                "codebase_memory_search_graph",
                serde_json::json!({"query":"unrelated exploration after convergence"}),
            )]),
        );
        assert!(closed_graph.iter().any(|request| matches!(
            request,
            AgentRequest::RunTool {
                call,
                denial: Some(ToolCallDenial::GraphExplorationClosed(Some(details))),
                rejection: None,
                ..
            } if call.id == "closed-general-exploration"
                && details.reason
                    == temper_protocol_activity::GraphExplorationClosedReasonV1::Completed
        )));
        let closed_failure = local_failure(&closed_graph, "closed-general-exploration");
        retained_diagnostics.push(closed_failure.clone());
        let after_closed = complete_tool(
            &mut machine,
            "closed-general-exploration",
            failed_output(),
            Some(closed_failure),
        );
        assert_eq!(
            handoff_references(active_handoff(&after_closed)),
            correction_references,
            "the published correction menu remains usable after graph closure",
        );
        assert_eq!(
            crate::codebase_memory::tests::test_support::calls_named(&log_path, "search_graph")
                .len(),
            2,
            "closed exploration never reaches the provider",
        );

        let provisional_path = if retain_route_authority {
            "repo/src/route.rs"
        } else {
            "repo/src/model.rs"
        };
        let premature_read = complete_llm(
            &mut machine,
            assistant(vec![(
                "read-before-correction-inspection",
                "read",
                serde_json::json!({"path": provisional_path}),
            )]),
        );
        assert!(premature_read.iter().any(|request| matches!(
            request,
            AgentRequest::RunTool {
                call,
                denial: Some(ToolCallDenial::DecisionAnchorCorrectionInspection),
                rejection: None,
                ..
            } if call.id == "read-before-correction-inspection"
        )));
        let premature_failure = local_failure(&premature_read, "read-before-correction-inspection");
        retained_diagnostics.push(premature_failure.clone());
        let _ = complete_tool(
            &mut machine,
            "read-before-correction-inspection",
            failed_output(),
            Some(premature_failure),
        );

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
        let mut preview_results = Vec::new();
        for (index, id) in ["correction-preview-one", "correction-preview-two"]
            .into_iter()
            .enumerate()
        {
            let call = dispatched_call(&correction_previews, id);
            let output = source
                .execute(&call.id, call.arguments, None)
                .await
                .unwrap();
            assert_eq!(lineage(&output).decision_evidence_kind, None);
            retained_metadata.push(output.details.clone());
            preview_results.push((id, correction_references[index].clone(), output));
        }
        let route_correction_reference = preview_results
            .iter()
            .find(|(_, _, output)| {
                crate::codebase_memory::tests::test_support::output_text(output)
                    .contains(TWO_ROOT_ROUTE_SELECTOR)
            })
            .map(|(_, reference, _)| reference.clone());
        if !retain_route_authority {
            assert!(route_correction_reference.is_some());
        }
        if reverse_preview_completion {
            preview_results.reverse();
        }
        let (first_id, _, first_output) = preview_results.remove(0);
        assert!(complete_tool(&mut machine, first_id, first_output, None).is_empty());
        let (second_id, _, second_output) = preview_results.remove(0);
        let after_previews = complete_tool(&mut machine, second_id, second_output, None);
        assert_eq!(
            handoff_references(active_handoff(&after_previews)),
            correction_references
        );

        let selected_correction_reference = if retain_route_authority {
            None
        } else {
            let selected = route_correction_reference.expect("route correction reference");
            let correction = complete_llm(
                &mut machine,
                assistant(vec![(
                    "corrected-route-source",
                    "codebase_memory_get_code_snippet",
                    serde_json::json!({
                        "qualified_name": selected,
                        "decision_evidence_kind": "implementation",
                    }),
                )]),
            );
            let correction_call = dispatched_call(&correction, "corrected-route-source");
            let correction_output = source
                .execute(&correction_call.id, correction_call.arguments, None)
                .await
                .unwrap();
            let correction_lineage = lineage(&correction_output);
            assert_eq!(
                correction_lineage.root_binding,
                implementation_root.root_binding
            );
            assert!(correction_lineage.implementation_authority_corrected);
            let TargetAdmissionOutcome::Eligible(corrected_target) =
                admission.resolve_source_target(&correction_lineage)
            else {
                panic!("corrected route source target");
            };
            let InvocationTargetAdmission::Read(TargetAdmissionOutcome::Eligible(route_target)) =
                admission.resolve_invocation_targets(
                    "read",
                    &serde_json::json!({"path":"repo/src/route.rs"}),
                )
            else {
                panic!("exact route read target");
            };
            assert!(corrected_target.matches(&route_target));
            retained_metadata.push(correction_output.details.clone());
            let after_correction = complete_tool(
                &mut machine,
                "corrected-route-source",
                correction_output,
                None,
            );
            assert!(!has_active_root_handoff(&after_correction));
            Some(selected)
        };

        let exact_route_read = complete_llm(
            &mut machine,
            assistant(vec![(
                "exact-route-read",
                "read",
                serde_json::json!({"path":"repo/src/route.rs"}),
            )]),
        );
        assert!(exact_route_read.iter().any(|request| matches!(
            request,
            AgentRequest::RunTool { call, denial: None, rejection: None, .. }
                if call.id == "exact-route-read"
        )));
        let after_exact_read = complete_tool(
            &mut machine,
            "exact-route-read",
            successful_output(),
            None,
        );
        assert!(!has_active_root_handoff(&after_exact_read));

        for (id, path) in [
            ("old-model-mutation", "repo/src/model.rs"),
            ("unrelated-mutation", "repo/src/unrelated.rs"),
        ] {
            let denied = complete_llm(
                &mut machine,
                assistant(vec![(
                    id,
                    "write",
                    serde_json::json!({"path":path,"content":"changed"}),
                )]),
            );
            assert_decision_anchor_mutation_denial(&denied, id);
            let failure = ToolFailureDiagnostic::policy_denial();
            retained_diagnostics.push(failure.clone());
            let _ = complete_tool(&mut machine, id, failed_output(), Some(failure));
        }

        for (read_id, path) in [
            ("old-model-read", "repo/src/model.rs"),
            ("post-decision-route-read", "repo/src/route.rs"),
        ] {
            let reads = complete_llm(
                &mut machine,
                assistant(vec![(read_id, "read", serde_json::json!({"path": path}))]),
            );
            assert!(reads.iter().any(|request| matches!(
                request,
                AgentRequest::RunTool { call, denial: None, rejection: None, .. }
                    if call.id == read_id
            )));
            let _ = complete_tool(&mut machine, read_id, successful_output(), None);
        }

        let selected_or_first = selected_correction_reference
            .as_ref()
            .unwrap_or(&correction_references[0]);
        let unchosen_reference = correction_references
            .iter()
            .find(|reference| *reference != selected_or_first)
            .unwrap();
        let stale_references = complete_llm(
            &mut machine,
            assistant(vec![
                (
                    "stale-selected-preview",
                    "codebase_memory_get_code_snippet",
                    serde_json::json!({"qualified_name": selected_or_first}),
                ),
                (
                    "stale-unchosen-correction",
                    "codebase_memory_get_code_snippet",
                    serde_json::json!({
                        "qualified_name": unchosen_reference,
                        "decision_evidence_kind": "implementation",
                    }),
                ),
            ]),
        );
        let stale_failures = ["stale-selected-preview", "stale-unchosen-correction"]
            .map(|id| (id, local_failure(&stale_references, id)));
        for (id, failure) in stale_failures {
            retained_diagnostics.push(failure.clone());
            let _ = complete_tool(&mut machine, id, failed_output(), Some(failure));
        }
        assert_eq!(
            crate::codebase_memory::tests::test_support::calls_named(
                &log_path,
                "get_code_snippet"
            )
            .len(),
            if retain_route_authority { 6 } else { 7 },
            "retired correction references cannot be reused at the provider",
        );

        assert_two_root_companion_mutation(&mut machine);

        let still_closed = complete_llm(
            &mut machine,
            assistant(vec![(
                "post-decision-graph",
                "codebase_memory_search_graph",
                serde_json::json!({"query":"attempt to reopen completed decision"}),
            )]),
        );
        let still_closed_failure = local_failure(&still_closed, "post-decision-graph");
        retained_diagnostics.push(still_closed_failure.clone());
        let _ = complete_tool(
            &mut machine,
            "post-decision-graph",
            failed_output(),
            Some(still_closed_failure),
        );
        assert_eq!(
            crate::codebase_memory::tests::test_support::calls_named(&log_path, "search_graph")
                .len(),
            2,
        );

        let intended_mutation = complete_llm(
            &mut machine,
            assistant(vec![(
                "intended-route-mutation",
                "write",
                serde_json::json!({"path":"repo/src/route.rs","content":"changed"}),
            )]),
        );
        assert_eq!(
            intended_mutation
                .iter()
                .filter(|request| matches!(
                    request,
                    AgentRequest::RunTool { call, denial: None, rejection: None, .. }
                        if call.id == "intended-route-mutation"
                ))
                .count(),
            1,
            "the exact route mutation is admitted once after all stale state is settled",
        );

        assert_two_root_privacy(
            &retained_metadata,
            &retained_diagnostics,
            &correction_text,
            &correction_references,
            &implementation_root,
            &focused_root,
        );
    });
}
