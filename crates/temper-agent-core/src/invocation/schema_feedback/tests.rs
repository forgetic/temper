use super::*;
use serde_json::json;

#[test]
fn schema_feedback_is_bounded_for_deep_or_unrenderable_schema_paths() {
    let mut schema = json!({"type":"string"});
    let mut value = json!(42);
    for _ in 0..20 {
        schema = json!({"type":"object","properties":{"nested":schema}});
        value = json!({"nested":value});
    }
    let detail = SchemaFeedback::from_schema("known_tool", &schema, &value).unwrap();
    assert_eq!(
        detail.message(),
        "Tool known_tool: use the required fields and types in the tool schema."
    );
    for key in [
        "x".repeat(1000),
        "unsafe\nfield".to_string(),
        "field.with.dots".to_string(),
    ] {
        let schema =
            json!({"type":"object","properties":{key.clone():{"type":"string"}},"required":[key]});
        let detail = SchemaFeedback::from_schema("known_tool", &schema, &json!({})).unwrap();
        assert!(detail.message().len() < 128);
        assert!(!detail.message().contains("unsafe\n"));
        assert!(!detail.message().contains("field.with.dots"));
    }
    assert!(SchemaFeedback::from_schema(&"t".repeat(129), &schema, &value).is_none());
    assert!(SchemaFeedback::from_schema("unsafe\nname", &schema, &value).is_none());
}

#[test]
fn schema_feedback_does_not_report_requirements_from_alternative_branches() {
    for keyword in ["anyOf", "oneOf"] {
        let schema = json!({keyword:[
            {"type":"object","properties":{"left":{"type":"string"}},"required":["left"]},
            {"type":"object","properties":{"right":{"type":"string"}},"required":["right"]}
        ]});
        let detail = SchemaFeedback::from_schema("known_tool", &schema, &json!({})).unwrap();
        assert_eq!(
            detail.message(),
            "Tool known_tool: use the required fields and types in the tool schema."
        );
    }
    let schema = json!({"allOf":[{"type":"object","properties":{"field":{"type":"string"}},"required":["field"]}]});
    assert_eq!(
        SchemaFeedback::from_schema("known_tool", &schema, &json!({}))
            .unwrap()
            .message(),
        "Tool known_tool: required field $.field is missing."
    );
}

#[test]
fn schema_feedback_never_echoes_enum_values_or_supplied_unknown_properties() {
    let schema =
        json!({"type":"object","properties":{"mode":{"type":"string","enum":["SCHEMA-VALUE"]}}});
    let detail = SchemaFeedback::from_schema(
        "known_tool",
        &schema,
        &json!({"mode":"PRIVATE-VALUE","PRIVATE-KEY":"PRIVATE-VALUE"}),
    )
    .unwrap();
    assert!(!detail.message().contains("SCHEMA-VALUE"));
    assert!(!detail.message().contains("PRIVATE"));
    assert_eq!(
        detail.message(),
        "Tool known_tool: use the required fields and types in the tool schema."
    );
}
