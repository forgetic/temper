// Stateful, privacy-safe model guidance regressions.

mod progress {
    use super::*;

    fn one_guidance(state: &mut DecisionAnchorState) -> String {
        let guidance = state.take_model_guidance();
        assert_eq!(
            guidance.len(),
            1,
            "one completed result has one classification"
        );
        guidance.into_iter().next().unwrap()
    }

    #[test]
    fn accepted_current_root_steps_change_guidance_until_exact_read_convergence() {
        let mut state = DecisionAnchorState::from_effects(&effects()).unwrap();
        state.on_tool_dispatched(&call("root", "codebase_memory_search_graph"), 0);
        finish(
            &mut state,
            "root",
            "codebase_memory_search_graph",
            ROOT,
            DecisionAnchorLineageStageV1::Root,
        );
        let root = one_guidance(&mut state);
        assert!(root.contains("result=active_root_progress"));
        assert!(root.contains("accepted evidence=[root]"));
        assert!(root.contains(
            "active-root missing evidence=[trace, implementation, caller, focused_test]"
        ));
        assert!(root.contains("get_code_snippet/qualified_name/implementation"));

        state.on_tool_dispatched(
            &source_call("implementation", DecisionEvidenceKindV1::Implementation),
            1,
        );
        finish_with_evidence(
            &mut state,
            "implementation",
            ROOT,
            DecisionEvidenceKindV1::Implementation,
        );
        let implementation = one_guidance(&mut state);
        assert!(implementation.contains("accepted evidence=[implementation]"));
        assert!(
            implementation.contains("active-root missing evidence=[trace, caller, focused_test]")
        );
        assert!(implementation.contains("trace_path/function_name/trace"));
        assert_ne!(root, implementation);

        state.on_tool_dispatched(&call("trace", "codebase_memory_trace_path"), 2);
        finish(
            &mut state,
            "trace",
            "codebase_memory_trace_path",
            ROOT,
            DecisionAnchorLineageStageV1::CarryForward,
        );
        let trace = one_guidance(&mut state);
        assert!(trace.contains("accepted evidence=[trace]"));
        assert!(trace.contains("active-root missing evidence=[caller, focused_test]"));
        assert!(trace.contains("get_code_snippet/qualified_name/caller"));

        state.on_tool_dispatched(&source_call("caller", DecisionEvidenceKindV1::Caller), 3);
        finish_with_evidence(&mut state, "caller", ROOT, DecisionEvidenceKindV1::Caller);
        let caller = one_guidance(&mut state);
        assert!(caller.contains("accepted evidence=[caller]"));
        assert!(caller.contains("active-root missing evidence=[focused_test]"));

        state.on_tool_dispatched(
            &source_call("focused-test", DecisionEvidenceKindV1::FocusedTest),
            4,
        );
        assert_eq!(
            finish_with_evidence(
                &mut state,
                "focused-test",
                ROOT,
                DecisionEvidenceKindV1::FocusedTest,
            ),
            DecisionAnchorTransition::Converged,
        );
        let complete = one_guidance(&mut state);
        assert!(complete.contains("accepted evidence=[focused_test]"));
        assert!(complete.contains("active-root missing evidence=[]"));
        assert!(complete.contains("recovery=complete"));
        assert!(complete.contains("read/workspace_target/exact_post_source"));
        assert!(complete.contains("perform one successful ordinary exact read"));
        assert!(complete.contains("matching minimal mutation"));

        for private in [ROOT, "repo/src/route.rs", "sha256", "private::selector"] {
            assert!(!complete.contains(private));
        }
    }

    #[test]
    fn sibling_result_is_explicitly_non_authoritative_for_the_active_root() {
        let mut state = DecisionAnchorState::from_effects(&effects()).unwrap();
        state.on_tool_dispatched(&call("root", "codebase_memory_search_graph"), 0);
        finish(
            &mut state,
            "root",
            "codebase_memory_search_graph",
            ROOT,
            DecisionAnchorLineageStageV1::Root,
        );
        state.take_model_guidance();

        state.on_tool_dispatched(&call("sibling", "codebase_memory_search_graph"), 1);
        finish(
            &mut state,
            "sibling",
            "codebase_memory_search_graph",
            OTHER_ROOT,
            DecisionAnchorLineageStageV1::Root,
        );
        let guidance = one_guidance(&mut state);
        assert!(guidance.contains("result=sibling_or_cross_root_evidence"));
        assert!(guidance.contains("accepted evidence=[]"));
        assert!(guidance.contains("does not satisfy the selected active root"));
        assert!(!guidance.contains(ROOT));
        assert!(!guidance.contains(OTHER_ROOT));
    }

    #[test]
    fn rejected_recovery_tuple_stays_excluded_while_a_distinct_selector_remains_actionable() {
        const PRIVATE_SELECTOR: &str = "private::repeated::implementation";
        const DISTINCT_SELECTOR: &str = "private::eligible::implementation";
        let mut state = DecisionAnchorState::from_effects(&effects()).unwrap();
        state.on_tool_dispatched(&call("root", "codebase_memory_search_graph"), 0);
        finish(
            &mut state,
            "root",
            "codebase_memory_search_graph",
            ROOT,
            DecisionAnchorLineageStageV1::Root,
        );
        state.take_model_guidance();
        for (turn, id, expected) in [
            (1, "broad-one", DecisionAnchorTransition::Unchanged),
            (2, "broad-two", DecisionAnchorTransition::GapRecoveryNeeded),
        ] {
            state.on_tool_dispatched(&call(id, "codebase_memory_get_architecture"), turn);
            assert_eq!(
                state.on_tool_finished(id, "codebase_memory_get_architecture", &plain_success()),
                expected,
            );
            state.take_model_guidance();
        }

        let mut rejected = source_call("denied", DecisionEvidenceKindV1::Implementation);
        rejected.arguments["qualified_name"] = serde_json::json!(PRIVATE_SELECTOR);
        assert_eq!(
            state.on_tool_dispatched_with_admission(
                &rejected,
                3,
                Some(&LineageAdmissionOutcome::Ineligible(
                    LineageAdmissionStatus::UnknownSelector,
                )),
            ),
            recovery_graph_denial(all_missing(), 4),
        );
        let first_guidance = one_guidance(&mut state);
        assert!(first_guidance.contains("result=non_progress"));
        assert!(first_guidance.contains("rejected selector tuple excluded=true"));
        assert!(first_guidance.contains("remaining allowance=4"));
        assert!(first_guidance.contains("get_code_snippet/qualified_name/implementation"));
        assert!(!first_guidance.contains(PRIVATE_SELECTOR));
        assert_eq!(
            state.on_tool_finished(
                "denied",
                "codebase_memory_get_code_snippet",
                &plain_success(),
            ),
            DecisionAnchorTransition::Unchanged,
        );

        let eligible_same_tuple = EligibleLineageAdmission::new(
            ROOT.to_string(),
            DecisionAnchorTargetKindV1::QualifiedName,
            GraphCorrelationToolV1::GetCodeSnippet,
            Some(DecisionEvidenceKindV1::Implementation),
        )
        .unwrap();
        for (turn, id) in [(4, "retry"), (5, "eligible-retry")] {
            rejected.id = id.to_string();
            let admission = if id == "retry" {
                LineageAdmissionOutcome::Ineligible(LineageAdmissionStatus::UnknownSelector)
            } else {
                LineageAdmissionOutcome::Eligible(eligible_same_tuple.clone())
            };
            assert_eq!(
                state.on_tool_dispatched_with_admission(&rejected, turn, Some(&admission)),
                recovery_graph_denial(all_missing(), 4),
                "a closed tuple cannot be retried or later admitted",
            );
            let guidance = one_guidance(&mut state);
            assert!(guidance.contains("rejected selector tuple excluded=true"));
            assert!(guidance.contains("remaining allowance=4"));
            assert!(guidance.contains("get_code_snippet/qualified_name/implementation"));
            assert!(!guidance.contains(PRIVATE_SELECTOR));
            assert_eq!(
                state.on_tool_finished(id, "codebase_memory_get_code_snippet", &plain_success()),
                DecisionAnchorTransition::Unchanged,
            );
        }

        let mut distinct = source_call("distinct", DecisionEvidenceKindV1::Implementation);
        distinct.arguments["qualified_name"] = serde_json::json!(DISTINCT_SELECTOR);
        assert_eq!(
            state.on_tool_dispatched_with_admission(
                &distinct,
                6,
                Some(&LineageAdmissionOutcome::Eligible(eligible_same_tuple)),
            ),
            None,
        );
        assert_eq!(
            finish_with_evidence(
                &mut state,
                "distinct",
                ROOT,
                DecisionEvidenceKindV1::Implementation,
            ),
            DecisionAnchorTransition::GapRecoveryNeeded,
        );
        assert!(!one_guidance(&mut state).contains(DISTINCT_SELECTOR));
    }
}
