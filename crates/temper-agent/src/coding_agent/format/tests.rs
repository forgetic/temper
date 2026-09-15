use super::*;
use crate::workspace_format::{MAX_FORMAT_BYTES, existing_format_target};

const UNFORMATTED: &str = "fn example(){let value=1;println!(\"{value}\");}\n";

fn input(paths: &[&str]) -> FormatRustInput {
    FormatRustInput::parse(&serde_json::json!({"paths": paths, "edition": "2021"})).unwrap()
}

fn format(root: &Path, paths: &[&str]) -> std::result::Result<usize, String> {
    workspace::commit(
        root,
        workspace::prepare(
            root,
            &input(paths),
            &AgentContainmentContext::production(None),
        )?,
    )
}

#[test]
fn real_formatter_preserves_project_style_without_visiting_child_modules() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let external = outside.path().join("external.rs");
    std::fs::write(&external, "invalid external Rust !!!").unwrap();
    std::fs::write(root.path().join("child.rs"), "invalid child Rust !!!").unwrap();
    std::fs::write(root.path().join("rustfmt.toml"), "hard_tabs = true\n").unwrap();
    let source = format!(
        "mod child;\n#[path = {:?}] mod external;\n{UNFORMATTED}",
        external
    );
    let path = root.path().join("lib.rs");
    std::fs::write(&path, &source).unwrap();
    assert_eq!(format(root.path(), &["lib.rs"]).unwrap(), 1);
    let formatted = std::fs::read_to_string(&path).unwrap();
    assert!(formatted.contains("\tlet value = 1;"), "{formatted}");
    assert_eq!(
        std::fs::read_to_string(root.path().join("child.rs")).unwrap(),
        "invalid child Rust !!!"
    );
    assert_eq!(
        std::fs::read_to_string(external).unwrap(),
        "invalid external Rust !!!"
    );
    assert_eq!(format(root.path(), &["lib.rs"]).unwrap(), 0);
}

#[test]
fn invalid_later_file_does_not_write_earlier_formatted_output() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("one.rs"), UNFORMATTED).unwrap();
    std::fs::write(root.path().join("two.rs"), "fn broken( {").unwrap();
    assert!(format(root.path(), &["one.rs", "two.rs"]).is_err());
    assert_eq!(
        std::fs::read_to_string(root.path().join("one.rs")).unwrap(),
        UNFORMATTED
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("two.rs")).unwrap(),
        "fn broken( {"
    );
    assert_eq!(
        std::fs::read_dir(root.path()).unwrap().count(),
        2,
        "staging files are removed"
    );
}

#[test]
fn changed_later_preimage_prevents_all_replacements() {
    let root = tempfile::tempdir().unwrap();
    for name in ["one.rs", "two.rs"] {
        std::fs::write(root.path().join(name), UNFORMATTED).unwrap();
    }
    let prepared = workspace::prepare(
        root.path(),
        &input(&["one.rs", "two.rs"]),
        &AgentContainmentContext::production(None),
    )
    .unwrap();
    std::fs::write(root.path().join("two.rs"), "fn externally_changed() {}\n").unwrap();
    assert!(
        workspace::commit(root.path(), prepared)
            .unwrap_err()
            .contains("target changed")
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("one.rs")).unwrap(),
        UNFORMATTED
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("two.rs")).unwrap(),
        "fn externally_changed() {}\n"
    );
}

#[test]
fn shared_parser_rejects_ambiguous_paths_editions_and_extra_arguments() {
    for paths in [
        vec![],
        vec!["../one.rs"],
        vec!["/one.rs"],
        vec!["./one.rs"],
        vec!["one.rs", "one.rs"],
        vec!["one.txt"],
        vec!["a//one.rs"],
        vec!["a\\one.rs"],
    ] {
        assert!(
            FormatRustInput::parse(&serde_json::json!({"paths": paths, "edition": "2021"}))
                .is_err()
        );
    }
    for arguments in [
        serde_json::json!({"paths": ["one.rs"], "edition": "2025"}),
        serde_json::json!({"paths": ["one.rs"], "edition": "2021", "config": "skip_children=false"}),
    ] {
        assert!(FormatRustInput::parse(&arguments).is_err());
    }
}

#[test]
fn missing_non_utf8_and_oversized_inputs_do_not_write_other_targets() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("one.rs"), UNFORMATTED).unwrap();
    assert!(format(root.path(), &["one.rs", "missing.rs"]).is_err());
    for bad in [vec![0xff], vec![b' '; MAX_FORMAT_BYTES + 1]] {
        std::fs::write(root.path().join("two.rs"), bad).unwrap();
        assert!(format(root.path(), &["one.rs", "two.rs"]).is_err());
        assert_eq!(
            std::fs::read_to_string(root.path().join("one.rs")).unwrap(),
            UNFORMATTED
        );
    }
}

#[cfg(unix)]
#[test]
fn rejects_symlink_targets_and_parent_swaps_and_preserves_permissions() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let path = root.path().join("one.rs");
    std::fs::write(&path, UNFORMATTED).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
    symlink(&path, root.path().join("alias.rs")).unwrap();
    assert!(existing_format_target(root.path(), "alias.rs").is_err());
    symlink(outside.path(), root.path().join("linked")).unwrap();
    std::fs::write(outside.path().join("foreign.rs"), UNFORMATTED).unwrap();
    assert!(format(root.path(), &["one.rs", "linked/foreign.rs"]).is_err());
    assert_eq!(format(root.path(), &["one.rs"]).unwrap(), 1);
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o640
    );

    std::fs::create_dir(root.path().join("nested")).unwrap();
    std::fs::write(root.path().join("nested/foreign.rs"), UNFORMATTED).unwrap();
    let prepared = workspace::prepare(
        root.path(),
        &input(&["nested/foreign.rs"]),
        &AgentContainmentContext::production(None),
    )
    .unwrap();
    std::fs::rename(root.path().join("nested"), root.path().join("moved")).unwrap();
    symlink(outside.path(), root.path().join("nested")).unwrap();
    assert!(workspace::commit(root.path(), prepared).is_err());
    assert_eq!(
        std::fs::read_to_string(outside.path().join("foreign.rs")).unwrap(),
        UNFORMATTED
    );
}

#[test]
fn joined_tool_formats_inside_agent_executor_and_is_writable_only() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("one.rs"), UNFORMATTED).unwrap();
    let tool = temper_agent_core::joined_filesystem_tool(Box::new(FormatRustTool::new(
        root.path(),
        AgentContainmentContext::production(None),
    )));
    let output = temper_agent_io::block_on_with(move |_cx, _handle| async move {
        tool.execute(
            "format",
            serde_json::json!({"paths": ["one.rs"], "edition": "2021"}),
            None,
        )
        .await
    })
    .unwrap();
    assert!(!output.is_error);
    assert_ne!(
        std::fs::read_to_string(root.path().join("one.rs")).unwrap(),
        UNFORMATTED
    );
    for capability in [
        super::super::Capability::TriageWorkspace,
        super::super::Capability::ReviewWorkspace,
    ] {
        assert!(
            !super::super::tools::tool_registry(capability, root.path())
                .tools()
                .iter()
                .any(|tool| tool.name() == "format_rust")
        );
    }
    assert!(
        super::super::tools::tool_registry(super::super::Capability::CodingWorkspace, root.path())
            .tools()
            .iter()
            .any(|tool| tool.name() == "format_rust")
    );
}
