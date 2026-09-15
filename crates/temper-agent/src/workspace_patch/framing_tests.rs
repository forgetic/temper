use super::{PatchOperation, patch_targets, prepare_patch};

fn existing(name: &str, counts: &str, body: &str) -> String {
    format!("diff --git a/{name} b/{name}\n--- a/{name}\n+++ b/{name}\n@@ {counts} @@\n{body}")
}

#[test]
fn indented_second_header_cannot_hide_a_target() {
    let patch = existing("a.txt", "-1 +1", "-old a\n+new a\n")
        + " "
        + &existing("b.txt", "-1 +1", "-old b\n+new b\n");
    let parsed = prepare_patch(&patch).unwrap();
    assert_eq!(
        parsed
            .targets
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["a.txt", "b.txt"]
    );
    assert!(parsed.text.contains("\ndiff --git a/b.txt b/b.txt\n"));
    assert_eq!(patch_targets(&patch).unwrap(), parsed.targets);
}

#[test]
fn recounts_each_hunk_and_inserts_explicit_creation_mode() {
    let patch = existing("a.txt", "-1,90 +1,80", "-old\n+new\n")
        + "diff --git a/new.txt b/new.txt\n--- /dev/null\n+++ b/new.txt\n@@ -0,0 +1,50 @@\n+created\n";
    let parsed = prepare_patch(&patch).unwrap();
    assert_eq!(parsed.targets["new.txt"], PatchOperation::Create);
    assert!(parsed.text.contains("@@ -1,1 +1,1 @@\n-old\n+new\n"));
    assert!(parsed.text.contains(
        "new file mode 100644\n--- /dev/null\n+++ b/new.txt\n@@ -0,0 +1,1 @@\n+created\n"
    ));
    assert_eq!(prepare_patch(&parsed.text).unwrap().text, parsed.text);
}

#[test]
fn valid_header_looking_context_survives_another_malformed_hunk() {
    let body = " diff --git a/decoy.txt b/decoy.txt\n--- a/decoy.txt\n+++ b/decoy.txt\n";
    let patch = existing("a.txt", "-1,2 +1,2", body) + "@@ -5,9 +5,9 @@\n-old\n+new\n";
    let parsed = prepare_patch(&patch).unwrap();
    assert_eq!(parsed.targets.len(), 1);
    assert!(parsed.targets.contains_key("a.txt"));
    assert!(parsed.text.contains(body));
    assert!(parsed.text.contains("@@ -5,1 +5,1 @@\n-old\n+new\n"));
}

#[test]
fn hidden_traditional_second_file_is_rejected() {
    let patch = existing("a.txt", "-1 +1", "-old a\n+new a\n")
        + "--- a/b.txt\n+++ b/b.txt\n@@ -1 +1 @@\n-old b\n+new b\n";
    assert!(prepare_patch(&patch).is_err());
}

#[test]
fn malformed_framing_and_unsafe_targets_are_rejected() {
    let valid = existing("a.txt", "-1 +1", "-old\n+new\n");
    for invalid in [
        valid.replace("+++ b/a.txt", "+++ b/other.txt"),
        valid.replace("a/a.txt", "a/../a.txt"),
        valid.replace("b/a.txt", "b/.git/config"),
        valid.replace("--- a/a.txt", "new file mode 120000\n--- a/a.txt"),
        valid.replace("--- a/a.txt", "new file mode 100644\n--- a/a.txt"),
        valid.replace("@@ -1 +1 @@", "@@ nonsense @@"),
        valid.replace("@@ -1 +1 @@", "GIT binary patch"),
        valid.clone() + "unconsumed framing\n",
        valid.clone() + &valid,
    ] {
        assert!(prepare_patch(&invalid).is_err(), "accepted {invalid:?}");
    }
}

#[test]
fn missing_newline_markers_and_carriage_return_data_are_preserved() {
    let body = "-old\r\n\\ No newline at end of file\n+new\r\n\\ No newline at end of file\n";
    let parsed = prepare_patch(&existing("a.txt", "-1 +1", body)).unwrap();
    assert!(parsed.text.ends_with(body));
    assert!(
        prepare_patch(&existing(
            "a.txt",
            "-1 +1",
            "\\ No newline at end of file\n-old\n+new\n"
        ))
        .is_err()
    );
}

#[test]
fn regular_executable_creation_keeps_its_requested_mode() {
    let parsed = prepare_patch("diff --git a/run.sh b/run.sh\nnew file mode 100755\nindex 0000000..abcdef0\n--- /dev/null\n+++ b/run.sh\n@@ -0,0 +1 @@\n+#!/bin/sh\n").unwrap();
    assert!(parsed.text.contains("new file mode 100755\n"));
    assert!(!parsed.text.contains("index "));
}
