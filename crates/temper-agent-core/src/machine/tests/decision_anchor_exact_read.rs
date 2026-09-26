// Post-source exact-read admission and batching regressions.

include!("decision_anchor_companion_read.rs");

use std::sync::Arc;

use crate::{
    EligibleLineageAdmission, EligibleWorkspaceTarget, InvocationTargetAdmission,
    LineageAdmissionOutcome, LineageAdmissionResolver, LineageAdmissionStatus,
    TargetAdmissionOutcome, TargetAdmissionStatus,
};

const TARGET_A: &str = "00000000-0000-4000-8000-000000000011";
const TARGET_B: &str = "00000000-0000-4000-8000-000000000012";

fn exact_target(value: &str) -> EligibleWorkspaceTarget {
    EligibleWorkspaceTarget::new(value.to_string()).expect("opaque workspace target")
}

fn read_target(value: &str) -> InvocationTargetAdmission {
    InvocationTargetAdmission::Read(TargetAdmissionOutcome::Eligible(exact_target(value)))
}

fn mutation_targets(targets: Vec<TargetAdmissionOutcome>) -> InvocationTargetAdmission {
    InvocationTargetAdmission::Mutation(targets)
}

fn successful_read() -> ToolOutput {
    ToolOutput {
        content: Vec::new(),
        details: None,
        is_error: false,
    }
}

#[test]
fn only_successful_post_source_exact_reads_authorize_every_mutation_target() {
    let mut effects = effects();
    effects.insert("bash".to_string(), ToolEffects::process());
    effects.insert("submit_for_pr".to_string(), ToolEffects::process());
    let mut state = DecisionAnchorState::from_effects(&effects).unwrap();
    let target_a = TargetAdmissionOutcome::Eligible(exact_target(TARGET_A));

    let early = call("early-bulk-read", "read");
    assert_eq!(
        state.on_tool_dispatched_with_targets(&early, 0, Some(&read_target(TARGET_A))),
        None
    );
    state.on_tool_finished("early-bulk-read", "read", &successful_read());
    assert_eq!(
        state.on_tool_dispatched_with_targets(
            &call("pre-evidence-mutation", "write"),
            1,
            Some(&mutation_targets(vec![target_a.clone()])),
        ),
        Some(ToolCallDenial::DecisionAnchorMutation),
        "enabled mode cannot authorize mutation before graph evidence is complete",
    );

    state.on_tool_dispatched(&call("root", "codebase_memory_search_graph"), 1);
    finish(
        &mut state,
        "root",
        "codebase_memory_search_graph",
        ROOT,
        DecisionAnchorLineageStageV1::Root,
    );

    state.on_tool_dispatched(
        &source_call("implementation", DecisionEvidenceKindV1::Implementation),
        2,
    );
    state.on_tool_finished_with_source_target(
        "implementation",
        "codebase_memory_get_code_snippet",
        &output_with_evidence(
            ROOT,
            DecisionAnchorLineageStageV1::CarryForward,
            DecisionEvidenceKindV1::Implementation,
        ),
        Some(&target_a),
    );

    let incomplete = call("incomplete-evidence-read", "read");
    state.on_tool_dispatched_with_targets(&incomplete, 3, Some(&read_target(TARGET_A)));
    state.on_tool_finished(
        "incomplete-evidence-read",
        "read",
        &successful_read(),
    );

    state.on_tool_dispatched(&call("trace", "codebase_memory_trace_path"), 3);
    finish(
        &mut state,
        "trace",
        "codebase_memory_trace_path",
        ROOT,
        DecisionAnchorLineageStageV1::CarryForward,
    );
    state.on_tool_dispatched(
        &source_call("caller", DecisionEvidenceKindV1::Caller),
        4,
    );
    finish_with_evidence(
        &mut state,
        "caller",
        ROOT,
        DecisionEvidenceKindV1::Caller,
    );

    let final_source = source_call("focused", DecisionEvidenceKindV1::FocusedTest);
    let sibling_read = call("source-sibling-read", "read");
    assert_eq!(
        state.on_tool_batch_dispatched_with_admissions_and_targets(
            &[final_source, sibling_read],
            5,
            &[None, None],
            &[None, Some(read_target(TARGET_A))],
        ),
        [None, None],
    );
    let final_output = output_with_evidence(
        ROOT,
        DecisionAnchorLineageStageV1::CarryForward,
        DecisionEvidenceKindV1::FocusedTest,
    );
    let sibling_output = successful_read();
    assert_eq!(
        state.on_tool_batch_finished_with_targets(&[
            (
                "focused",
                "codebase_memory_get_code_snippet",
                &final_output,
                Some(&target_a),
                true,
            ),
            (
                "source-sibling-read",
                "read",
                &sibling_output,
                None,
                true,
            ),
        ]),
        DecisionAnchorTransition::EnabledEvidenceComplete,
    );
    assert_eq!(
        state.on_tool_dispatched(&call("closed-graph", "codebase_memory_search_graph"), 6),
        completed_graph_denial(),
        "graph exploration stays closed while an exact read is awaited",
    );

    for number in 0..6 {
        let id = format!("coverage-{number}");
        assert_eq!(state.on_tool_dispatched(&call(&id,"codebase_memory_check_index_coverage"),6),None);
        assert_eq!(state.on_tool_finished(&id,"codebase_memory_check_index_coverage",&successful_read()),DecisionAnchorTransition::Unchanged);
    }
    assert_eq!(state.on_tool_dispatched(&call("still-closed","codebase_memory_search_graph"),6),completed_graph_denial(),"coverage cannot reopen discovery");

    let mutation = call("direct-mutation", "write");
    assert_eq!(
        state.on_tool_dispatched_with_targets(
            &mutation,
            6,
            Some(&mutation_targets(vec![target_a.clone()])),
        ),
        Some(ToolCallDenial::DecisionAnchorMutation),
        "pre-completion and graph-source-sibling reads cannot gain retroactive authority",
    );

    let failed = call("failed-exact-read", "read");
    state.on_tool_dispatched_with_targets(&failed, 7, Some(&read_target(TARGET_A)));
    let failed_output = successful_read();
    state.on_tool_batch_finished_with_targets(&[(
        "failed-exact-read",
        "read",
        &failed_output,
        None,
        false,
    )]);

    let wrong = call("wrong-target-read", "read");
    state.on_tool_dispatched_with_targets(&wrong, 8, Some(&read_target(TARGET_B)));
    state.on_tool_finished("wrong-target-read", "read", &successful_read());

    let malformed = call("malformed-read", "read");
    state.on_tool_dispatched_with_targets(
        &malformed,
        9,
        Some(&InvocationTargetAdmission::Ineligible(
            TargetAdmissionStatus::MalformedTarget,
        )),
    );
    state.on_tool_finished("malformed-read", "read", &successful_read());

    let non_read = call("non-read", "grep");
    state.on_tool_dispatched_with_targets(&non_read, 10, Some(&read_target(TARGET_A)));
    state.on_tool_finished("non-read", "grep", &successful_read());

    assert_eq!(
        state.on_tool_dispatched_with_targets(
            &call("still-blocked", "write"),
            11,
            Some(&mutation_targets(vec![target_a.clone()])),
        ),
        Some(ToolCallDenial::DecisionAnchorMutation),
    );

    let matching = call("matching-read", "read");
    state.on_tool_dispatched_with_targets(&matching, 12, Some(&read_target(TARGET_A)));
    state.on_tool_finished("matching-read", "read", &successful_read());

    assert_eq!(
        state.on_tool_dispatched_with_targets(
            &call("multi-target", "write"),
            13,
            Some(&mutation_targets(vec![
                target_a.clone(),
                TargetAdmissionOutcome::Ineligible(TargetAdmissionStatus::UnknownTarget),
            ])),
        ),
        Some(ToolCallDenial::DecisionAnchorMutation),
        "an unmatched explicit target cannot piggyback on exact-read authority",
    );
    assert_eq!(
        state.on_tool_dispatched_with_targets(
            &call("matching-mutation", "write"),
            14,
            Some(&mutation_targets(vec![target_a])),
        ),
        None,
    );
    assert_eq!(
        state.on_tool_dispatched_with_targets(
            &call("source-mutating-shell", "bash"),
            15,
            Some(&InvocationTargetAdmission::Ineligible(
                TargetAdmissionStatus::UnsupportedTool,
            )),
        ),
        Some(ToolCallDenial::DecisionAnchorMutation),
        "an unclassified shell cannot bypass exact target admission",
    );
    assert_eq!(
        state.on_tool_dispatched_with_targets(
            &call("validation", "bash"),
            16,
            Some(&InvocationTargetAdmission::SourceNeutralProcess),
        ),
        None,
    );
    assert_eq!(
        state.on_tool_dispatched_with_targets(
            &call("submit", "submit_for_pr"),
            17,
            Some(&InvocationTargetAdmission::ControlPlane),
        ),
        None,
    );

    let debug = format!("{:?} {:?}", read_target(TARGET_A), read_target(TARGET_B));
    assert!(!debug.contains(TARGET_A));
    assert!(!debug.contains(TARGET_B));
}

include!("decision_anchor_authority_correction.rs");

#[test]
fn initial_provider_unavailability_requires_a_fresh_matching_conventional_read() {
    let mut state = DecisionAnchorState::from_effects(&effects()).unwrap();
    let target_a = TargetAdmissionOutcome::Eligible(exact_target(TARGET_A));

    let early = call("pre-failure-read", "read");
    state.on_tool_dispatched_with_targets(&early, 0, Some(&read_target(TARGET_A)));
    state.on_tool_finished("pre-failure-read", "read", &successful_read());

    state.on_tool_dispatched(&call("unavailable", "codebase_memory_search_graph"), 1);
    assert_eq!(
        state.on_tool_finished(
            "unavailable",
            "codebase_memory_search_graph",
            &failure_output("transport"),
        ),
        DecisionAnchorTransition::ProviderUnavailableFallback,
    );
    let mutation = call("fallback-mutation", "write");
    assert_eq!(
        state.on_tool_dispatched_with_targets(
            &mutation,
            2,
            Some(&mutation_targets(vec![target_a.clone()])),
        ),
        Some(ToolCallDenial::DecisionAnchorMutation),
        "a conventional read completed before unavailability is not fresh authority",
    );

    let wrong = call("wrong-fallback-read", "read");
    state.on_tool_dispatched_with_targets(&wrong, 3, Some(&read_target(TARGET_B)));
    state.on_tool_finished("wrong-fallback-read", "read", &successful_read());
    assert_eq!(
        state.on_tool_dispatched_with_targets(
            &mutation,
            4,
            Some(&mutation_targets(vec![target_a.clone()])),
        ),
        Some(ToolCallDenial::DecisionAnchorMutation),
    );

    let matching = call("matching-fallback-read", "read");
    state.on_tool_dispatched_with_targets(&matching, 5, Some(&read_target(TARGET_A)));
    state.on_tool_finished("matching-fallback-read", "read", &successful_read());
    assert_eq!(
        state.on_tool_dispatched_with_targets(
            &mutation,
            6,
            Some(&mutation_targets(vec![target_a])),
        ),
        None,
    );
}

#[test]
fn provider_failure_requires_fresh_conventional_authority_for_an_independent_target() {
    for focused_test_already_retained in [false, true] {
        let mut state = DecisionAnchorState::from_effects(&effects()).unwrap();
        let graph_target = TargetAdmissionOutcome::Eligible(exact_target(TARGET_A));
        let fallback_target = TargetAdmissionOutcome::Eligible(exact_target(TARGET_B));

        state.on_tool_dispatched(&call("root", "codebase_memory_search_graph"), 0);
        finish(
            &mut state,
            "root",
            "codebase_memory_search_graph",
            ROOT,
            DecisionAnchorLineageStageV1::Root,
        );
        state.on_tool_dispatched(
            &source_call("implementation", DecisionEvidenceKindV1::Implementation),
            1,
        );
        state.on_tool_finished_with_source_target(
            "implementation",
            "codebase_memory_get_code_snippet",
            &output_with_evidence(
                ROOT,
                DecisionAnchorLineageStageV1::CarryForward,
                DecisionEvidenceKindV1::Implementation,
            ),
            Some(&graph_target),
        );

        if focused_test_already_retained {
            state.on_tool_dispatched(&call("trace", "codebase_memory_trace_path"), 2);
            finish(
                &mut state,
                "trace",
                "codebase_memory_trace_path",
                ROOT,
                DecisionAnchorLineageStageV1::CarryForward,
            );
            state.on_tool_dispatched(
                &source_call("focused", DecisionEvidenceKindV1::FocusedTest),
                3,
            );
            state.on_tool_finished_with_source_target(
                "focused",
                "codebase_memory_get_code_snippet",
                &output_with_evidence(
                    ROOT,
                    DecisionAnchorLineageStageV1::CarryForward,
                    DecisionEvidenceKindV1::FocusedTest,
                ),
                Some(&graph_target),
            );
        }

        let (failed_call, failed_name) = if focused_test_already_retained {
            (
                source_call("missing-caller", DecisionEvidenceKindV1::Caller),
                "codebase_memory_get_code_snippet",
            )
        } else {
            (
                call("missing-trace", "codebase_memory_trace_path"),
                "codebase_memory_trace_path",
            )
        };
        state.on_tool_dispatched(&failed_call, 4);
        assert_eq!(
            state.on_tool_finished(
                &failed_call.id,
                failed_name,
                &failure_output("provider_protocol"),
            ),
            DecisionAnchorTransition::ProviderUnavailableFallback,
        );

        let mutation = call("partial-mutation", "write");
        assert_eq!(
            state.on_tool_dispatched_with_targets(
                &mutation,
                5,
                Some(&mutation_targets(vec![fallback_target.clone()])),
            ),
            Some(ToolCallDenial::DecisionAnchorMutation),
            "partial graph evidence cannot authorize an independent fallback target",
        );

        let fallback_read = call("fallback-read", "read");
        state.on_tool_dispatched_with_targets(
            &fallback_read,
            6,
            Some(&read_target(TARGET_B)),
        );
        state.on_tool_finished("fallback-read", "read", &successful_read());
        assert_eq!(
            state.on_tool_dispatched_with_targets(
                &mutation,
                7,
                Some(&mutation_targets(vec![fallback_target.clone()])),
            ),
            None,
            "a fresh conventional exact read authorizes its matching target without graph source authority",
        );
    }
}

#[test]
fn successful_targeted_result_without_lineage_cannot_authorize_mutation() {
    let mut state = DecisionAnchorState::from_effects(&effects()).unwrap();
    let correlation = GraphCorrelationV1::new(
        GraphCorrelationToolV1::SearchGraph,
        GraphCorrelationTargetKindV1::GraphQuery,
        "oversized provider result",
    )
    .unwrap();
    let output_without_lineage = ToolOutput {
        content: Vec::new(),
        details: Some(serde_json::json!({
            SAFE_GRAPH_CORRELATION_DETAIL_KEY: correlation,
        })),
        is_error: false,
    };
    let target = TargetAdmissionOutcome::Eligible(exact_target(TARGET_A));

    state.on_tool_dispatched(&call("unretained-root", "codebase_memory_search_graph"), 0);
    assert_eq!(
        state.on_tool_finished(
            "unretained-root",
            "codebase_memory_search_graph",
            &output_without_lineage,
        ),
        DecisionAnchorTransition::Unchanged,
    );
    assert_eq!(
        state.on_tool_dispatched_with_targets(
            &call("unretained-mutation", "write"),
            1,
            Some(&mutation_targets(vec![target.clone()])),
        ),
        Some(ToolCallDenial::DecisionAnchorMutation),
    );

    state.on_tool_dispatched(&call("second-unretained", "codebase_memory_search_graph"), 2);
    assert_eq!(
        state.on_tool_finished(
            "second-unretained",
            "codebase_memory_search_graph",
            &output_without_lineage,
        ),
        DecisionAnchorTransition::EnabledEvidenceIncomplete,
        "bounded unretained targeted results stop without a product",
    );
}

struct ExactTargetResolver {
    target: EligibleWorkspaceTarget,
}

impl LineageAdmissionResolver for ExactTargetResolver {
    fn resolve(&self, tool_name: &str, _: &serde_json::Value) -> LineageAdmissionOutcome {
        match GraphCorrelationToolV1::from_public_name(tool_name) {
            Some(GraphCorrelationToolV1::SearchGraph) => {
                LineageAdmissionOutcome::Ineligible(LineageAdmissionStatus::BroadSelector)
            }
            Some(GraphCorrelationToolV1::TracePath) => LineageAdmissionOutcome::Eligible(
                EligibleLineageAdmission::implementation_caller_traversal(
                    ROOT.to_string(),
                    DecisionAnchorTargetKindV1::FunctionName,
                )
                .unwrap(),
            ),
            Some(GraphCorrelationToolV1::GetCodeSnippet) => LineageAdmissionOutcome::Eligible(
                EligibleLineageAdmission::new(
                    ROOT.to_string(),
                    DecisionAnchorTargetKindV1::QualifiedName,
                    GraphCorrelationToolV1::GetCodeSnippet,
                    Some(DecisionEvidenceKindV1::Implementation),
                )
                .unwrap(),
            ),
            _ => LineageAdmissionOutcome::Ineligible(LineageAdmissionStatus::UnsupportedTool),
        }
    }

    fn resolve_source_target(&self, _: &DecisionAnchorLineageV1) -> TargetAdmissionOutcome {
        TargetAdmissionOutcome::Eligible(self.target.clone())
    }

    fn resolve_invocation_targets(
        &self,
        tool_name: &str,
        _: &serde_json::Value,
    ) -> InvocationTargetAdmission {
        match tool_name {
            "read" => InvocationTargetAdmission::Read(TargetAdmissionOutcome::Eligible(
                self.target.clone(),
            )),
            "write" | "apply_patch" => InvocationTargetAdmission::Mutation(vec![
                TargetAdmissionOutcome::Eligible(self.target.clone()),
            ]),
            "bash" => InvocationTargetAdmission::SourceNeutralProcess,
            "submit_for_pr" => InvocationTargetAdmission::ControlPlane,
            _ => InvocationTargetAdmission::Ineligible(TargetAdmissionStatus::UnsupportedTool),
        }
    }
}

#[test]
fn exact_read_matching_patch_validation_and_submission_serialize_successfully() {
    let resolver = Arc::new(ExactTargetResolver {
        target: exact_target(TARGET_A),
    });
    let mut effects = effects();
    effects.insert("apply_patch".to_string(), ToolEffects::write());
    effects.insert("bash".to_string(), ToolEffects::process());
    effects.insert("submit_for_pr".to_string(), ToolEffects::process());
    let mut machine = AgentMachine::with_effects(vec![user("repair")], 10, effects)
        .with_lineage_admission(resolver);
    let _ = machine.on_start(EngineTime::ZERO);

    for (id, name, output) in [
        (
            "root",
            "codebase_memory_search_graph",
            output(
                "codebase_memory_search_graph",
                ROOT,
                DecisionAnchorLineageStageV1::Root,
            ),
        ),
        (
            "implementation",
            "codebase_memory_get_code_snippet",
            output_with_evidence(
                ROOT,
                DecisionAnchorLineageStageV1::CarryForward,
                DecisionEvidenceKindV1::Implementation,
            ),
        ),
        (
            "trace",
            "codebase_memory_trace_path",
            output(
                "codebase_memory_trace_path",
                ROOT,
                DecisionAnchorLineageStageV1::CarryForward,
            ),
        ),
        (
            "caller",
            "codebase_memory_get_code_snippet",
            output_with_evidence(
                ROOT,
                DecisionAnchorLineageStageV1::CarryForward,
                DecisionEvidenceKindV1::Caller,
            ),
        ),
        (
            "focused",
            "codebase_memory_get_code_snippet",
            output_with_evidence(
                ROOT,
                DecisionAnchorLineageStageV1::CarryForward,
                DecisionEvidenceKindV1::FocusedTest,
            ),
        ),
    ] {
        let dispatched = complete(
            &mut machine,
            llm_responded(assistant_tool_calls(&[(id, name)])),
        );
        assert_eq!(run_tools(&dispatched), [id]);
        let _ = complete(&mut machine, tool_finished(id, output));
    }

    let read_batch = complete(
        &mut machine,
        llm_responded(assistant_tool_calls(&[
            ("post-source-read", "read"),
            ("matching-patch", "apply_patch"),
        ])),
    );
    assert_eq!(run_tools(&read_batch), ["post-source-read"]);
    let mutation_batch = complete(
        &mut machine,
        tool_finished("post-source-read", successful_read()),
    );
    assert!(mutation_batch.iter().any(|request| {
        matches!(
            request,
            AgentRequest::RunTool {
                call,
                denial: None,
                ..
            } if call.id == "matching-patch"
        )
    }));

    let after_patch = complete(
        &mut machine,
        tool_finished("matching-patch", successful_read()),
    );
    assert_eq!(calls_llm(&after_patch), 1);
    let validation = complete(
        &mut machine,
        llm_responded(assistant_tool_calls(&[
            ("validation", "bash"),
            ("submission", "submit_for_pr"),
        ])),
    );
    assert_eq!(run_tools(&validation), ["validation"]);
    let submission = complete(
        &mut machine,
        tool_finished("validation", successful_read()),
    );
    assert_eq!(run_tools(&submission), ["submission"]);
    let after_submission = complete(
        &mut machine,
        tool_finished("submission", successful_read()),
    );
    assert_eq!(calls_llm(&after_submission), 1);
}

include!("decision_anchor_root_pivot.rs");
