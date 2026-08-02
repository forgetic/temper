use super::*;

fn fixture(name: &str) -> String {
    std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join(name),
    )
    .expect("fixture is readable")
}

#[test]
fn golden_bundles_round_trip() {
    for name in ["complete.json", "diagnostics-truncation.json"] {
        let json = fixture(name);
        let bundle: ArtifactContextBundle = serde_json::from_str(&json).expect("fixture parses");
        assert!(is_supported_artifact_context_version(bundle.version));
        let round_trip = serde_json::to_value(&bundle).expect("bundle serializes");
        let golden: serde_json::Value = serde_json::from_str(&json).expect("valid json");
        assert_eq!(round_trip, golden, "fixture {name} is canonical");
    }
}

#[test]
fn artifact_context_version_support_is_exact() {
    assert!(is_supported_artifact_context_version(
        ARTIFACT_CONTEXT_VERSION
    ));
    assert!(!is_supported_artifact_context_version(
        ARTIFACT_CONTEXT_VERSION + 1
    ));
}

#[test]
fn complete_fixture_has_explicit_primary_and_scopes() {
    let bundle: ArtifactContextBundle =
        serde_json::from_str(&fixture("complete.json")).expect("fixture parses");
    assert_eq!(bundle.primary.artifact.number, 295);
    assert_eq!(bundle.primary.workflow_kind.as_deref(), Some("code"));
    let workflow = bundle
        .primary
        .workflow
        .as_ref()
        .expect("workflow projection");
    assert_eq!(workflow.kind.as_deref(), Some("code"));
    assert_eq!(workflow.parents[0].repository_id, "repo-1");
    assert_eq!(workflow.parents[0].number, 277);
    assert_eq!(workflow.dependencies[0].repository_id, "repo-2");
    assert_eq!(workflow.target_branch.as_deref(), Some("main"));
    assert_eq!(
        workflow.correlation_key.as_deref(),
        Some("context-for-code-295")
    );
    assert_eq!(workflow.children[0].number, 296);
    assert_eq!(workflow.children[0].state.as_deref(), Some("open"));
    assert_eq!(workflow.children[1].state, None);
    assert_eq!(
        bundle
            .lineage
            .iter()
            .map(|snapshot| snapshot.workflow_kind.as_deref())
            .collect::<Vec<_>>(),
        [Some("feature"), Some("plan")]
    );
    assert_eq!(bundle.validation_scope[0].labels, ["implementation"]);
    assert_eq!(bundle.validation_scope[0].source, bundle.primary.artifact);
    assert_eq!(
        bundle.optional_references[0].relation_type,
        ArtifactRelationType::Related
    );
}

#[test]
fn legacy_snapshot_omits_absent_workflow_projection_canonically() {
    let legacy = serde_json::json!({
        "artifact": {
            "repository": {"id": "repo-1", "path": "ai/temper"},
            "artifact_type": "issue",
            "number": 7
        },
        "title": "Legacy snapshot",
        "body": "No compact projection yet.",
        "state": "open",
        "workflow_kind": "code"
    });

    let snapshot: ArtifactSnapshot = serde_json::from_value(legacy.clone()).unwrap();
    assert!(snapshot.workflow.is_none());
    assert_eq!(serde_json::to_value(snapshot).unwrap(), legacy);
}

#[test]
fn sparse_workflow_projection_defaults_collections_and_omits_them() {
    let json = serde_json::json!({
        "artifact": {
            "repository": {"id": "repo-1", "path": "ai/temper"},
            "artifact_type": "issue",
            "number": 7
        },
        "title": "Sparse projection",
        "body": "Authored body.",
        "state": "open",
        "workflow": {"kind": "code"}
    });

    let snapshot: ArtifactSnapshot = serde_json::from_value(json.clone()).unwrap();
    let workflow = snapshot.workflow.as_ref().unwrap();
    assert!(workflow.parents.is_empty());
    assert!(workflow.dependencies.is_empty());
    assert!(workflow.children.is_empty());
    assert_eq!(serde_json::to_value(snapshot).unwrap(), json);
}

#[test]
fn workflow_projection_serializes_only_compact_allowed_fields() {
    let bundle: ArtifactContextBundle =
        serde_json::from_str(&fixture("complete.json")).expect("fixture parses");
    let workflow = serde_json::to_value(bundle.primary.workflow.unwrap()).unwrap();
    let fields = workflow
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>();
    assert_eq!(
        fields,
        BTreeSet::from([
            "children".to_string(),
            "correlation_key".to_string(),
            "dependencies".to_string(),
            "kind".to_string(),
            "parents".to_string(),
            "target_branch".to_string(),
        ])
    );
    for relation in ["parents", "dependencies"] {
        let fields = workflow[relation][0]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        assert_eq!(
            fields,
            BTreeSet::from(["number".to_string(), "repository_id".to_string()])
        );
    }
    let child_fields = workflow["children"][0]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>();
    assert_eq!(
        child_fields,
        BTreeSet::from([
            "number".to_string(),
            "repository_id".to_string(),
            "state".to_string(),
            "title".to_string(),
        ])
    );

    let serialized = serde_json::to_string(&workflow).unwrap();
    for forbidden in [
        "lease",
        "assignment",
        "create_issue_intents",
        "body_hex",
        "completion",
        "staging",
        "wired",
        "body",
        "workflow",
    ] {
        assert!(
            !serialized.contains(&format!("\"{forbidden}\":")),
            "unexpected field {forbidden}"
        );
    }

    for forbidden in [
        serde_json::json!({"body_hex": "00"}),
        serde_json::json!({"lease": {"worker": "worker-1"}}),
        serde_json::json!({"children": [{
            "repository_id": "repo-1",
            "number": 8,
            "title": "child",
            "body": "nested child source body"
        }]}),
    ] {
        assert!(
            serde_json::from_value::<ArtifactWorkflowContext>(forbidden).is_err(),
            "workflow projection must reject bookkeeping fields"
        );
    }
}

#[test]
fn diagnostic_codes_are_stable_snake_case() {
    let codes = [
        ArtifactContextDiagnosticCode::MissingArtifact,
        ArtifactContextDiagnosticCode::ClosedAncestor,
        ArtifactContextDiagnosticCode::MalformedMetadata,
        ArtifactContextDiagnosticCode::RepositoryNotAllowed,
        ArtifactContextDiagnosticCode::CycleDetected,
        ArtifactContextDiagnosticCode::DepthExceeded,
        ArtifactContextDiagnosticCode::CountExceeded,
        ArtifactContextDiagnosticCode::ContentTruncated,
        ArtifactContextDiagnosticCode::ForgeReadFailed,
    ];
    let names: Vec<String> = codes
        .into_iter()
        .map(|code| {
            serde_json::to_value(code)
                .unwrap()
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();
    assert_eq!(
        names,
        [
            "missing_artifact",
            "closed_ancestor",
            "malformed_metadata",
            "repository_not_allowed",
            "cycle_detected",
            "depth_exceeded",
            "count_exceeded",
            "content_truncated",
            "forge_read_failed",
        ]
    );
}

#[test]
fn forge_operations_use_closed_snake_case_shapes() {
    let operation = ForgeContextOperation::ForgeListRelated(ForgeListRelatedOperation {
        repo: "ai/temper".into(),
        number: 7,
        artifact_type: Some(ArtifactType::Issue),
        relations: vec![ForgeRelationType::Child, ForgeRelationType::ProducedPr],
        depth: Some(2),
        limit: Some(50),
    });
    let json = serde_json::to_value(&operation).unwrap();
    assert_eq!(json["operation"], "forge_list_related");
    assert_eq!(json["repo"], "ai/temper");
    assert_eq!(json["type"], "issue");
    assert_eq!(json["relations"][1], "produced_pr");
    assert_eq!(
        serde_json::from_value::<ForgeContextOperation>(json).unwrap(),
        operation
    );
}

#[test]
fn w3c_trace_context_validation_is_strict_and_bounded() {
    let context = W3cTraceContext {
        traceparent: "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01".into(),
        tracestate: Some("vendor=value,other=opaque".into()),
    };
    context.validate().unwrap();
    assert_eq!(
        serde_json::from_value::<W3cTraceContext>(serde_json::to_value(&context).unwrap()).unwrap(),
        context
    );

    for invalid in [
        "00-00000000000000000000000000000000-00f067aa0ba902b7-01",
        "00-4BF92F3577B34DA6A3CE929D0E0E4736-00f067aa0ba902b7-01",
        "ff-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
    ] {
        let invalid = W3cTraceContext {
            traceparent: invalid.into(),
            tracestate: None,
        };
        assert!(invalid.validate().is_err());
    }
}

#[test]
fn w3c_tracestate_rejects_control_characters_and_unbounded_values() {
    for tracestate in [
        "vendor=ok\nsecret=value".to_string(),
        "x".repeat(513),
        "Vendor=value".to_string(),
        "1vendor=value".to_string(),
        "vendor=has=equals".to_string(),
        "vendor=first,vendor=duplicate".to_string(),
        "vendor;bad=value".to_string(),
    ] {
        let invalid = W3cTraceContext {
            traceparent: "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01".into(),
            tracestate: Some(tracestate),
        };
        assert!(invalid.validate().is_err());
    }

    W3cTraceContext {
        traceparent: "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01".into(),
        tracestate: Some("1tenant@vendor=value,other=opaque".into()),
    }
    .validate()
    .unwrap();
}

#[test]
fn stable_context_error_vocabulary_is_snake_case() {
    let errors = [
        ForgeContextErrorCode::InvalidRequest,
        ForgeContextErrorCode::NotAuthorized,
        ForgeContextErrorCode::NotFound,
        ForgeContextErrorCode::ForgeUnavailable,
        ForgeContextErrorCode::LimitExceeded,
    ];
    let values: Vec<_> = errors
        .into_iter()
        .map(|error| serde_json::to_value(error).unwrap())
        .collect();
    assert_eq!(
        values,
        [
            "invalid_request",
            "not_authorized",
            "not_found",
            "forge_unavailable",
            "limit_exceeded",
        ]
    );
}
