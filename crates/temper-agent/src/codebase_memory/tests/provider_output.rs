use super::*;
use serde_json::json;

fn result(value: Value) -> McpToolCallResult {
    McpToolCallResult {
        text: value.to_string(),
        is_error: false,
        typed_parts: Some(vec![McpToolResultPart::StructuredContent(value)]),
    }
}

#[test]
fn release_tables_preserve_provider_order_identity_and_pagination() {
    let mut response = result(json!({
        "cols": ["name", "label", "lines", "is_test"],
        "groups": [
            {"qn_prefix": "demo.example", "file": "example.py", "rows": [["increment", "Function", "1-2", false]]},
            {"qn_prefix": "demo.tests", "file": "tests/test_example.py", "rows": [["test_increment", "Function", "1-3", true]]}
        ],
        "total": 3, "count": 2, "has_more": true
    }));
    normalize(&mut response);
    let value: Value = serde_json::from_str(&response.text).unwrap();
    assert_eq!(
        value["results"][0]["qualified_name"],
        "demo.example.increment"
    );
    assert_eq!(value["results"][1]["file_path"], "tests/test_example.py");
    assert_eq!(value["results"][1]["is_test"], true);
    assert_eq!(value["has_more"], true);
    assert_eq!(value["total"], 3);
    assert!(response.typed_parts.is_some());
}

#[test]
fn release_trace_and_search_tables_preserve_nested_evidence() {
    let mut response = result(json!({
        "callers": {"cols": ["name", "hop"], "groups": [{"qn_prefix": "demo.example", "rows": [["run", 1]]}]},
        "search": {"cols": ["qn", "file", "source"], "rows": [["demo.example.run", "example.py", {"source": "return increment(41)"}]]}
    }));
    normalize(&mut response);
    let value: Value = serde_json::from_str(&response.text).unwrap();
    assert_eq!(
        value["callers"]["results"][0]["qualified_name"],
        "demo.example.run"
    );
    assert_eq!(value["search"]["results"][0]["file_path"], "example.py");
    assert_eq!(
        value["search"]["results"][0]["source"]["source"],
        "return increment(41)"
    );
}

#[test]
fn malformed_or_expanding_tables_cannot_create_typed_authority() {
    for value in [
        json!({"cols": ["qn", "file"], "rows": [["demo.run"]]}),
        json!({"cols": ["qn", "qualified_name"], "rows": [["demo.run", "other.run"]]}),
        json!({"cols": ["name", "qualified_name"], "groups": [{"qn_prefix": "demo", "rows": [["run", "other.run"]]}]}),
        json!({"cols": ["name"], "groups": [{"qn_prefix": "x".repeat(8000), "rows": [["a"], ["b"], ["c"]]}]}),
    ] {
        let mut response = result(value);
        let original = response.text.clone();
        normalize(&mut response);
        assert_eq!(response.text, original);
        assert!(response.typed_parts.is_none());
    }
}

#[test]
fn conflicting_mcp_representations_cannot_be_normalized_into_authority() {
    let mut response = result(json!({"cols": ["qn"], "rows": [["demo.run"]]}));
    response
        .typed_parts
        .as_mut()
        .unwrap()
        .push(McpToolResultPart::StructuredContent(
            json!({"cols": ["qn"], "rows": [["other.run"]]}),
        ));
    normalize(&mut response);
    assert!(response.typed_parts.is_none());
}

#[test]
fn source_less_decorator_is_omitted_without_rewriting_provider_counts() {
    let mut response = result(json!({
        "cols": ["qn", "label", "file", "lines", "rank"],
        "rows": [["<decorator:test>", "Decorator", "", "", -7.252591004539256]],
        "total": 1, "has_more": false, "search_mode": "bm25"
    }));
    normalize(&mut response);
    let value: Value = serde_json::from_str(&response.text).unwrap();
    assert_eq!(value["results"], json!([]));
    assert_eq!(value["total"], 1);
    assert_eq!(value["has_more"], false);
    assert_eq!(value["omitted_non_source_nodes"], 1);
    assert!(!response.text.contains("<decorator:test>"));
    assert!(response.typed_parts.is_some());
}
