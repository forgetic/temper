use super::*;

fn create(path: &str) -> String {
    format!(
        "diff --git a/{path} b/{path}\nnew file mode 100644\n--- /dev/null\n+++ b/{path}\n@@ -0,0 +1 @@\n+new\n"
    )
}

fn existing() -> &'static str {
    "diff --git a/existing.txt b/existing.txt\n--- a/existing.txt\n+++ b/existing.txt\n@@ -1 +1 @@\n-old\n+updated\n"
}

#[test]
fn patch_creation_applies_new_subdirectories_and_existing_hunks_together() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("existing.txt"), "old\n").unwrap();
    git_apply(
        root.path(),
        &format!("{}{}", existing(), create("tests/new/case.rs")),
    )
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(root.path().join("existing.txt")).unwrap(),
        "updated\n"
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("tests/new/case.rs")).unwrap(),
        "new\n"
    );
}

#[test]
fn patch_creation_execution_rechecks_after_successful_git_check() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("existing.txt"), "old\n").unwrap();
    let patch = format!("{}{}", existing(), create("new.rs"));
    run_git_apply(root.path(), &patch, true).unwrap();
    std::fs::write(root.path().join("new.rs"), "concurrent\n").unwrap();
    assert!(run_git_apply(root.path(), &patch, false).is_err());
    assert_eq!(
        std::fs::read_to_string(root.path().join("existing.txt")).unwrap(),
        "old\n"
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("new.rs")).unwrap(),
        "concurrent\n"
    );
}

#[test]
fn patch_creation_rejects_an_invalid_sibling_without_partial_writes() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("existing.txt"), "old\n").unwrap();
    let patch = format!(
        "{}{}{}",
        existing(),
        create("valid.rs"),
        create("../escape.rs")
    );
    assert!(git_apply(root.path(), &patch).is_err());
    assert_eq!(
        std::fs::read_to_string(root.path().join("existing.txt")).unwrap(),
        "old\n"
    );
    assert!(!root.path().join("valid.rs").exists());
}

#[cfg(unix)]
#[test]
fn patch_creation_rechecks_symlink_ancestors_before_application() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let patch = create("new/sub/file.rs");
    run_git_apply(root.path(), &patch, true).unwrap();
    std::os::unix::fs::symlink(outside.path(), root.path().join("new")).unwrap();
    assert!(run_git_apply(root.path(), &patch, false).is_err());
    assert!(!outside.path().join("sub/file.rs").exists());
}

#[cfg(unix)]
#[test]
fn patch_creation_rejects_replacement_of_the_prepared_workspace_root() {
    let parent = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let workspace = parent.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let tool = ApplyPatchTool::new(&workspace);
    std::fs::remove_dir(&workspace).unwrap();
    std::os::unix::fs::symlink(outside.path(), &workspace).unwrap();
    let output = temper_agent_io::block_on_with(move |_cx, _handle| async move {
        tool.execute(
            "replaced-root",
            serde_json::json!({"patch": create("new.rs")}),
            None,
        )
        .await
    })
    .unwrap();
    assert!(output.is_error);
    assert!(!outside.path().join("new.rs").exists());
}
