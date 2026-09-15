use super::*;

fn existing(name: &str, counts: &str, old: &str, new: &str) -> String {
    format!(
        "diff --git a/{name} b/{name}\n--- a/{name}\n+++ b/{name}\n@@ {counts} @@\n-{old}\n+{new}\n"
    )
}

#[test]
fn canonical_patch_applies_all_declared_files_and_new_file() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("a.txt"), "old a\n").unwrap();
    std::fs::write(root.path().join("b.txt"), "old b\n").unwrap();
    let patch = existing("a.txt", "-1 +1,99", "old a", "new a")
        + " "
        + &existing("b.txt", "-1 +1,99", "old b", "new b")
        + " diff --git a/new.txt b/new.txt\n--- /dev/null\n+++ b/new.txt\n@@ -0,0 +1,99 @@\n+created\n";
    git_apply(root.path(), &patch).unwrap();
    for (path, expected) in [
        ("a.txt", "new a\n"),
        ("b.txt", "new b\n"),
        ("new.txt", "created\n"),
    ] {
        assert_eq!(
            std::fs::read_to_string(root.path().join(path)).unwrap(),
            expected
        );
    }
}

#[test]
fn bad_second_context_rejects_every_write_after_recount() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("a.txt"), "old a\n").unwrap();
    std::fs::write(root.path().join("b.txt"), "actual b\n").unwrap();
    let patch = existing("a.txt", "-1 +1,99", "old a", "new a")
        + " "
        + &existing("b.txt", "-1 +1,99", "wrong b", "new b");
    assert!(git_apply(root.path(), &patch).is_err());
    assert_eq!(
        std::fs::read_to_string(root.path().join("a.txt")).unwrap(),
        "old a\n"
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("b.txt")).unwrap(),
        "actual b\n"
    );
}

#[test]
fn hidden_traditional_target_cannot_reach_git_execution() {
    let root = tempfile::tempdir().unwrap();
    for name in ["a.txt", "b.txt"] {
        std::fs::write(root.path().join(name), "old\n").unwrap();
    }
    let patch = existing("a.txt", "-1 +1", "old", "new")
        + "--- a/b.txt\n+++ b/b.txt\n@@ -1 +1 @@\n-old\n+new\n";
    assert!(git_apply(root.path(), &patch).is_err());
    for name in ["a.txt", "b.txt"] {
        assert_eq!(
            std::fs::read_to_string(root.path().join(name)).unwrap(),
            "old\n"
        );
    }
}

#[test]
fn tool_reports_all_files_from_canonical_framing() {
    let root = tempfile::tempdir().unwrap();
    for name in ["a.txt", "b.txt"] {
        std::fs::write(root.path().join(name), "old\n").unwrap();
    }
    let patch = " ".to_string()
        + &existing("a.txt", "-1 +1", "old", "new")
        + " "
        + &existing("b.txt", "-1 +1", "old", "new");
    let tool = ApplyPatchTool::new(root.path());
    let output = temper_agent_io::block_on_with(move |_cx, _handle| async move {
        tool.execute("framing", serde_json::json!({"patch": patch}), None)
            .await
    })
    .unwrap();
    assert!(!output.is_error);
    assert!(format!("{:?}", output.content).contains("across 2 file(s)"));
}

#[test]
fn zero_context_hunks_replace_lines_before_the_end_of_a_file() {
    let root = tempfile::tempdir().unwrap();
    for name in ["a.txt", "b.txt", "c.txt"] {
        std::fs::write(root.path().join(name), "first\nmiddle\nlast\n").unwrap();
    }
    let patch = existing("a.txt", "-1 +1", "first", "updated first")
        + &existing("b.txt", "-2 +2", "middle", "updated middle")
        + "diff --git a/c.txt b/c.txt\n--- a/c.txt\n+++ b/c.txt\n@@ -1,0 +2 @@\n+inserted\n";
    git_apply(root.path(), &patch).unwrap();
    for (name, expected) in [
        ("a.txt", "updated first\nmiddle\nlast\n"),
        ("b.txt", "first\nupdated middle\nlast\n"),
        ("c.txt", "first\ninserted\nmiddle\nlast\n"),
    ] {
        assert_eq!(
            std::fs::read_to_string(root.path().join(name)).unwrap(),
            expected
        );
    }
}

#[test]
fn zero_context_source_mismatch_rejects_every_write_and_creation() {
    let root = tempfile::tempdir().unwrap();
    for name in ["a.txt", "b.txt"] {
        std::fs::write(root.path().join(name), "first\nmiddle\nlast\n").unwrap();
    }
    let patch = existing("a.txt", "-1 +1", "first", "updated first")
        + &existing("b.txt", "-2 +2", "wrong middle", "updated middle")
        + "diff --git a/new.txt b/new.txt\n--- /dev/null\n+++ b/new.txt\n@@ -0,0 +1 @@\n+created\n";
    assert!(git_apply(root.path(), &patch).is_err());
    for name in ["a.txt", "b.txt"] {
        assert_eq!(
            std::fs::read_to_string(root.path().join(name)).unwrap(),
            "first\nmiddle\nlast\n"
        );
    }
    assert!(!root.path().join("new.txt").exists());
}
