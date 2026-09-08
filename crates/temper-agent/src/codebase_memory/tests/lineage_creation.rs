fn creation_registry(root: &std::path::Path) -> DecisionAnchorLineageRegistry {
    let context = crate::codebase_memory::tests::test_support::workspace_context(
        root,
        &[("acme", "demo", "demo")],
    );
    let scope =
        crate::codebase_memory::scope::WorkspaceScope::from_context(&context, root).unwrap();
    DecisionAnchorLineageRegistry::new(std::sync::Arc::new(scope))
}

fn new_file_patch(path: &str) -> String {
    format!(
        "diff --git a/{path} b/{path}\nnew file mode 100644\n--- /dev/null\n+++ b/{path}\n@@ -0,0 +1 @@\n+new content\n"
    )
}

#[test]
fn patch_creation_mints_only_distinct_missing_target_admission() {
    let root = tempfile::tempdir().unwrap();
    let registry = creation_registry(root.path());
    let path = "demo/new/subdirectory/test.rs";
    for tool in ["read", "write", "edit"] {
        let admission =
            registry.resolve_invocation_targets(tool, &serde_json::json!({"path": path}));
        match admission {
            InvocationTargetAdmission::Read(TargetAdmissionOutcome::Ineligible(_)) => {}
            InvocationTargetAdmission::Mutation(targets) => assert!(
                targets
                    .iter()
                    .all(|target| matches!(target, TargetAdmissionOutcome::Ineligible(_)))
            ),
            _ => panic!("an ordinary missing path must remain ineligible"),
        }
    }
    let admission = registry.resolve_invocation_targets(
        "apply_patch",
        &serde_json::json!({"patch": new_file_patch(path)}),
    );
    let InvocationTargetAdmission::PatchCreation {
        existing,
        creations,
    } = admission
    else {
        panic!("explicit absent target must resolve");
    };
    assert!(existing.is_empty());
    assert_eq!(creations.len(), 1);
    assert!(!format!("{creations:?}").contains(path));
    assert!(!root.path().join(path).exists(), "admission is read only");
}

#[test]
fn patch_creation_rejects_existing_conflicting_and_escaping_targets() {
    let root = tempfile::tempdir().unwrap();
    let registry = creation_registry(root.path());
    std::fs::create_dir_all(root.path().join("demo")).unwrap();
    std::fs::write(root.path().join("demo/existing.rs"), "old\n").unwrap();
    for patch in [
        new_file_patch("demo/existing.rs"),
        new_file_patch("../outside.rs"),
        new_file_patch("demo/existing.rs/child.rs"),
        format!(
            "{}{}",
            new_file_patch("demo/new.rs"),
            new_file_patch("demo/new.rs")
        ),
        format!(
            "{}{}",
            new_file_patch("demo/new.rs"),
            new_file_patch("demo/existing.rs")
        ),
    ] {
        assert!(matches!(
            registry
                .resolve_invocation_targets("apply_patch", &serde_json::json!({"patch": patch})),
            InvocationTargetAdmission::Ineligible(_)
        ));
    }
    assert!(!root.path().join("demo/new.rs").exists());
}

#[cfg(unix)]
#[test]
fn patch_creation_rejects_dangling_and_ancestor_symlinks() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let registry = creation_registry(root.path());
    std::fs::create_dir_all(root.path().join("demo")).unwrap();
    std::os::unix::fs::symlink(outside.path(), root.path().join("demo/linked")).unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("missing"),
        root.path().join("demo/dangling"),
    )
    .unwrap();
    for path in ["demo/linked/escape.rs", "demo/dangling"] {
        assert!(matches!(
            registry.resolve_invocation_targets(
                "apply_patch",
                &serde_json::json!({"patch": new_file_patch(path)})
            ),
            InvocationTargetAdmission::Ineligible(_)
        ));
    }
}

#[test]
fn patch_creation_destination_appearing_after_admission_is_rejected_by_real_tool() {
    let root = tempfile::tempdir().unwrap();
    let registry = creation_registry(root.path());
    let path = "demo/new/file.rs";
    let arguments = serde_json::json!({"patch": new_file_patch(path)});
    assert!(matches!(
        registry.resolve_invocation_targets("apply_patch", &arguments),
        InvocationTargetAdmission::PatchCreation { .. }
    ));
    std::fs::create_dir_all(root.path().join("demo/new")).unwrap();
    std::fs::write(root.path().join(path), "concurrent content\n").unwrap();
    let tools = crate::coding_agent::tool_registry(
        crate::coding_agent::Capability::CodingWorkspace,
        root.path(),
    );
    let output = temper_agent_io::block_on_with(move |_cx, _handle| async move {
        let tool = tools
            .tools()
            .iter()
            .find(|tool| tool.name() == "apply_patch")
            .unwrap();
        tool.execute("create", arguments, None).await
    })
    .unwrap();
    assert!(output.is_error);
    assert_eq!(
        std::fs::read_to_string(root.path().join(path)).unwrap(),
        "concurrent content\n"
    );
}
