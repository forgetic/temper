use super::*;

fn response(value: Value, is_error: bool) -> McpToolCallResult {
    McpToolCallResult {
        text: value.to_string(),
        is_error,
        typed_parts: None,
    }
}

#[test]
fn released_provider_missing_project_error_allows_initial_indexing() {
    let result = response(
        json!({
            "error": "project not found or not indexed",
            "hint": "No projects indexed yet. Call index_repository first."
        }),
        true,
    );
    assert_eq!(
        parse_targeted_status(&result, "temper-v1-demo", None).unwrap(),
        TargetedProjectState::Missing
    );
}

#[test]
fn missing_project_error_does_not_hide_other_errors_or_conflicting_metadata() {
    for (value, is_error) in [
        (json!({"error": "project not found or not indexed"}), false),
        (json!({"error": "database unavailable"}), true),
        (
            json!({"error": "project not found or not indexed: database unavailable"}),
            true,
        ),
        (
            json!({"error": "project not found or not indexed", "status": "ready"}),
            true,
        ),
        (
            json!({"error": "project not found or not indexed", "state": "indexing"}),
            true,
        ),
        (
            json!({"error": "project not found or not indexed", "project": "different-project"}),
            true,
        ),
        (
            json!({"error": "project not found or not indexed", "root_path": "/other/root"}),
            true,
        ),
    ] {
        assert!(
            parse_targeted_status(&response(value.clone(), is_error), "temper-v1-demo", None)
                .is_err(),
            "must not turn an unknown or contradictory response into permission to index: {value}"
        );
    }
}
