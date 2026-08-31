#[test]
fn active_root_handoff_selects_the_first_admission_valid_provider_candidate() {
    use std::sync::Arc;

    use temper_agent_core::LineageAdmissionResolver;
    use temper_protocol_activity::{
        DecisionEvidenceKindV1, GraphRecoveryActionV1, GraphRecoveryEvidenceKindV1,
        GraphRecoveryReferenceDispositionV1,
    };

    fn projected_results(prefix: &str) -> serde_json::Value {
        serde_json::json!({
            "total": 13,
            "has_more": false,
            "results": (1..=13)
                .map(|rank| serde_json::json!({
                    "name": format!("{prefix}_{rank}"),
                    "qualified_name": format!("crate::{prefix}::{prefix}_{rank}"),
                    "label": "Function",
                    "file_path": "src/route.rs",
                }))
                .collect::<Vec<_>>()
        })
    }

    fn implementation_references(guidance: &str) -> Vec<String> {
        guidance
            .split([',', '.', ' ', ']'])
            .filter_map(|part| part.split_once('='))
            .filter(|(label, value)| {
                label.starts_with("implementation_candidate")
                    && value.starts_with("temper-recovery-selector:")
            })
            .map(|(_, value)| value.to_string())
            .collect()
    }

    fn source_input(reference: &str) -> serde_json::Value {
        serde_json::json!({
            "qualified_name": reference,
            "decision_evidence_kind": "implementation",
        })
    }

    let workspace = tempfile::tempdir().unwrap();
    let context = crate::codebase_memory::tests::test_support::workspace_context(
        workspace.path(),
        &[("acme", "demo", "demo")],
    );
    let scope = Arc::new(
        crate::codebase_memory::scope::WorkspaceScope::from_context(&context, workspace.path())
            .unwrap(),
    );
    let registry = DecisionAnchorLineageRegistry::new(scope);

    let active = registry
        .record_with_evidence_kind(
            &correlation(GraphCorrelationTargetKindV1::GraphQuery),
            &serde_json::json!({"query": "active routing implementation"}),
            Some(&structured_parts(projected_results("active"))),
            None,
        )
        .expect("valid projected active root");
    let sibling = registry
        .record_with_evidence_kind(
            &correlation(GraphCorrelationTargetKindV1::GraphQuery),
            &serde_json::json!({"query": "sibling routing implementation"}),
            Some(&structured_parts(projected_results("sibling"))),
            None,
        )
        .expect("valid projected sibling root");
    let exhausted = registry
        .record_with_evidence_kind(
            &correlation(GraphCorrelationTargetKindV1::GraphQuery),
            &serde_json::json!({"query": "selectorless routing implementation"}),
            Some(&structured_parts(projected_results("selectorless"))),
            None,
        )
        .expect("valid projected selectorless root");

    let active_references = implementation_references(
        &registry
            .recovery_selector_guidance(&active)
            .expect("active projected candidates"),
    );
    let sibling_references = implementation_references(
        &registry
            .recovery_selector_guidance(&sibling)
            .expect("sibling projected candidates"),
    );
    assert_eq!(active_references.len(), 4);
    assert_eq!(sibling_references.len(), 4);

    let action = GraphRecoveryActionV1::for_evidence(GraphRecoveryEvidenceKindV1::Implementation);
    let sibling_selector = registry
        .active_root_recovery_selector(&sibling.root_binding, action)
        .expect("rank 1 is actionable before a provider-classified implementation exists");
    assert_eq!(
        sibling_selector.as_public_selector(),
        sibling_references[0]
    );
    let (sibling_outcome, sibling_disposition) = registry
        .resolve_for_active_root_with_recovery(
            action.tool.public_name(),
            &source_input(sibling_selector.as_public_selector()),
            Some(&sibling.root_binding),
        );
    assert!(matches!(sibling_outcome, LineageAdmissionOutcome::Eligible(_)));
    assert_eq!(
        sibling_disposition,
        Some(GraphRecoveryReferenceDispositionV1::Recognized)
    );

    let classified = registry
        .record_with_evidence_kind(
            &correlation(GraphCorrelationTargetKindV1::QualifiedName),
            &serde_json::json!({"qualified_name": "crate::active::active_4"}),
            Some(&structured_parts(serde_json::json!({
                "name": "active_4",
                "qualified_name": "crate::active::active_4",
                "source": "fn active_4() {}",
            }))),
            Some(DecisionEvidenceKindV1::Implementation),
        )
        .expect("rank 4 becomes the provider-classified implementation result");
    assert_eq!(classified.root_binding, active.root_binding);
    assert_eq!(
        classified.decision_evidence_kind,
        Some(DecisionEvidenceKindV1::Implementation)
    );

    let (distractor, distractor_disposition) = registry.resolve_for_active_root_with_recovery(
        action.tool.public_name(),
        &source_input(&active_references[0]),
        Some(&active.root_binding),
    );
    assert_eq!(
        distractor,
        LineageAdmissionOutcome::Ineligible(LineageAdmissionStatus::IncapableSelection)
    );
    assert_eq!(
        distractor_disposition,
        Some(GraphRecoveryReferenceDispositionV1::Rejected)
    );

    let selected = registry
        .active_root_recovery_selector(&active.root_binding, action)
        .expect("one admission-valid active-root implementation remains");
    assert_eq!(selected.as_public_selector(), active_references[3]);
    assert!(!active_references[..3].contains(&selected.as_public_selector().to_string()));

    let exact_input = source_input(selected.as_public_selector());
    let (selected_outcome, selected_disposition) = registry
        .resolve_for_active_root_with_recovery(
            action.tool.public_name(),
            &exact_input,
            Some(&active.root_binding),
        );
    let LineageAdmissionOutcome::Eligible(selected_admission) = selected_outcome else {
        panic!("the advertised exact call must be admitted");
    };
    assert!(selected_admission.matches_root(&active.root_binding));
    assert_eq!(
        selected_admission.evidence_purpose(),
        Some(DecisionEvidenceKindV1::Implementation)
    );
    assert_eq!(
        selected_disposition,
        Some(GraphRecoveryReferenceDispositionV1::Recognized)
    );

    let mut provider_input = exact_input;
    registry
        .expand_recovery_selector(
            action.tool.public_name(),
            &mut provider_input,
            Some(DecisionEvidenceKindV1::Implementation),
        )
        .unwrap()
        .expect("the admitted opaque handoff expands at provider dispatch");
    assert_eq!(provider_input["qualified_name"], "crate::active::active_4");
    let advanced = registry
        .record_with_evidence_kind(
            &correlation(GraphCorrelationTargetKindV1::QualifiedName),
            &provider_input,
            Some(&structured_parts(serde_json::json!({
                "name": "active_4",
                "qualified_name": "crate::active::active_4",
                "source": "fn active_4() {}",
            }))),
            Some(DecisionEvidenceKindV1::Implementation),
        )
        .expect("the exact admitted handoff advances implementation evidence");
    assert_eq!(advanced.root_binding, active.root_binding);
    assert_eq!(
        advanced.decision_evidence_kind,
        Some(DecisionEvidenceKindV1::Implementation)
    );

    registry
        .record_with_evidence_kind(
            &correlation(GraphCorrelationTargetKindV1::QualifiedName),
            &serde_json::json!({"qualified_name": "crate::selectorless::selectorless_5"}),
            Some(&structured_parts(serde_json::json!({
                "name": "selectorless_5",
                "qualified_name": "crate::selectorless::selectorless_5",
                "source": "fn selectorless_5() {}",
            }))),
            Some(DecisionEvidenceKindV1::Implementation),
        )
        .expect("an implementation outside the retained candidate window is classified");
    assert!(
        registry
            .active_root_recovery_selector(&exhausted.root_binding, action)
            .is_none(),
        "no selector-complete handoff is emitted when every retained candidate is incapable",
    );
}

