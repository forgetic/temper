use super::*;

#[test]
fn exact_selectors_resolve_before_provider_with_only_closed_values() {
    const PRIVATE_SELECTOR: &str = "crate::private::engine::run";
    let mut lineages = DecisionAnchorLineages::default();
    let root = lineages
        .record(
            &correlation(GraphCorrelationTargetKindV1::GraphQuery),
            &serde_json::json!({"query": "private discovery query"}),
            Some(&structured_parts(serde_json::json!({
                "qualified_name": PRIVATE_SELECTOR
            }))),
        )
        .unwrap();

    let implementation = lineages.resolve(
        GraphCorrelationToolV1::GetCodeSnippet.public_name(),
        &serde_json::json!({
            "qualified_name": PRIVATE_SELECTOR,
            "decision_evidence_kind": "implementation"
        }),
    );
    assert!(matches!(
        implementation,
        LineageAdmissionOutcome::Eligible(_)
    ));
    lineages
        .record_with_evidence_kind(
            &correlation(GraphCorrelationTargetKindV1::QualifiedName),
            &serde_json::json!({"qualified_name": PRIVATE_SELECTOR}),
            Some(&structured_parts(serde_json::json!({
                "qualified_name": PRIVATE_SELECTOR
            }))),
            Some(temper_protocol_activity::DecisionEvidenceKindV1::Implementation),
        )
        .unwrap();
    for (input, expected) in [
        (
            serde_json::json!({}),
            LineageAdmissionStatus::MalformedSelector,
        ),
        (
            serde_json::json!({"function_name": ""}),
            LineageAdmissionStatus::MalformedSelector,
        ),
        (
            serde_json::json!({"qualified_name": PRIVATE_SELECTOR}),
            LineageAdmissionStatus::IncapableSelection,
        ),
        (
            serde_json::json!({"function_name": "not_returned"}),
            LineageAdmissionStatus::UnknownSelector,
        ),
    ] {
        assert_eq!(
            lineages.resolve(GraphCorrelationToolV1::TracePath.public_name(), &input),
            LineageAdmissionOutcome::Ineligible(expected),
        );
    }
    let admission = lineages.resolve(
        GraphCorrelationToolV1::TracePath.public_name(),
        &serde_json::json!({"function_name": "run"}),
    );
    let LineageAdmissionOutcome::Eligible(admission) = admission else {
        panic!("exact registered selector must be eligible");
    };
    assert!(admission.matches_root(&root.root_binding));
    assert_eq!(
        admission.selector_kind(),
        DecisionAnchorTargetKindV1::FunctionName
    );
    assert_eq!(admission.tool_kind(), GraphCorrelationToolV1::TracePath);
    assert_eq!(admission.evidence_purpose(), None);
    let rendered = format!("{admission:?}");
    assert!(!rendered.contains(PRIVATE_SELECTOR));
    assert!(!rendered.contains(&root.root_binding));
}

#[test]
fn partial_implementation_snapshot_stays_closed_when_recheck_does_not_enrich() {
    let mut lineages = DecisionAnchorLineages::default();
    let root = lineages
        .record(
            &correlation(GraphCorrelationTargetKindV1::GraphQuery),
            &serde_json::json!({"query": "partial implementation"}),
            Some(&structured_parts(serde_json::json!({
                "qualified_name": "crate::engine::run"
            }))),
        )
        .unwrap();
    lineages
        .record_with_evidence_kind(
            &correlation(GraphCorrelationTargetKindV1::QualifiedName),
            &serde_json::json!({"qualified_name": "crate::engine::run"}),
            Some(&structured_parts(serde_json::json!({
                "qualified_name": "crate::engine::run",
                "callers": 1
            }))),
            Some(temper_protocol_activity::DecisionEvidenceKindV1::Implementation),
        )
        .unwrap();

    let guidance = lineages
        .recovery_selector_guidance(&root.root_binding)
        .expect("partial implementation retains a bounded recovery reference");
    let reference = guidance
        .split_once("implementation_evidence_result=")
        .and_then(|(_, rest)| rest.split([',', '.']).next())
        .expect("implementation recovery reference");
    let trace = serde_json::json!({"function_name": reference, "direction": "inbound"});
    assert_eq!(
        lineages.resolve(GraphCorrelationToolV1::TracePath.public_name(), &trace),
        LineageAdmissionOutcome::Ineligible(LineageAdmissionStatus::TraversalNotReady),
    );

    let source = serde_json::json!({
        "qualified_name": reference,
        "decision_evidence_kind": "implementation"
    });
    let LineageAdmissionOutcome::Eligible(recheck) =
        lineages.resolve(GraphCorrelationToolV1::GetCodeSnippet.public_name(), &source)
    else {
        panic!("one exact typed implementation recheck must be admitted");
    };
    assert!(recheck.is_traversal_readiness_recheck());
    lineages
        .record_with_evidence_kind(
            &correlation(GraphCorrelationTargetKindV1::QualifiedName),
            &serde_json::json!({"qualified_name": "crate::engine::run"}),
            Some(&structured_parts(serde_json::json!({
                "qualified_name": "crate::engine::run",
                "callers": 1
            }))),
            Some(temper_protocol_activity::DecisionEvidenceKindV1::Implementation),
        )
        .unwrap();

    let refreshed_guidance = lineages
        .recovery_selector_guidance(&root.root_binding)
        .expect("partial recheck replaces the recovery reference");
    let refreshed_reference = refreshed_guidance
        .split_once("implementation_evidence_result=")
        .and_then(|(_, rest)| rest.split([',', '.']).next())
        .expect("refreshed implementation recovery reference");
    let refreshed_trace =
        serde_json::json!({"function_name": refreshed_reference, "direction": "inbound"});
    let refreshed_source = serde_json::json!({
        "qualified_name": refreshed_reference,
        "decision_evidence_kind": "implementation"
    });
    assert_eq!(
        lineages.resolve(GraphCorrelationToolV1::TracePath.public_name(), &trace),
        LineageAdmissionOutcome::Ineligible(LineageAdmissionStatus::MalformedSelector),
        "the pre-recheck reference is stale",
    );
    for _ in 0..3 {
        assert_eq!(
            lineages.resolve(
                GraphCorrelationToolV1::TracePath.public_name(),
                &refreshed_trace,
            ),
            LineageAdmissionOutcome::Ineligible(
                LineageAdmissionStatus::TraversalReadinessExhausted
            ),
            "elapsed model turns and repeated partial results cannot authorize traversal",
        );
    }
    assert_eq!(
        lineages.resolve(
            GraphCorrelationToolV1::GetCodeSnippet.public_name(),
            &refreshed_source,
        ),
        LineageAdmissionOutcome::Ineligible(LineageAdmissionStatus::IncapableSelection),
        "the provider recheck is bounded to one exact attempt",
    );
}

#[test]
fn partial_implementation_becomes_ready_only_after_new_caller_identity_evidence() {
    let mut lineages = DecisionAnchorLineages::default();
    let root = lineages
        .record(
            &correlation(GraphCorrelationTargetKindV1::GraphQuery),
            &serde_json::json!({"query": "partial implementation"}),
            Some(&structured_parts(serde_json::json!({
                "qualified_name": "crate::engine::run"
            }))),
        )
        .unwrap();
    lineages
        .record_with_evidence_kind(
            &correlation(GraphCorrelationTargetKindV1::QualifiedName),
            &serde_json::json!({"qualified_name": "crate::engine::run"}),
            Some(&structured_parts(serde_json::json!({
                "qualified_name": "crate::engine::run",
                "callers": 1
            }))),
            Some(temper_protocol_activity::DecisionEvidenceKindV1::Implementation),
        )
        .unwrap();
    let guidance = lineages
        .recovery_selector_guidance(&root.root_binding)
        .expect("partial implementation recovery guidance");
    let reference = guidance
        .split_once("implementation_evidence_result=")
        .and_then(|(_, rest)| rest.split([',', '.']).next())
        .expect("implementation recovery reference");
    let trace = serde_json::json!({"function_name": reference, "direction": "inbound"});
    assert_eq!(
        lineages.resolve(GraphCorrelationToolV1::TracePath.public_name(), &trace),
        LineageAdmissionOutcome::Ineligible(LineageAdmissionStatus::TraversalNotReady),
    );
    assert!(matches!(
        lineages.resolve(
            GraphCorrelationToolV1::GetCodeSnippet.public_name(),
            &serde_json::json!({
                "qualified_name": reference,
                "decision_evidence_kind": "implementation"
            }),
        ),
        LineageAdmissionOutcome::Eligible(_)
    ));
    lineages
        .record_with_evidence_kind(
            &correlation(GraphCorrelationTargetKindV1::QualifiedName),
            &serde_json::json!({"qualified_name": "crate::engine::run"}),
            Some(&structured_parts(serde_json::json!({
                "qualified_name": "crate::engine::run",
                "callers": 1,
                "caller_names": ["dispatch"]
            }))),
            Some(temper_protocol_activity::DecisionEvidenceKindV1::Implementation),
        )
        .unwrap();
    assert_eq!(
        lineages.resolve(GraphCorrelationToolV1::TracePath.public_name(), &trace),
        LineageAdmissionOutcome::Ineligible(LineageAdmissionStatus::MalformedSelector),
        "the earlier reference becomes stale when fresh implementation evidence replaces it",
    );
    let refreshed_guidance = lineages
        .recovery_selector_guidance(&root.root_binding)
        .expect("fresh implementation recovery guidance");
    let refreshed_reference = refreshed_guidance
        .split_once("implementation_evidence_result=")
        .and_then(|(_, rest)| rest.split([',', '.']).next())
        .expect("fresh implementation recovery reference");
    let refreshed_trace =
        serde_json::json!({"function_name": refreshed_reference, "direction": "inbound"});
    let LineageAdmissionOutcome::Eligible(admission) = lineages.resolve(
        GraphCorrelationToolV1::TracePath.public_name(),
        &refreshed_trace,
    ) else {
        panic!("new eligible caller identity evidence must authorize traversal");
    };
    assert!(admission.matches_root(&root.root_binding));
}

#[test]
fn implementation_snapshot_with_caller_names_is_immediately_traversal_ready() {
    let mut lineages = DecisionAnchorLineages::default();
    lineages
        .record(
            &correlation(GraphCorrelationTargetKindV1::GraphQuery),
            &serde_json::json!({"query": "ready implementation"}),
            Some(&structured_parts(serde_json::json!({
                "qualified_name": "crate::engine::run"
            }))),
        )
        .unwrap();
    lineages
        .record_with_evidence_kind(
            &correlation(GraphCorrelationTargetKindV1::QualifiedName),
            &serde_json::json!({"qualified_name": "crate::engine::run"}),
            Some(&structured_parts(serde_json::json!({
                "qualified_name": "crate::engine::run",
                "callers": 1,
                "caller_names": ["dispatch"]
            }))),
            Some(temper_protocol_activity::DecisionEvidenceKindV1::Implementation),
        )
        .unwrap();
    assert!(matches!(
        lineages.resolve(
            GraphCorrelationToolV1::TracePath.public_name(),
            &serde_json::json!({"function_name": "run", "direction": "inbound"}),
        ),
        LineageAdmissionOutcome::Eligible(_)
    ));
}

#[test]
fn opaque_recovery_reference_resolves_and_expands_without_exposing_selector() {
    const PRIVATE_SELECTOR: &str = "temper-v1-private.src.engine.run";
    const FUNCTION_SELECTOR: &str = "run";
    let mut lineages = DecisionAnchorLineages::default();
    let root = lineages
        .record(
            &correlation(GraphCorrelationTargetKindV1::GraphQuery),
            &serde_json::json!({"query": "private discovery query"}),
            Some(&structured_parts(serde_json::json!({
                "qualified_name": PRIVATE_SELECTOR
            }))),
        )
        .unwrap();
    lineages
        .record_with_evidence_kind(
            &correlation(GraphCorrelationTargetKindV1::QualifiedName),
            &serde_json::json!({"qualified_name": PRIVATE_SELECTOR}),
            Some(&structured_parts(serde_json::json!({
                "qualified_name": PRIVATE_SELECTOR,
                "name": FUNCTION_SELECTOR
            }))),
            Some(temper_protocol_activity::DecisionEvidenceKindV1::Implementation),
        )
        .unwrap();

    let guidance = lineages
        .recovery_selector_guidance(&root.root_binding)
        .expect("implementation result exposes an opaque trace selector reference");
    assert!(!guidance.contains(PRIVATE_SELECTOR));
    assert!(!guidance.contains(&root.root_binding));
    assert!(
        guidance.len()
            <= crate::codebase_memory::result_presentation::RECOVERY_SELECTOR_GUIDANCE_RESERVE_BYTES
    );
    let reference = guidance
        .split_once("implementation_evidence_result=")
        .and_then(|(_, rest)| rest.split([',', '.']).next())
        .expect("closed implementation trace reference");
    assert!(reference.starts_with("temper-recovery-selector:"));

    let mut source_form = serde_json::json!({
        "qualified_name": reference,
        "decision_evidence_kind": "implementation"
    });
    assert!(
        lineages
            .expand_recovery_selector(
                GraphCorrelationToolV1::GetCodeSnippet.public_name(),
                &mut source_form,
                Some(temper_protocol_activity::DecisionEvidenceKindV1::Implementation),
            )
            .unwrap()
            .is_some()
    );
    assert_eq!(source_form["qualified_name"], PRIVATE_SELECTOR);

    let mut trace_input = serde_json::json!({"function_name": reference});
    let admission = lineages
        .reserve_implementation_trace_reference(&trace_input, Some(&root.root_binding))
        .expect("well-formed recovery reference")
        .expect("current-root reference is recognized before selector readiness");
    assert!(admission.matches_root(&root.root_binding));
    assert!(
        lineages
            .expand_recovery_selector(
                GraphCorrelationToolV1::TracePath.public_name(),
                &mut trace_input,
                None,
            )
            .unwrap()
            .is_some()
    );
    assert_eq!(trace_input["function_name"], PRIVATE_SELECTOR);

    let repeated = serde_json::json!({"function_name": reference});
    assert!(
        lineages
            .reserve_implementation_trace_reference(&repeated, Some(&root.root_binding))
            .is_err(),
        "a consumed reference cannot authorize a second dispatch",
    );

    let unknown_reference = format!("{reference}-inexact");
    let mut unknown_input = serde_json::json!({"function_name": unknown_reference});
    assert_eq!(
        lineages.resolve(
            GraphCorrelationToolV1::TracePath.public_name(),
            &unknown_input,
        ),
        LineageAdmissionOutcome::Ineligible(LineageAdmissionStatus::MalformedSelector),
    );
    assert!(
        lineages
            .expand_recovery_selector(
                GraphCorrelationToolV1::TracePath.public_name(),
                &mut unknown_input,
                None,
            )
            .is_err()
    );

    let mut source_input = serde_json::json!({
        "qualified_name": reference,
        "decision_evidence_kind": "implementation"
    });
    assert!(
        lineages
            .expand_recovery_selector(
                GraphCorrelationToolV1::GetCodeSnippet.public_name(),
                &mut source_input,
                Some(temper_protocol_activity::DecisionEvidenceKindV1::Implementation),
            )
            .is_err(),
        "a consumed trace reference is stale for every later purpose",
    );

    let mut wrong_purpose = serde_json::json!({"qualified_name": reference});
    assert!(
        lineages
            .expand_recovery_selector(
                GraphCorrelationToolV1::GetCodeSnippet.public_name(),
                &mut wrong_purpose,
                Some(temper_protocol_activity::DecisionEvidenceKindV1::Caller),
            )
            .is_err(),
        "a trace reference cannot be substituted for a caller-source reference",
    );
}

#[test]
fn opaque_trace_reference_is_current_root_bound_and_superseded_fail_closed() {
    const PRIVATE_SELECTOR: &str = "temper-v1-private.src.engine.run";
    let mut lineages = DecisionAnchorLineages::default();
    let root = lineages
        .record(
            &correlation(GraphCorrelationTargetKindV1::GraphQuery),
            &serde_json::json!({"query": "private discovery query"}),
            Some(&structured_parts(serde_json::json!({
                "qualified_name": PRIVATE_SELECTOR,
                "name": "run"
            }))),
        )
        .unwrap();
    let record_implementation = |lineages: &mut DecisionAnchorLineages| {
        lineages
            .record_with_evidence_kind(
                &correlation(GraphCorrelationTargetKindV1::QualifiedName),
                &serde_json::json!({"qualified_name": PRIVATE_SELECTOR}),
                Some(&structured_parts(serde_json::json!({
                    "qualified_name": PRIVATE_SELECTOR,
                    "name": "run",
                    "callers": 1
                }))),
                Some(temper_protocol_activity::DecisionEvidenceKindV1::Implementation),
            )
            .unwrap();
    };
    record_implementation(&mut lineages);
    let first_guidance = lineages
        .recovery_selector_guidance(&root.root_binding)
        .expect("first reference");
    let first = first_guidance
        .split_once("implementation_evidence_result=")
        .and_then(|(_, rest)| rest.split([',', '.']).next())
        .unwrap()
        .to_string();

    assert!(
        lineages
            .reserve_implementation_trace_reference(
                &serde_json::json!({"function_name": first}),
                Some("00000000-0000-4000-8000-000000000099"),
            )
            .is_err(),
        "a reference cannot cross its run-local root",
    );

    record_implementation(&mut lineages);
    let second_guidance = lineages
        .recovery_selector_guidance(&root.root_binding)
        .expect("replacement reference");
    let second = second_guidance
        .split_once("implementation_evidence_result=")
        .and_then(|(_, rest)| rest.split([',', '.']).next())
        .unwrap();
    assert_ne!(first, second);
    assert!(
        lineages
            .reserve_implementation_trace_reference(
                &serde_json::json!({"function_name": first}),
                Some(&root.root_binding),
            )
            .is_err(),
        "a superseded reference must be stale",
    );
    assert!(
        lineages
            .reserve_implementation_trace_reference(
                &serde_json::json!({"function_name": second}),
                Some(&root.root_binding),
            )
            .unwrap()
            .is_some(),
        "the current reference remains admissible",
    );
}

#[test]
fn source_admission_requires_one_validated_wrapper_evidence_purpose() {
    let mut lineages = DecisionAnchorLineages::default();
    let root = lineages
        .record(
            &correlation(GraphCorrelationTargetKindV1::GraphQuery),
            &serde_json::json!({"query": "root"}),
            Some(&structured_parts(serde_json::json!({
                "qualified_name": "crate::engine::run"
            }))),
        )
        .unwrap();
    let source = GraphCorrelationToolV1::GetCodeSnippet.public_name();

    assert_eq!(
        lineages.resolve(
            source,
            &serde_json::json!({"qualified_name": "crate::engine::run"})
        ),
        LineageAdmissionOutcome::Ineligible(LineageAdmissionStatus::IncapableSelection)
    );
    assert_eq!(
        lineages.resolve(
            source,
            &serde_json::json!({
                "qualified_name": "crate::engine::run",
                "decision_evidence_kind": "provider prose"
            })
        ),
        LineageAdmissionOutcome::Ineligible(LineageAdmissionStatus::MalformedSelector)
    );

    let denied_caller = lineages.resolve(
        source,
        &serde_json::json!({
            "qualified_name": "crate::engine::run",
            "decision_evidence_kind": "caller"
        }),
    );
    assert_eq!(
        denied_caller,
        LineageAdmissionOutcome::Ineligible(LineageAdmissionStatus::IncapableSelection),
        "a root candidate is not caller evidence before implementation traversal"
    );
    let admitted = lineages.resolve(
        source,
        &serde_json::json!({
            "qualified_name": "crate::engine::run",
            "decision_evidence_kind": "implementation"
        }),
    );
    let LineageAdmissionOutcome::Eligible(admitted) = admitted else {
        panic!("validated implementation purpose must be eligible");
    };
    assert!(admitted.matches_root(&root.root_binding));
    assert_eq!(
        admitted.evidence_purpose(),
        Some(temper_protocol_activity::DecisionEvidenceKindV1::Implementation)
    );
}
