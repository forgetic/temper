// Deterministic provider-derived routing regressions.

use super::*;
use crate::{EligibleLineageAdmission, LineageAdmissionOutcome, LineageAdmissionStatus};

fn source_admission(root: &str, kind: DecisionEvidenceKindV1) -> LineageAdmissionOutcome {
    LineageAdmissionOutcome::Eligible(
        EligibleLineageAdmission::new(
            root.to_string(),
            DecisionAnchorTargetKindV1::QualifiedName,
            GraphCorrelationToolV1::GetCodeSnippet,
            Some(kind),
        )
        .expect("closed source admission"),
    )
}

fn trace_admission(root: &str) -> LineageAdmissionOutcome {
    LineageAdmissionOutcome::Eligible(
        EligibleLineageAdmission::implementation_caller_traversal(
            root.to_string(),
            DecisionAnchorTargetKindV1::FunctionName,
        )
        .expect("closed trace admission"),
    )
}

fn install_implementation_and_focused_roots(state: &mut DecisionAnchorState) {
    for id in ["implementation-root", "focused-root"] {
        state.on_tool_dispatched(&call(id, "codebase_memory_search_graph"), 0);
    }
    let implementation = output_with_kinds(
        "codebase_memory_search_graph",
        ROOT,
        DecisionAnchorLineageStageV1::Root,
        &[
            DecisionAnchorTargetKindV1::FunctionName,
            DecisionAnchorTargetKindV1::QualifiedName,
        ],
    );
    let focused = output_with_focused_test_discovery(
        "codebase_memory_search_graph",
        OTHER_ROOT,
        DecisionAnchorLineageStageV1::Root,
        FocusedTestDiscoveryOutcomeV1::EligibleSelectorReturned,
    );
    assert_eq!(
        state.on_tool_batch_finished(&[
            (
                "implementation-root",
                "codebase_memory_search_graph",
                &implementation,
            ),
            (
                "focused-root",
                "codebase_memory_search_graph",
                &focused,
            ),
        ]),
        DecisionAnchorTransition::Unchanged,
    );
    state.take_model_guidance();
}

#[test]
fn implementation_and_focused_test_roots_form_one_complete_retained_forest() {
    let mut state = DecisionAnchorState::from_effects(&effects()).unwrap();
    install_implementation_and_focused_roots(&mut state);

    let implementation = source_call("implementation", DecisionEvidenceKindV1::Implementation);
    assert_eq!(
        state.on_tool_dispatched_with_admission(
            &implementation,
            1,
            Some(&source_admission(
                ROOT,
                DecisionEvidenceKindV1::Implementation,
            )),
        ),
        None,
    );
    assert_eq!(
        finish_with_evidence(
            &mut state,
            "implementation",
            ROOT,
            DecisionEvidenceKindV1::Implementation,
        ),
        DecisionAnchorTransition::Unchanged,
    );
    state.take_model_guidance();

    let trace = call("trace", "codebase_memory_trace_path");
    let speculative_caller = source_call("speculative-caller", DecisionEvidenceKindV1::Caller);
    let speculative_test = source_call("speculative-test", DecisionEvidenceKindV1::FocusedTest);
    let denials = state.on_tool_batch_dispatched_with_admissions(
        &[trace, speculative_caller, speculative_test],
        2,
        &[
            Some(trace_admission(ROOT)),
            Some(source_admission(ROOT, DecisionEvidenceKindV1::Caller)),
            Some(source_admission(
                OTHER_ROOT,
                DecisionEvidenceKindV1::FocusedTest,
            )),
        ],
    );
    assert_eq!(denials[0], None);
    assert!(denials[1].is_some(), "same-batch caller consumer is denied");
    assert!(
        denials[2].is_some(),
        "the focused sibling is not consumed while the implementation route is active"
    );
    assert_eq!(
        state.on_tool_finished(
            "trace",
            "codebase_memory_trace_path",
            &output_with_caller_discovery(
                ROOT,
                DecisionAnchorLineageStageV1::CarryForward,
                CallerDiscoveryOutcomeV1::EligibleSelectorReturned,
            ),
        ),
        DecisionAnchorTransition::GapRecoveryNeeded,
    );
    state.take_model_guidance();

    let mut caller = source_call("caller", DecisionEvidenceKindV1::Caller);
    caller.arguments["qualified_name"] = serde_json::json!("provider-returned-caller");
    assert_eq!(
        state.on_tool_dispatched_with_admission(
            &caller,
            3,
            Some(&source_admission(ROOT, DecisionEvidenceKindV1::Caller)),
        ),
        None,
    );
    assert_eq!(
        finish_with_evidence(&mut state, "caller", ROOT, DecisionEvidenceKindV1::Caller),
        DecisionAnchorTransition::GapRecoveryNeeded,
    );
    let guidance = state.take_model_guidance();
    assert!(guidance.iter().any(|message| message.contains(
        "required next stage=[get_code_snippet/qualified_name/focused_test/selector=focused_test_result]"
    )));

    let mut focused = source_call("focused", DecisionEvidenceKindV1::FocusedTest);
    focused.arguments["qualified_name"] = serde_json::json!("provider-returned-focused-test");
    assert_eq!(
        state.on_tool_dispatched_with_admission(
            &focused,
            4,
            Some(&source_admission(
                OTHER_ROOT,
                DecisionEvidenceKindV1::FocusedTest,
            )),
        ),
        None,
    );
    assert_eq!(
        finish_with_evidence(
            &mut state,
            "focused",
            OTHER_ROOT,
            DecisionEvidenceKindV1::FocusedTest,
        ),
        DecisionAnchorTransition::EnabledEvidenceComplete,
    );
    assert!(state.blocks_mutation("write"), "the exact ordinary read is still required");
}

#[test]
fn missing_caller_has_one_active_root_continuation_and_rejections_are_bounded() {
    let mut state = DecisionAnchorState::from_effects(&effects()).unwrap();
    install_implementation_and_focused_roots(&mut state);
    state.on_tool_dispatched_with_admission(
        &source_call("implementation", DecisionEvidenceKindV1::Implementation),
        1,
        Some(&source_admission(
            ROOT,
            DecisionEvidenceKindV1::Implementation,
        )),
    );
    finish_with_evidence(
        &mut state,
        "implementation",
        ROOT,
        DecisionEvidenceKindV1::Implementation,
    );
    state.take_model_guidance();
    state.on_tool_dispatched_with_admission(
        &call("trace", "codebase_memory_trace_path"),
        2,
        Some(&trace_admission(ROOT)),
    );
    state.on_tool_finished(
        "trace",
        "codebase_memory_trace_path",
        &output_with_caller_discovery(
            ROOT,
            DecisionAnchorLineageStageV1::CarryForward,
            CallerDiscoveryOutcomeV1::EligibleSelectorReturned,
        ),
    );
    state.take_model_guidance();

    let rejected = source_call("wrong", DecisionEvidenceKindV1::Caller);
    assert!(
        state
            .on_tool_dispatched_with_admission(
                &rejected,
                3,
                Some(&LineageAdmissionOutcome::Ineligible(
                    LineageAdmissionStatus::UnknownSelector,
                )),
            )
            .is_some()
    );
    let guidance = state.take_model_guidance();
    assert_eq!(guidance.len(), 1);
    assert!(guidance[0].contains("remaining allowance=3"));
    assert!(guidance[0].contains(
        "required next stage=[get_code_snippet/qualified_name/caller/selector=caller_traversal_result]"
    ));
    assert!(guidance[0].contains("do not repeat that selector value"));
}

#[test]
fn successful_enabled_route_without_a_focused_test_selector_stops_without_fallback() {
    let mut state = DecisionAnchorState::from_effects(&effects()).unwrap();
    state.on_tool_dispatched(&call("root", "codebase_memory_search_graph"), 0);
    state.on_tool_finished(
        "root",
        "codebase_memory_search_graph",
        &output_with_kinds(
            "codebase_memory_search_graph",
            ROOT,
            DecisionAnchorLineageStageV1::Root,
            &[
                DecisionAnchorTargetKindV1::FunctionName,
                DecisionAnchorTargetKindV1::QualifiedName,
            ],
        ),
    );
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
    state.on_tool_dispatched(&call("trace", "codebase_memory_trace_path"), 2);
    state.on_tool_finished(
        "trace",
        "codebase_memory_trace_path",
        &output_with_caller_discovery(
            ROOT,
            DecisionAnchorLineageStageV1::CarryForward,
            CallerDiscoveryOutcomeV1::EligibleSelectorReturned,
        ),
    );
    state.on_tool_dispatched(&source_call("caller", DecisionEvidenceKindV1::Caller), 3);
    assert_eq!(
        finish_with_evidence(&mut state, "caller", ROOT, DecisionEvidenceKindV1::Caller),
        DecisionAnchorTransition::EnabledEvidenceIncomplete,
    );
    assert_eq!(
        state.on_tool_dispatched(&call("retry", "codebase_memory_search_graph"), 4),
        exhausted_graph_denial([GraphRecoveryEvidenceKindV1::FocusedTest]),
    );
    assert!(state.blocks_mutation("write"));
}

#[test]
fn unavailable_expected_active_root_action_releases_only_the_distinct_fallback() {
    let mut state = DecisionAnchorState::from_effects(&effects()).unwrap();
    install_implementation_and_focused_roots(&mut state);
    let implementation = source_call("implementation", DecisionEvidenceKindV1::Implementation);
    assert_eq!(
        state.on_tool_dispatched_with_admission(
            &implementation,
            1,
            Some(&source_admission(
                ROOT,
                DecisionEvidenceKindV1::Implementation,
            )),
        ),
        None,
    );
    assert_eq!(
        state.on_tool_finished(
            "implementation",
            "codebase_memory_get_code_snippet",
            &failure_output("transport"),
        ),
        DecisionAnchorTransition::ProviderUnavailableFallback,
    );
    assert_eq!(
        state.on_tool_dispatched(&call("no-retry", "codebase_memory_search_graph"), 2),
        conventional_fallback_graph_denial(),
    );
}
