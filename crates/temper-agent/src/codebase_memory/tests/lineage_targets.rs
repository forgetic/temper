    #[test]
    fn source_reads_and_each_mutation_target_share_only_opaque_run_local_identity() {
        use std::sync::Arc;
        use temper_protocol_activity::DecisionEvidenceKindV1;

        let workspace = tempfile::tempdir().unwrap();
        let context = crate::codebase_memory::tests::test_support::workspace_context(
            workspace.path(),
            &[("acme", "demo", "demo")],
        );
        let source_path = workspace.path().join("demo/src/lib.rs");
        std::fs::create_dir_all(source_path.parent().unwrap()).unwrap();
        std::fs::write(&source_path, "fn run() {}\n").unwrap();
        let other_path = workspace.path().join("demo/src/other.rs");
        std::fs::write(&other_path, "fn other() {}\n").unwrap();
        let scope = Arc::new(
            crate::codebase_memory::scope::WorkspaceScope::from_context(&context, workspace.path())
                .unwrap(),
        );
        let registry = DecisionAnchorLineageRegistry::new(scope);

        registry
            .record_with_evidence_kind(
                &correlation(GraphCorrelationTargetKindV1::GraphQuery),
                &serde_json::json!({"query": "root"}),
                Some(&structured_parts(serde_json::json!({
                    "qualified_name": "crate::engine::run"
                }))),
                None,
            )
            .unwrap();
        let source = registry
            .record_with_evidence_kind(
                &correlation(GraphCorrelationTargetKindV1::QualifiedName),
                &serde_json::json!({"qualified_name": "crate::engine::run"}),
                Some(&structured_parts(serde_json::json!({
                    "qualified_name": "crate::engine::run",
                    "file_path": "src/lib.rs",
                    "source": "PRIVATE SOURCE"
                }))),
                Some(DecisionEvidenceKindV1::Implementation),
            )
            .unwrap();
        let TargetAdmissionOutcome::Eligible(source_target) =
            registry.resolve_source_target(&source)
        else {
            panic!("typed current-root source must resolve");
        };

        for path in [
            "demo/src/lib.rs".to_string(),
            source_path.display().to_string(),
            "demo/src/../src/lib.rs".to_string(),
        ] {
            let InvocationTargetAdmission::Read(TargetAdmissionOutcome::Eligible(read_target)) =
                registry.resolve_invocation_targets(
                    "read",
                    &serde_json::json!({"path": path, "offset": 1}),
                )
            else {
                panic!("canonical read alias must resolve");
            };
            assert!(source_target.matches(&read_target));
        }
        for (tool, arguments) in [
            (
                "write",
                serde_json::json!({"path": "demo/src/lib.rs", "content": "replacement"}),
            ),
            (
                "edit",
                serde_json::json!({
                    "path": "demo/src/lib.rs",
                    "edits": [{"oldText": "run", "newText": "work"}]
                }),
            ),
        ] {
            let InvocationTargetAdmission::Mutation(targets) =
                registry.resolve_invocation_targets(tool, &arguments)
            else {
                panic!("canonical mutation must resolve");
            };
            assert!(
                matches!(targets.as_slice(), [TargetAdmissionOutcome::Eligible(target)] if source_target.matches(target))
            );
        }

        let patch = "diff --git a/demo/src/lib.rs b/demo/src/lib.rs\n--- a/demo/src/lib.rs\n+++ b/demo/src/lib.rs\n@@ -1 +1 @@\n-fn run() {}\n+fn run() { work(); }\ndiff --git a/demo/src/other.rs b/demo/src/other.rs\n--- a/demo/src/other.rs\n+++ b/demo/src/other.rs\n@@ -1 +1 @@\n-fn other() {}\n+fn other() { work(); }\n";
        let InvocationTargetAdmission::Mutation(targets) = registry
            .resolve_invocation_targets("apply_patch", &serde_json::json!({"patch": patch}))
        else {
            panic!("well-formed multi-target patch must expose its target set");
        };
        assert_eq!(targets.len(), 2);
        assert!(
            matches!(&targets[0], TargetAdmissionOutcome::Eligible(target) if source_target.matches(target))
        );
        assert_eq!(
            targets[1],
            TargetAdmissionOutcome::Ineligible(TargetAdmissionStatus::UnknownTarget),
            "an unmatched sibling must not piggyback on the admitted source target"
        );

        let outside = tempfile::NamedTempFile::new().unwrap();
        assert_eq!(
            registry
                .resolve_invocation_targets("read", &serde_json::json!({"path": outside.path()})),
            InvocationTargetAdmission::Read(TargetAdmissionOutcome::Ineligible(
                TargetAdmissionStatus::OutsideWorkspace
            ))
        );
        assert_eq!(
            registry.resolve_invocation_targets(
                "apply_patch",
                &serde_json::json!({"patch": "not a patch"})
            ),
            InvocationTargetAdmission::Ineligible(TargetAdmissionStatus::MalformedTarget)
        );
        assert_eq!(
            registry.resolve_invocation_targets(
                "read",
                &serde_json::json!({"path": ["demo/src/lib.rs"]})
            ),
            InvocationTargetAdmission::Read(TargetAdmissionOutcome::Ineligible(
                TargetAdmissionStatus::MalformedTarget
            ))
        );
        assert_eq!(
            registry.resolve_invocation_targets(
                "apply_patch",
                &serde_json::json!({"patch": "diff --git a/demo/src/lib.rs b/demo/src/lib.rs\n--- a/demo/src/other.rs\n+++ b/demo/src/lib.rs\n"})
            ),
            InvocationTargetAdmission::Ineligible(TargetAdmissionStatus::CompetingTargets)
        );
        assert_eq!(
            registry.resolve_invocation_targets(
                "bash",
                &serde_json::json!({
                    "command": "cd demo && cargo fmt --check && cargo test --quiet && git diff --check && test \"$(git diff --name-only)\" = src/lib.rs && test \"$(git diff --numstat -- src/lib.rs)\" = \"$(printf '1\\t1\\tsrc/lib.rs')\""
                })
            ),
            InvocationTargetAdmission::SourceNeutralProcess
        );
        assert_eq!(
            registry.resolve_invocation_targets(
                "bash",
                &serde_json::json!({"command": "cd demo && rg worker_slot src"})
            ),
            InvocationTargetAdmission::SourceNeutralProcess
        );
        assert_eq!(
            registry.resolve_invocation_targets(
                "submit_for_pr",
                &serde_json::json!({"summary": "validated"})
            ),
            InvocationTargetAdmission::ControlPlane
        );
        for source_mutation in [
            "printf changed > demo/src/lib.rs",
            "sed -i s/run/work/ demo/src/lib.rs",
            "cargo fmt",
            "git checkout -- demo/src/lib.rs",
            "test \"$(touch demo/src/lib.rs)\" = changed",
        ] {
            assert_eq!(
                registry.resolve_invocation_targets(
                    "bash",
                    &serde_json::json!({"command": source_mutation})
                ),
                InvocationTargetAdmission::Ineligible(TargetAdmissionStatus::UnsupportedTool),
                "source-mutating shell command must remain fail-closed",
            );
        }

        let private = serde_json::to_string(&source).unwrap();
        let debug = format!("{source_target:?} {targets:?}");
        let source_display = source_path.display().to_string();
        for forbidden in ["src/lib.rs", "PRIVATE SOURCE", source_display.as_str()] {
            assert!(!private.contains(forbidden));
            assert!(!debug.contains(forbidden));
        }

        let ambiguous = registry
            .record_with_evidence_kind(
                &correlation(GraphCorrelationTargetKindV1::QualifiedName),
                &serde_json::json!({"qualified_name": "crate::engine::run"}),
                Some(&structured_parts(serde_json::json!({
                    "qualified_name": "crate::engine::run",
                    "file_path": "src/lib.rs",
                    "path": "src/other.rs",
                    "source": "PRIVATE SOURCE"
                }))),
                Some(DecisionEvidenceKindV1::Implementation),
            )
            .unwrap();
        assert_eq!(
            registry.resolve_source_target(&ambiguous),
            TargetAdmissionOutcome::Ineligible(TargetAdmissionStatus::AmbiguousTarget)
        );
    }
