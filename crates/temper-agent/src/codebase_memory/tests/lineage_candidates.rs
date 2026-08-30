#[test]
fn oversized_search_graph_projects_provider_order_and_admits_the_exact_source_chain() {
    let mut primary = [
        "zeta_worker",
        "yankee_worker",
        "xray_worker",
        "select_worker",
        "whiskey_worker",
        "victor_worker",
        "uniform_worker",
        "tango_worker",
        "sierra_worker",
        "romeo_worker",
        "quebec_worker",
        "papa_worker",
    ]
    .into_iter()
    .map(|name| {
        serde_json::json!({
            "name": name,
            "qualified_name": format!("crate::route::{name}"),
            "label": "Function",
            "file_path": "src/route.rs"
        })
    })
    .collect::<Vec<_>>();
    primary.push(serde_json::json!({
        "name": "keeps_worker_affinity",
        "qualified_name": "crate::tests::keeps_worker_affinity",
        "label": "Function",
        "file_path": "tests/worker_affinity.rs",
        "is_test": true
    }));
    let broad_result = serde_json::json!({
        "total": 15,
        "has_more": false,
        "results": primary,
        "semantic_results": [
            {
                "name": "aaa_lexical_first",
                "qualified_name": "crate::route::aaa_lexical_first",
                "label": "Function",
                "file_path": "src/route.rs"
            },
            {
                "name": "another_semantic_result",
                "qualified_name": "crate::route::another_semantic_result",
                "label": "Function",
                "file_path": "src/route.rs"
            }
        ]
    });

    let mut lineages = DecisionAnchorLineages::default();
    let root = lineages
        .record(
            &correlation(GraphCorrelationTargetKindV1::GraphQuery),
            &serde_json::json!({"query": "routing implementation and focused regression"}),
            Some(&structured_parts(broad_result)),
        )
        .expect("an oversized valid provider result yields a bounded root");
    assert_eq!(
        root.focused_test_discovery,
        Some(temper_protocol_activity::FocusedTestDiscoveryOutcomeV1::EligibleSelectorReturned)
    );
    assert!(!root.result_target_kinds.is_empty());

    assert_eq!(
        lineages.resolve_for_active_root(
            GraphCorrelationToolV1::GetCodeSnippet.public_name(),
            &serde_json::json!({
                "qualified_name": "crate::route::select_worker",
                "decision_evidence_kind": "implementation"
            }),
            Some(&root.root_binding),
        ),
        LineageAdmissionOutcome::Ineligible(LineageAdmissionStatus::IncapableSelection),
        "a projected broad root is not direct source authority",
    );
    assert_eq!(
        lineages.resolve_for_active_root(
            GraphCorrelationToolV1::SearchGraph.public_name(),
            &serde_json::json!({
                "name_pattern": "select_worker",
                "label": "Function"
            }),
            Some(&root.root_binding),
        ),
        LineageAdmissionOutcome::Ineligible(LineageAdmissionStatus::UnknownSelector),
        "projected roots expose only their run-local candidate references",
    );

    let guidance = lineages
        .recovery_selector_guidance(&root.root_binding)
        .expect("the projected root exposes bounded continuations");
    let implementation_references = guidance
        .split([',', '.', ' ', ']'])
        .filter_map(|part| part.split_once('='))
        .filter(|(label, value)| {
            label.starts_with("implementation_candidate")
                && value.starts_with("temper-recovery-selector:")
        })
        .map(|(_, value)| value.to_string())
        .collect::<Vec<_>>();
    assert_eq!(implementation_references.len(), 4);
    for private in [
        "zeta_worker",
        "select_worker",
        "aaa_lexical_first",
        "crate::route",
    ] {
        assert!(!guidance.contains(private));
    }

    let mut implementation_input = serde_json::json!({
        "qualified_name": implementation_references[3],
        "decision_evidence_kind": "implementation"
    });
    assert!(matches!(
        lineages.resolve_for_active_root(
            GraphCorrelationToolV1::GetCodeSnippet.public_name(),
            &implementation_input,
            Some(&root.root_binding),
        ),
        LineageAdmissionOutcome::Eligible(_)
    ));
    let expanded_implementation = lineages
        .expand_recovery_selector(
            GraphCorrelationToolV1::GetCodeSnippet.public_name(),
            &mut implementation_input,
            Some(temper_protocol_activity::DecisionEvidenceKindV1::Implementation),
        )
        .unwrap()
        .expect("the fourth provider-ranked implementation is selectable");
    assert_eq!(
        implementation_input["qualified_name"],
        "crate::route::select_worker",
        "provider order, not lexical candidate order, determines the projection",
    );
    let implementation = lineages
        .record_with_evidence_kind(
            &correlation(GraphCorrelationTargetKindV1::QualifiedName),
            &implementation_input,
            Some(&structured_parts(serde_json::json!({
                "name": "select_worker",
                "qualified_name": "crate::route::select_worker",
                "file_path": "src/route.rs",
                "source": "fn select_worker() {}",
                "callers": [{
                    "name": "dispatch",
                    "qualified_name": "crate::delivery::dispatch"
                }]
            }))),
            Some(temper_protocol_activity::DecisionEvidenceKindV1::Implementation),
        )
        .unwrap();
    lineages.complete_candidate_reference(&expanded_implementation, false);
    assert_eq!(implementation.root_binding, root.root_binding);
    assert_eq!(
        implementation.decision_evidence_kind,
        Some(temper_protocol_activity::DecisionEvidenceKindV1::Implementation)
    );

    let trace_input = serde_json::json!({
        "function_name": "select_worker",
        "mode": "calls",
        "direction": "inbound"
    });
    assert!(matches!(
        lineages.resolve_for_active_root(
            GraphCorrelationToolV1::TracePath.public_name(),
            &trace_input,
            Some(&root.root_binding),
        ),
        LineageAdmissionOutcome::Eligible(_)
    ));
    let trace = lineages
        .record(
            &correlation(GraphCorrelationTargetKindV1::FunctionName),
            &trace_input,
            Some(&structured_parts(serde_json::json!({
                "function": {
                    "name": "select_worker",
                    "qualified_name": "crate::route::select_worker"
                },
                "callers": [{
                    "name": "dispatch",
                    "qualified_name": "crate::delivery::dispatch"
                }]
            }))),
        )
        .unwrap();
    assert_eq!(trace.root_binding, root.root_binding);
    assert_eq!(
        trace.caller_discovery,
        Some(temper_protocol_activity::CallerDiscoveryOutcomeV1::EligibleSelectorReturned)
    );

    let caller_input = serde_json::json!({
        "qualified_name": "crate::delivery::dispatch",
        "decision_evidence_kind": "caller"
    });
    assert!(matches!(
        lineages.resolve_for_active_root(
            GraphCorrelationToolV1::GetCodeSnippet.public_name(),
            &caller_input,
            Some(&root.root_binding),
        ),
        LineageAdmissionOutcome::Eligible(_)
    ));
    let caller = lineages
        .record_with_evidence_kind(
            &correlation(GraphCorrelationTargetKindV1::QualifiedName),
            &caller_input,
            Some(&structured_parts(serde_json::json!({
                "name": "dispatch",
                "qualified_name": "crate::delivery::dispatch",
                "file_path": "src/delivery.rs",
                "source": "fn dispatch() {}"
            }))),
            Some(temper_protocol_activity::DecisionEvidenceKindV1::Caller),
        )
        .unwrap();
    assert_eq!(caller.root_binding, root.root_binding);
    assert_eq!(
        caller.decision_evidence_kind,
        Some(temper_protocol_activity::DecisionEvidenceKindV1::Caller)
    );

    let focused_guidance = lineages
        .recovery_selector_guidance(&root.root_binding)
        .expect("the focused-test candidate remains on the same root");
    let focused_reference = focused_guidance
        .split([',', '.', ' ', ']'])
        .filter_map(|part| part.split_once('='))
        .find(|(label, value)| {
            label.starts_with("focused_test_result")
                && value.starts_with("temper-recovery-selector:")
        })
        .map(|(_, value)| value.to_string())
        .expect("focused-test opaque reference");
    let mut focused_input = serde_json::json!({
        "qualified_name": focused_reference,
        "decision_evidence_kind": "focused_test"
    });
    assert!(matches!(
        lineages.resolve_for_active_root(
            GraphCorrelationToolV1::GetCodeSnippet.public_name(),
            &focused_input,
            Some(&root.root_binding),
        ),
        LineageAdmissionOutcome::Eligible(_)
    ));
    lineages
        .expand_recovery_selector(
            GraphCorrelationToolV1::GetCodeSnippet.public_name(),
            &mut focused_input,
            Some(temper_protocol_activity::DecisionEvidenceKindV1::FocusedTest),
        )
        .unwrap()
        .expect("focused-test reference expands only for provider dispatch");
    assert_eq!(focused_input["qualified_name"], "keeps_worker_affinity");
    let focused = lineages
        .record_with_evidence_kind(
            &correlation(GraphCorrelationTargetKindV1::QualifiedName),
            &focused_input,
            Some(&structured_parts(serde_json::json!({
                "name": "keeps_worker_affinity",
                "qualified_name": "crate::tests::keeps_worker_affinity",
                "file_path": "tests/worker_affinity.rs",
                "source": "fn keeps_worker_affinity() {}",
                "is_test": true
            }))),
            Some(temper_protocol_activity::DecisionEvidenceKindV1::FocusedTest),
        )
        .unwrap();
    assert_eq!(focused.root_binding, root.root_binding);
    assert_eq!(
        focused.decision_evidence_kind,
        Some(temper_protocol_activity::DecisionEvidenceKindV1::FocusedTest)
    );

    let durable = serde_json::to_string(&root).unwrap();
    for private in implementation_references
        .iter()
        .chain(std::iter::once(&focused_reference))
        .map(String::as_str)
        .chain([
            "select_worker",
            "keeps_worker_affinity",
            "src/route.rs",
            "tests/worker_affinity.rs",
        ])
    {
        assert!(!durable.contains(private));
    }
}

#[test]
fn oversized_malformed_projection_remains_non_authoritative() {
    let mut results = (0..13)
        .map(|index| {
            serde_json::json!({
                "name": format!("worker_{index}"),
                "qualified_name": format!("crate::route::worker_{index}"),
                "label": "Function"
            })
        })
        .collect::<Vec<_>>();
    results.push(serde_json::json!({
        "name": "conflicting_name",
        "qualified_name": "crate::route::different_name",
        "label": "Function"
    }));

    let mut lineages = DecisionAnchorLineages::default();
    let root = lineages
        .record(
            &correlation(GraphCorrelationTargetKindV1::GraphQuery),
            &serde_json::json!({"query": "malformed broad response"}),
            Some(&structured_parts(serde_json::json!({"results": results}))),
        )
        .unwrap();
    assert!(root.result_target_kinds.is_empty());
    assert!(
        lineages
            .recovery_selector_guidance(&root.root_binding)
            .is_none()
    );
    assert_eq!(
        lineages.resolve_for_active_root(
            GraphCorrelationToolV1::GetCodeSnippet.public_name(),
            &serde_json::json!({
                "qualified_name": "crate::route::worker_0",
                "decision_evidence_kind": "implementation"
            }),
            Some(&root.root_binding),
        ),
        LineageAdmissionOutcome::Ineligible(LineageAdmissionStatus::UnknownSelector),
    );
}

#[test]
fn multi_result_root_retains_bounded_same_root_candidates_and_selects_a_later_one() {
    let mut lineages = DecisionAnchorLineages::default();
    let root = lineages
        .record(
            &correlation(GraphCorrelationTargetKindV1::GraphQuery),
            &serde_json::json!({"query": "routing implementation"}),
            Some(&structured_parts(serde_json::json!({
                "results": [
                    {"qualified_name": "crate::route::alpha_worker"},
                    {"qualified_name": "crate::route::beta_worker"}
                ]
            }))),
        )
        .unwrap();

    let guidance = lineages
        .recovery_selector_guidance(&root.root_binding)
        .expect("a multi-result root retains exact candidate continuations");
    let references = guidance
        .split([',', '.', ' ', ']'])
        .filter_map(|part| part.split_once('=').map(|(_, value)| value))
        .filter(|value| value.starts_with("temper-recovery-selector:"))
        .collect::<Vec<_>>();
    assert_eq!(references.len(), 2);
    assert!(guidance.contains("implementation_candidate_1="));
    assert!(guidance.contains("implementation_candidate_2="));
    assert!(!guidance.contains("alpha_worker"));
    assert!(!guidance.contains("beta_worker"));
    assert!(!guidance.contains(&root.root_binding));

    let mut selected = serde_json::json!({
        "qualified_name": references[1],
        "decision_evidence_kind": "implementation"
    });
    let outcome = lineages.resolve_for_active_root(
        GraphCorrelationToolV1::GetCodeSnippet.public_name(),
        &selected,
        Some(&root.root_binding),
    );
    let LineageAdmissionOutcome::Eligible(admission) = outcome else {
        panic!("a later candidate must remain selectable on its producing root: {outcome:?}");
    };
    assert!(admission.matches_root(&root.root_binding));
    let expanded = lineages
        .expand_recovery_selector(
            GraphCorrelationToolV1::GetCodeSnippet.public_name(),
            &mut selected,
            Some(temper_protocol_activity::DecisionEvidenceKindV1::Implementation),
        )
        .unwrap()
        .expect("the opaque candidate expands only at provider dispatch");
    assert_eq!(selected["qualified_name"], "crate::route::beta_worker");

    let implementation = lineages
        .record_with_evidence_kind(
            &correlation(GraphCorrelationTargetKindV1::QualifiedName),
            &selected,
            Some(&structured_parts(serde_json::json!({
                "qualified_name": "crate::route::beta_worker",
                "callers": [{"qualified_name": "crate::delivery::dispatch"}]
            }))),
            Some(temper_protocol_activity::DecisionEvidenceKindV1::Implementation),
        )
        .unwrap();
    lineages.complete_candidate_reference(&expanded, false);
    assert_eq!(implementation.root_binding, root.root_binding);
    assert_eq!(
        implementation.stage,
        DecisionAnchorLineageStageV1::CarryForward
    );
    assert_eq!(
        implementation.decision_evidence_kind,
        Some(temper_protocol_activity::DecisionEvidenceKindV1::Implementation)
    );
}

#[test]
fn durable_lineage_serialization_excludes_transient_candidate_state() {
    const RAW_SELECTOR: &str = "PRIVATE raw routing selector";
    const LOCAL_PATH: &str = "/private/worktree/src/route.rs";
    const STRUCTURED_PROVIDER_VALUE: &str = "PRIVATE structured provider payload";
    const FIRST_CANDIDATE: &str = "crate::private::alpha_worker";
    const SECOND_CANDIDATE: &str = "crate::private::beta_worker";

    let mut lineages = DecisionAnchorLineages::default();
    let lineage = lineages
        .record(
            &correlation(GraphCorrelationTargetKindV1::GraphQuery),
            &serde_json::json!({"query": RAW_SELECTOR}),
            Some(&structured_parts(serde_json::json!({
                "results": [
                    {
                        "qualified_name": FIRST_CANDIDATE,
                        "path": LOCAL_PATH,
                        "source": STRUCTURED_PROVIDER_VALUE
                    },
                    {"qualified_name": SECOND_CANDIDATE}
                ]
            }))),
        )
        .expect("typed provider result creates lineage");
    let recovery_guidance = lineages
        .recovery_selector_guidance(&lineage.root_binding)
        .expect("candidate identities produce opaque recovery references");
    let opaque_references = recovery_guidance
        .split([',', '.', ' ', ']'])
        .filter_map(|part| part.split_once('=').map(|(_, value)| value))
        .filter(|value| value.starts_with("temper-recovery-selector:"))
        .collect::<Vec<_>>();
    assert_eq!(opaque_references.len(), 2);

    let durable = serde_json::to_string(&lineage).expect("lineage serializes");
    for private_value in [
        RAW_SELECTOR,
        LOCAL_PATH,
        STRUCTURED_PROVIDER_VALUE,
        FIRST_CANDIDATE,
        SECOND_CANDIDATE,
    ]
    .into_iter()
    .chain(opaque_references)
    {
        assert!(
            !durable.contains(private_value),
            "durable lineage retained transient value {private_value:?}"
        );
    }
}

#[test]
fn candidate_miss_keeps_only_root_local_alternatives_without_authority() {
    let mut lineages = DecisionAnchorLineages::default();
    let root = lineages
        .record(
            &correlation(GraphCorrelationTargetKindV1::GraphQuery),
            &serde_json::json!({"query": "routing implementation"}),
            Some(&structured_parts(serde_json::json!({
                "results": [
                    {"qualified_name": "crate::route::alpha_worker"},
                    {"qualified_name": "crate::route::beta_worker"}
                ]
            }))),
        )
        .unwrap();
    let guidance = lineages
        .recovery_selector_guidance(&root.root_binding)
        .unwrap();
    let references = guidance
        .split([',', '.', ' ', ']'])
        .filter_map(|part| part.split_once('=').map(|(_, value)| value.to_string()))
        .filter(|value| value.starts_with("temper-recovery-selector:"))
        .collect::<Vec<_>>();

    let expand = |lineages: &mut DecisionAnchorLineages, reference: &str| {
        let mut input = serde_json::json!({
            "qualified_name": reference,
            "decision_evidence_kind": "implementation"
        });
        lineages
            .expand_recovery_selector(
                GraphCorrelationToolV1::GetCodeSnippet.public_name(),
                &mut input,
                Some(temper_protocol_activity::DecisionEvidenceKindV1::Implementation),
            )
            .unwrap()
            .unwrap()
    };
    let first = expand(&mut lineages, &references[0]);
    let retry = lineages
        .complete_candidate_reference(&first, true)
        .expect("root candidate failure is closed typed state");
    assert!(retry.has_alternative);
    let retry_guidance = retry.guidance.expect("one root-local alternative remains");
    assert!(!retry_guidance.contains(&references[0]));
    assert!(retry_guidance.contains(&references[1]));

    let second = expand(&mut lineages, &references[1]);
    let exhausted = lineages
        .complete_candidate_reference(&second, true)
        .expect("last candidate failure is closed typed state");
    assert!(!exhausted.has_alternative);
    assert!(exhausted.guidance.is_none());
}

#[test]
fn focused_test_candidate_uses_an_explicit_provider_source_name() {
    let mut lineages = DecisionAnchorLineages::default();
    let root = lineages
        .record(
            &correlation(GraphCorrelationTargetKindV1::GraphQuery),
            &serde_json::json!({"query": "focused behavior"}),
            Some(&structured_parts(serde_json::json!({
                "results": [{
                    "name": "keeps_affinity",
                    "qualified_name": "crate::tests::keeps_affinity",
                    "is_test": true
                }]
            }))),
        )
        .unwrap();
    let guidance = lineages
        .recovery_selector_guidance(&root.root_binding)
        .unwrap();
    assert!(!guidance.contains("implementation_candidate"));
    let reference = guidance
        .split_once("focused_test_result=")
        .and_then(|(_, value)| value.split([',', ' ', ']', '.']).next())
        .unwrap();
    let mut input = serde_json::json!({
        "qualified_name": reference,
        "decision_evidence_kind": "focused_test"
    });
    assert!(matches!(
        lineages.resolve_for_active_root(
            GraphCorrelationToolV1::GetCodeSnippet.public_name(),
            &input,
            Some(&root.root_binding),
        ),
        LineageAdmissionOutcome::Eligible(_)
    ));
    lineages
        .expand_recovery_selector(
            GraphCorrelationToolV1::GetCodeSnippet.public_name(),
            &mut input,
            Some(temper_protocol_activity::DecisionEvidenceKindV1::FocusedTest),
        )
        .unwrap()
        .unwrap();
    assert_eq!(input["qualified_name"], "keeps_affinity");
}
