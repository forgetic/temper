use super::*;
use serde_json::{Value, json};

fn edit(path: &str, old: &str, new: &str) -> Value {
    json!({"path": path, "edits": [{"oldText": old, "newText": new}]})
}

fn apply(root: &Path, files: Vec<Value>) -> std::result::Result<usize, String> {
    let input = EditFilesInput::parse(&json!({"files": files}))?;
    file_updates::commit(root, prepare(root, &input)?, "edited")
}

#[test]
fn edit_files_preserves_exact_bytes_and_uses_only_original_matches() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("one.txt"),
        "\u{feff}alpha\r\nsecond β\r\nlast",
    )
    .unwrap();
    std::fs::write(root.path().join("two.txt"), "first SECOND third").unwrap();
    assert_eq!(
        apply(
            root.path(),
            vec![
                json!({"path":"one.txt","edits":[
            {"oldText":"alpha","newText":"gamma"},
            {"oldText":"second β","newText":"changed β"}]}),
                json!({"path":"two.txt","edits":[
            {"oldText":"SECOND","newText":"third"},
            {"oldText":"first","newText":"SECOND"}]})
            ]
        )
        .unwrap(),
        2
    );
    assert_eq!(
        std::fs::read(root.path().join("one.txt")).unwrap(),
        "\u{feff}gamma\r\nchanged β\r\nlast".as_bytes()
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("two.txt")).unwrap(),
        "SECOND third third"
    );
    assert_eq!(
        apply(root.path(), vec![edit("two.txt", "SECOND", "SECOND")]).unwrap(),
        0
    );
}

#[test]
fn edit_files_rejects_missing_repeated_overlapping_and_fuzzy_matches_before_any_write() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("one.txt"), "unchanged").unwrap();
    for (source, edits) in [
        ("old", json!([{"oldText":"missing","newText":"new"}])),
        ("old old", json!([{"oldText":"old","newText":"new"}])),
        ("aaa", json!([{"oldText":"aa","newText":"new"}])),
        ("βββ", json!([{"oldText":"ββ","newText":"new"}])),
        (
            "abcd",
            json!([{"oldText":"abc","newText":"a"},{"oldText":"bcd","newText":"b"}]),
        ),
        ("old\r\n", json!([{"oldText":"old\n","newText":"new\n"}])),
        ("  old", json!([{"oldText":" old ","newText":"new"}])),
    ] {
        std::fs::write(root.path().join("two.txt"), source).unwrap();
        assert!(
            apply(
                root.path(),
                vec![
                    edit("one.txt", "unchanged", "changed"),
                    json!({"path":"two.txt","edits":edits})
                ]
            )
            .is_err()
        );
        assert_eq!(
            std::fs::read_to_string(root.path().join("one.txt")).unwrap(),
            "unchanged"
        );
        assert_eq!(
            std::fs::read_to_string(root.path().join("two.txt")).unwrap(),
            source
        );
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 2);
    }
}

#[test]
fn edit_files_allows_adjacent_replacements_and_deletion() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("one.txt"), "abcd").unwrap();
    apply(
        root.path(),
        vec![json!({"path":"one.txt","edits":[
        {"oldText":"cd","newText":"y"},{"oldText":"ab","newText":""}]})],
    )
    .unwrap();
    assert_eq!(std::fs::read(root.path().join("one.txt")).unwrap(), b"y");
}

#[test]
fn edit_files_closed_parser_rejects_aliases_duplicates_and_unbounded_batches() {
    let file = edit("one.txt", "old", "new");
    for value in [
        json!({"files":[]}),
        json!({"files":[file.clone()],"extra":true}),
        json!({"files":[file.clone(),file.clone()]}),
        json!({"files":[{"path":"one.txt","edits":[],"create":true}]}),
        json!({"files":[{"path":"one.txt","edits":[{"oldText":"old","newText":"new","replaceAll":true}]}]}),
        json!({"files":[{"path":"one.txt","edits":[{"old_text":"old","newText":"new"}]}]}),
        json!({"files":[edit("one.txt","","new")]}),
        json!({"files":[edit("one.txt","old",&"x".repeat(MAX_FILE_BYTES + 1))]}),
        json!({"files":[{"path":"one.txt","edits":vec![json!({"oldText":"old","newText":"new"}); MAX_EDITS_PER_FILE+1]}]}),
        json!({"files":(0..65).map(|i|edit(&format!("{i}.txt"),"old","new")).collect::<Vec<_>>()}),
    ] {
        assert!(EditFilesInput::parse(&value).is_err());
    }
    for path in [
        "../one.txt",
        "/one.txt",
        "./one.txt",
        "a//one.txt",
        "a\\one.txt",
        ".git/config",
        "a/../one.txt",
        "one.txt ",
    ] {
        assert!(
            EditFilesInput::parse(&json!({"files":[edit(path,"old","new")]})).is_err(),
            "{path}"
        );
    }
}

#[test]
fn edit_files_missing_non_utf8_and_oversized_targets_leave_all_sources_unchanged() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("one.txt"), "old").unwrap();
    assert!(
        apply(
            root.path(),
            vec![
                edit("one.txt", "old", "new"),
                edit("missing.txt", "old", "new")
            ]
        )
        .is_err()
    );
    assert!(!root.path().join("missing.txt").exists());
    for bytes in [vec![0xff], vec![b'x'; MAX_FILE_BYTES + 1]] {
        std::fs::write(root.path().join("two.txt"), &bytes).unwrap();
        assert!(
            apply(
                root.path(),
                vec![edit("one.txt", "old", "new"), edit("two.txt", "x", "new")]
            )
            .is_err()
        );
        assert_eq!(std::fs::read(root.path().join("two.txt")).unwrap(), bytes);
        assert_eq!(std::fs::read(root.path().join("one.txt")).unwrap(), b"old");
    }
}

#[cfg(unix)]
#[test]
fn edit_files_symlink_target_never_redirects_an_admitted_batch() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("one.txt"), "old").unwrap();
    std::fs::write(outside.path().join("foreign.txt"), "old").unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("foreign.txt"),
        root.path().join("two.txt"),
    )
    .unwrap();
    assert!(
        apply(
            root.path(),
            vec![edit("one.txt", "old", "new"), edit("two.txt", "old", "new")]
        )
        .is_err()
    );
    assert_eq!(std::fs::read(root.path().join("one.txt")).unwrap(), b"old");
    assert_eq!(
        std::fs::read(outside.path().join("foreign.txt")).unwrap(),
        b"old"
    );
}

#[test]
fn edit_files_joined_executor_and_writable_registry_preserve_the_mutation_barrier() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("one.txt"), "old").unwrap();
    let tool = temper_agent_core::joined_filesystem_tool(Box::new(EditFilesTool::new(root.path())));
    assert!(tool.effects().writes);
    let output = temper_agent_io::block_on_with(move |_cx, _handle| async move {
        tool.execute(
            "edit-files",
            json!({"files":[edit("one.txt","old","new")]}),
            None,
        )
        .await
    })
    .unwrap();
    assert!(!output.is_error);
    assert_eq!(std::fs::read(root.path().join("one.txt")).unwrap(), b"new");
    for capability in [
        super::super::Capability::TriageWorkspace,
        super::super::Capability::ReviewWorkspace,
    ] {
        assert!(
            !super::super::tools::tool_registry(capability, root.path())
                .tools()
                .iter()
                .any(|tool| tool.name() == "edit_files")
        );
    }
    assert!(
        super::super::tools::tool_registry(super::super::Capability::CodingWorkspace, root.path())
            .tools()
            .iter()
            .any(|tool| tool.name() == "edit_files")
    );
}
