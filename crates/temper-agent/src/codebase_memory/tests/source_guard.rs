use super::*;
use serde_json::json;

const LOCAL: &str = "def sentinel():\n    return 'checkout-A'\n";
const FOREIGN: &str = "def sentinel():\n    return 'checkout-B'\n";

fn response(value: Value) -> McpToolCallResult {
    McpToolCallResult {
        text: value.to_string(),
        is_error: false,
        typed_parts: Some(vec![McpToolResultPart::StructuredContent(value)]),
    }
}

fn fixture() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("sentinel.py"), LOCAL).unwrap();
    root
}

#[test]
fn same_symbol_path_and_lines_cannot_hide_foreign_checkout_or_aba() {
    let root = fixture();
    for file in [
        "sentinel.py".to_string(),
        root.path().join("sentinel.py").display().to_string(),
    ] {
        for source in [FOREIGN, LOCAL] {
            let mut result = response(
                json!({"qualified_name":"fixture.sentinel.sentinel", "file_path":file,
                "start_line":1,"end_line":2,"source":source}),
            );
            assert_eq!(verify_at_root(root.path(), &mut result), source == LOCAL);
        }
    }
}

#[test]
fn release_columnar_nested_source_and_context_are_verified() {
    let root = fixture();
    for source in [FOREIGN, LOCAL] {
        for cell in [
            json!({"source": source, "source_start":1}),
            json!({"context":source,"context_start":1}),
        ] {
            let column = if cell.get("source").is_some() {
                "source"
            } else {
                "context"
            };
            let mut result = response(json!({"cols":["qn","file","lines",column],
                "rows":[["fixture.sentinel.sentinel","sentinel.py","1-2",cell]]}));
            crate::codebase_memory::provider_output::normalize(&mut result);
            assert_eq!(verify_at_root(root.path(), &mut result), source == LOCAL);
        }
    }
}

#[test]
fn whole_file_and_exact_window_work_but_partial_missing_or_malformed_ranges_do_not() {
    let root = fixture();
    for (value, expected) in [
        (json!({"file_path":"sentinel.py","source":LOCAL}), true),
        (
            json!({"file_path":"sentinel.py","source":"    return 'checkout-A'\n","start_line":2,"end_line":2}),
            true,
        ),
        (
            json!({"file_path":"sentinel.py","source":"checkout-A"}),
            false,
        ),
        (
            json!({"file_path":"sentinel.py","source":LOCAL,"start_line":1}),
            false,
        ),
        (
            json!({"file_path":"sentinel.py","source":LOCAL,"start_line":1,"end_line":999}),
            false,
        ),
        (json!({"file_path":"missing.py","source":LOCAL}), false),
        (
            json!({"file_path":"sentinel.py","source":LOCAL,"lines":"bad"}),
            false,
        ),
    ] {
        assert_eq!(verify_at_root(root.path(), &mut response(value)), expected);
    }
}

#[test]
fn whole_mixed_result_and_untyped_sidecars_are_rejected() {
    let root = fixture();
    let valid = json!({"file_path":"sentinel.py","source":LOCAL});
    let foreign = json!({"file_path":"sentinel.py","source":FOREIGN});
    assert!(!verify_at_root(
        root.path(),
        &mut response(json!([valid, foreign]))
    ));
    let mut result = response(valid.clone());
    result
        .typed_parts
        .as_mut()
        .unwrap()
        .push(McpToolResultPart::Content(
            json!({"type":"text","text":FOREIGN}),
        ));
    assert!(!verify_at_root(root.path(), &mut result));
    result = response(valid);
    result.typed_parts = None;
    assert!(!verify_at_root(root.path(), &mut result));
}

#[test]
fn stored_source_metadata_is_removed_without_claiming_graph_snapshot_identity() {
    let root = fixture();
    let mut result = response(
        json!({"results":[{"file_path":"sentinel.py","name":"sentinel",
        "signature":FOREIGN,"docstring":FOREIGN,"return_type":FOREIGN}]}),
    );
    assert!(verify_at_root(root.path(), &mut result));
    assert!(!result.text.contains("checkout-B"));
    assert!(result.text.contains("sentinel"));
}

#[test]
fn foreign_absolute_and_escaping_symlink_paths_are_rejected() {
    let root = fixture();
    let foreign = fixture();
    assert!(!verify_at_root(
        root.path(),
        &mut response(json!({"file_path":foreign.path().join("sentinel.py"),"source":LOCAL}))
    ));
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(
            foreign.path().join("sentinel.py"),
            root.path().join("escape.py"),
        )
        .unwrap();
        assert!(!verify_at_root(
            root.path(),
            &mut response(json!({"file_path":"escape.py","source":LOCAL}))
        ));
    }
}

#[test]
#[cfg(unix)]
fn nonregular_fifo_is_rejected_before_open() {
    let root = fixture();
    assert!(
        std::process::Command::new("mkfifo")
            .arg(root.path().join("fifo"))
            .status()
            .unwrap()
            .success()
    );
    let started = std::time::Instant::now();
    assert!(!verify_at_root(
        root.path(),
        &mut response(json!({"file_path":"fifo","source":LOCAL}))
    ));
    assert!(started.elapsed() < std::time::Duration::from_secs(1));
}

#[test]
fn malformed_typed_shapes_and_unknown_source_sidecars_never_reach_presentation() {
    let root = fixture();
    for value in [
        json!(FOREIGN),
        json!([FOREIGN]),
        json!({"text":FOREIGN}),
        json!({"results":[FOREIGN]}),
    ] {
        assert!(!verify_at_root(root.path(), &mut response(value)));
    }
    let mut result = response(
        json!({"file_path":"sentinel.py","source":LOCAL,"extra":{"text":FOREIGN},"message":FOREIGN}),
    );
    assert!(verify_at_root(root.path(), &mut result));
    assert!(!result.text.contains("checkout-B"));
}

#[test]
fn raw_search_match_content_is_checked_at_its_exact_line() {
    let root = fixture();
    for (content, accepted) in [
        ("    return 'checkout-A'", true),
        ("    return 'checkout-B'", false),
    ] {
        let mut result = response(
            json!({"results":[],"raw_matches":{"results":[{"file_path":"sentinel.py","line":2,"content":content}]}}),
        );
        assert_eq!(verify_at_root(root.path(), &mut result), accepted);
    }
}

#[test]
fn decorator_metadata_does_not_exempt_source_fields_or_malformed_source_paths() {
    let root = fixture();
    let foreign = fixture();
    for (path, source) in [
        (json!(""), json!(LOCAL)),
        (Value::Null, json!(LOCAL)),
        (json!(foreign.path().join("sentinel.py")), json!(LOCAL)),
        (json!("sentinel.py"), json!(FOREIGN)),
        (json!(""), json!({"source": LOCAL})),
    ] {
        let mut result = response(json!({
            "cols": ["qn", "label", "file", "lines", "rank", "source"],
            "search_mode": "bm25",
            "rows": [["<decorator:test>", "Decorator", path, "", -7.252591004539256, source]]
        }));
        crate::codebase_memory::provider_output::normalize(&mut result);
        assert!(!verify_at_root(root.path(), &mut result));
    }
    for row in [
        json!(["fixture.sentinel", "Function", "", "", -1]),
        json!(["fixture.sentinel", "Decorator", "", "", -1]),
        json!(["<decorator:test>", "Decorator", null, "", -1]),
        json!(["<decorator:test>", "Decorator", "", "1-2", -1]),
    ] {
        let mut result = response(json!({
            "cols": ["qn", "label", "file", "lines", "rank"], "rows": [row],
            "search_mode": "bm25"
        }));
        crate::codebase_memory::provider_output::normalize(&mut result);
        assert!(!verify_at_root(root.path(), &mut result));
    }
}
