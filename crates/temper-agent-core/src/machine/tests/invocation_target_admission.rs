#[test]
fn ordinary_targets_reach_the_resolver_only_after_alias_canonicalization() {
    let admission = Arc::new(CountingAdmission::default());
    let mut canonical_machine =
        machine(catalog(&["read"])).with_lineage_admission(admission.clone());
    let _ = canonical_machine.on_start(EngineTime::ZERO);
    let requests = complete(
        &mut canonical_machine,
        llm_responded(assistant(
            "anthropic-messages",
            vec![(
                "read-call",
                "Read",
                serde_json::json!({"file_path":"src/lib.rs"}),
            )],
        )),
    );
    let (call, rejection) = dispatched(&requests);
    assert!(rejection.is_none());
    assert_eq!(call.name, "read");
    assert_eq!(call.arguments, serde_json::json!({"path":"src/lib.rs"}));
    assert_eq!(admission.targets.load(Ordering::SeqCst), 1);
    assert_eq!(
        *admission.canonical_targets.lock().unwrap(),
        vec![("read".to_string(), serde_json::json!({"path":"src/lib.rs"}))]
    );

    let edit_admission = Arc::new(CountingAdmission::default());
    let mut edit_machine =
        machine(catalog(&["edit"])).with_lineage_admission(edit_admission.clone());
    let _ = edit_machine.on_start(EngineTime::ZERO);
    let _ = complete(
        &mut edit_machine,
        llm_responded(assistant(
            "anthropic-messages",
            vec![(
                "edit-call",
                "Edit",
                serde_json::json!({
                    "file_path":"src/lib.rs",
                    "old_string":"old",
                    "new_string":"new"
                }),
            )],
        )),
    );
    assert_eq!(
        *edit_admission.canonical_targets.lock().unwrap(),
        vec![(
            "edit".to_string(),
            serde_json::json!({
                "path":"src/lib.rs",
                "edits":[{"oldText":"old", "newText":"new"}]
            })
        )]
    );

    let rejected = Arc::new(CountingAdmission::default());
    let mut machine = machine(catalog(&["read"])).with_lineage_admission(rejected.clone());
    let _ = machine.on_start(EngineTime::ZERO);
    let _ = complete(
        &mut machine,
        llm_responded(assistant(
            "anthropic-messages",
            vec![(
                "competing",
                "Read",
                serde_json::json!({"path":"a", "file_path":"b"}),
            )],
        )),
    );
    assert_eq!(rejected.targets.load(Ordering::SeqCst), 0);
}

#[test]
fn typed_source_target_is_resolved_only_after_wrapper_completion() {
    use crate::{SAFE_DECISION_ANCHOR_LINEAGE_DETAIL_KEY, SAFE_GRAPH_CORRELATION_DETAIL_KEY};
    use temper_protocol_activity::{
        DecisionAnchorLineageStageV1, DecisionAnchorLineageV1, DecisionAnchorTargetKindV1,
        DecisionEvidenceKindV1, GraphCorrelationTargetKindV1, GraphCorrelationToolV1,
        GraphCorrelationV1,
    };

    let admission = Arc::new(CountingAdmission::default());
    let mut machine = machine(catalog(&["codebase_memory_get_code_snippet"]))
        .with_lineage_admission(admission.clone());
    let _ = machine.on_start(EngineTime::ZERO);
    let _ = complete(
        &mut machine,
        llm_responded(assistant(
            "openai-responses",
            vec![(
                "source",
                "codebase_memory_get_code_snippet",
                serde_json::json!({
                    "qualified_name":"crate::engine::run",
                    "decision_evidence_kind":"implementation"
                }),
            )],
        )),
    );
    assert_eq!(admission.sources.load(Ordering::SeqCst), 0);
    let correlation = GraphCorrelationV1::new(
        GraphCorrelationToolV1::GetCodeSnippet,
        GraphCorrelationTargetKindV1::QualifiedName,
        "crate::engine::run",
    )
    .unwrap();
    let lineage = DecisionAnchorLineageV1::new_with_decision_evidence_kind(
        "00000000-0000-4000-8000-000000000001".to_string(),
        DecisionAnchorLineageStageV1::CarryForward,
        DecisionAnchorTargetKindV1::QualifiedName,
        [DecisionAnchorTargetKindV1::QualifiedName],
        DecisionEvidenceKindV1::Implementation,
    )
    .unwrap();
    let _ = complete(
        &mut machine,
        tool_finished(
            "source",
            ToolOutput {
                content: Vec::new(),
                details: Some(serde_json::json!({
                    SAFE_GRAPH_CORRELATION_DETAIL_KEY: correlation,
                    SAFE_DECISION_ANCHOR_LINEAGE_DETAIL_KEY: lineage
                })),
                is_error: false,
            },
        ),
    );
    assert_eq!(admission.sources.load(Ordering::SeqCst), 1);
}

