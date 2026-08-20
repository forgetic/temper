use super::is_current_root_source_result;

#[test]
fn recognizes_structured_current_root_source_results() {
    assert!(is_current_root_source_result(
        r#"{"qualified_name":"retry_worker_topic","file_path":"src/lib.rs","source":"<fixture source>","binding":"current_prepared_checkout"}"#
    ));
    assert!(is_current_root_source_result(
        r#"{"qualified_name":"retry_worker_topic","file_path":"src/lib.rs","source":"<fixture source>","binding":"current_prepared_checkout"}

[Decision anchor: generic guidance]"#
    ));
}

#[test]
fn rejects_legacy_or_incomplete_source_markers() {
    assert!(!is_current_root_source_result("FAKE_MCP_SNIPPET_RESULT"));
    assert!(!is_current_root_source_result(
        r#"{"qualified_name":"retry_worker_topic","file_path":"src/lib.rs","binding":"current_prepared_checkout"}"#
    ));
    assert!(!is_current_root_source_result(
        r#"{"qualified_name":"retry_worker_topic","file_path":"src/lib.rs","source":"<fixture source>","binding":"unconfirmed"}"#
    ));
}

#[test]
fn recognizes_privacy_safe_typed_lineage_source_results_without_a_path() {
    assert!(is_current_root_source_result(
        r#"{"qualifiedName":"crate::fixture::selection","source":"<fixture source>","binding":"current_prepared_checkout"}"#
    ));
    assert!(is_current_root_source_result(
        r#"{"functionName":"selection","source":"<fixture source>","binding":"current_prepared_checkout"}"#
    ));
}
