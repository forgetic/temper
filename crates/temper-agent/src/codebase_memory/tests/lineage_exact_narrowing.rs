use super::*;

#[test]
fn exact_graph_narrowing_selects_the_active_root_and_promotes_its_source() {
    const IMPLEMENTATION: &str = "temper-v1-private.src.route.worker_slot";
    let mut lineages = DecisionAnchorLineages::default();
    let result = || {
        structured_parts(serde_json::json!({
            "results": [{
                "name": "worker_slot",
                "qualified_name": IMPLEMENTATION,
                "label": "Function"
            }]
        }))
    };
    let first = lineages
        .record(
            &correlation(GraphCorrelationTargetKindV1::GraphQuery),
            &serde_json::json!({"query": "route affinity"}),
            Some(&result()),
        )
        .unwrap();
    let second = lineages
        .record(
            &correlation(GraphCorrelationTargetKindV1::GraphQuery),
            &serde_json::json!({"query": "narrow route affinity"}),
            Some(&result()),
        )
        .unwrap();
    assert_ne!(first.root_binding, second.root_binding);

    let exact_input = serde_json::json!({
        "name_pattern": "worker_slot",
        "label": "Function"
    });
    let admission = lineages.resolve_for_active_root(
        GraphCorrelationToolV1::SearchGraph.public_name(),
        &exact_input,
        Some(&first.root_binding),
    );
    let LineageAdmissionOutcome::Eligible(admission) = admission else {
        panic!("the copied exact identity and type must select the active root");
    };
    assert!(admission.matches_root(&first.root_binding));
    assert_eq!(
        admission.selector_kind(),
        DecisionAnchorTargetKindV1::NamePattern
    );

    let narrowed = lineages
        .record(
            &correlation(GraphCorrelationTargetKindV1::NamePattern),
            &exact_input,
            Some(&result()),
        )
        .unwrap();
    assert_eq!(narrowed.stage, DecisionAnchorLineageStageV1::CarryForward);
    assert_eq!(narrowed.root_binding, first.root_binding);
    assert!(matches!(
        lineages.resolve(
            GraphCorrelationToolV1::GetCodeSnippet.public_name(),
            &serde_json::json!({
                "qualified_name": IMPLEMENTATION,
                "decision_evidence_kind": "implementation"
            }),
        ),
        LineageAdmissionOutcome::Eligible(_)
    ));
}

#[test]
fn exact_graph_narrowing_survives_an_unconsumable_broad_root() {
    const IMPLEMENTATION: &str = "temper-v1-private.src.route.worker_slot";
    let result = |count: usize| {
        let mut results = vec![serde_json::json!({
            "qualified_name": IMPLEMENTATION,
            "label": "Function"
        })];
        results.extend((1..count).map(|index| {
            serde_json::json!({
                "qualified_name": format!("crate.module_{index}.candidate_{index}"),
                "label": "Function"
            })
        }));
        structured_parts(serde_json::json!({"results": results}))
    };
    let mut lineages = DecisionAnchorLineages::default();
    let broad = lineages
        .record(
            &correlation(GraphCorrelationTargetKindV1::GraphQuery),
            &serde_json::json!({"query": "route affinity"}),
            Some(&result(14)),
        )
        .unwrap();
    assert!(broad.result_target_kinds.is_empty());
    let narrowed = lineages
        .record(
            &correlation(GraphCorrelationTargetKindV1::GraphQuery),
            &serde_json::json!({"query": "worker route affinity"}),
            Some(&result(10)),
        )
        .unwrap();
    assert!(!narrowed.result_target_kinds.is_empty());

    let input = serde_json::json!({"name_pattern": "worker_slot", "label": "Function"});
    assert!(matches!(
        lineages.resolve_for_active_root(
            GraphCorrelationToolV1::SearchGraph.public_name(),
            &input,
            Some(&narrowed.root_binding),
        ),
        LineageAdmissionOutcome::Eligible(_)
    ));
    let exact = lineages
        .record(
            &correlation(GraphCorrelationTargetKindV1::NamePattern),
            &input,
            Some(&result(1)),
        )
        .unwrap();
    assert_eq!(exact.stage, DecisionAnchorLineageStageV1::CarryForward);
    assert_eq!(exact.root_binding, narrowed.root_binding);
}

#[test]
fn exact_graph_narrowing_rejects_invented_mismatched_and_ambiguous_selectors() {
    let mut lineages = DecisionAnchorLineages::default();
    let root = lineages
        .record(
            &correlation(GraphCorrelationTargetKindV1::GraphQuery),
            &serde_json::json!({"query": "route affinity"}),
            Some(&structured_parts(serde_json::json!({
                "results": [
                    {
                        "name": "worker_slot",
                        "qualified_name": "crate.route.Primary.worker_slot",
                        "label": "Function"
                    },
                    {
                        "name": "worker_slot",
                        "qualified_name": "crate.route.Secondary.worker_slot",
                        "label": "Function"
                    }
                ]
            }))),
        )
        .unwrap();
    for (input, expected) in [
        (
            serde_json::json!({"name_pattern": "invented", "label": "Function"}),
            LineageAdmissionStatus::UnknownSelector,
        ),
        (
            serde_json::json!({"name_pattern": "worker_slot", "label": "Method"}),
            LineageAdmissionStatus::UnknownSelector,
        ),
        (
            serde_json::json!({"name_pattern": "worker_slot", "label": "Function"}),
            LineageAdmissionStatus::AmbiguousSelector,
        ),
    ] {
        assert_eq!(
            lineages.resolve_for_active_root(
                GraphCorrelationToolV1::SearchGraph.public_name(),
                &input,
                Some(&root.root_binding),
            ),
            LineageAdmissionOutcome::Ineligible(expected),
        );
    }
}

#[test]
fn exact_graph_narrowing_requires_one_matching_provider_result() {
    let mut lineages = DecisionAnchorLineages::default();
    let root_parts = structured_parts(serde_json::json!({
        "results": [{
            "name": "worker_slot",
            "qualified_name": "crate.route.worker_slot",
            "label": "Function"
        }]
    }));
    let root = lineages
        .record(
            &correlation(GraphCorrelationTargetKindV1::GraphQuery),
            &serde_json::json!({"query": "route affinity"}),
            Some(&root_parts),
        )
        .unwrap();
    let input = serde_json::json!({"name_pattern": "worker_slot", "label": "Function"});
    assert!(matches!(
        lineages.resolve_for_active_root(
            GraphCorrelationToolV1::SearchGraph.public_name(),
            &input,
            Some(&root.root_binding),
        ),
        LineageAdmissionOutcome::Eligible(_)
    ));
    let mismatched = structured_parts(serde_json::json!({
        "results": [{
            "name": "other_slot",
            "qualified_name": "crate.route.other_slot",
            "label": "Function"
        }]
    }));
    let output = lineages
        .record(
            &correlation(GraphCorrelationTargetKindV1::NamePattern),
            &input,
            Some(&mismatched),
        )
        .unwrap();
    assert_eq!(output.stage, DecisionAnchorLineageStageV1::Root);
    assert_ne!(output.root_binding, root.root_binding);
}

