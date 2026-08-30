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
