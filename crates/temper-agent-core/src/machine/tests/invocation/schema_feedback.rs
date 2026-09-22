fn batch_catalog() -> Arc<ToolInvocationCatalog> {
    Arc::new(ToolInvocationCatalog::from_registry(&ToolRegistry::from_tools(vec![
        Box::new(ContractTool {
            name: "edit_files",
            effects: ToolEffects::write(),
            schema: serde_json::json!({
                "type":"object", "required":["files"], "properties":{
                    "files":{"type":"array", "minItems":1, "items":{
                        "type":"object", "required":["path","edits"], "properties":{
                            "path":{"type":"string"},
                            "edits":{"type":"array", "items":{
                                "type":"object", "required":["oldText","newText"],
                                "properties":{"oldText":{"type":"string"},"newText":{"type":"string"}}
                            }}
                        }
                    }}
                }
            }),
        }),
    ])).unwrap())
}

fn rejected_message(
    catalog: Arc<ToolInvocationCatalog>,
    api: &str,
    name: &str,
    arguments: serde_json::Value,
    expected_name: &str,
) -> String {
    let admission = Arc::new(CountingAdmission::default());
    let mut machine = machine(catalog).with_lineage_admission(admission.clone());
    let _ = machine.on_start(EngineTime::ZERO);
    let requests = complete(&mut machine, llm_responded(assistant(api, vec![("bad", name, arguments)])));
    let (call, rejection) = dispatched(&requests);
    assert_eq!(call.name, expected_name);
    assert_eq!(call.arguments, serde_json::json!({}));
    let failure = rejection.expect("schema rejection").clone();
    assert_eq!(failure.category, crate::ToolFailureCategory::SchemaArgumentMismatch);
    assert_eq!(admission.graph.load(Ordering::SeqCst), 0);
    assert_eq!(admission.targets.load(Ordering::SeqCst), 0);
    assert_eq!(admission.sources.load(Ordering::SeqCst), 0);
    let _ = complete(&mut machine, tool_failed("bad", tool_output("RAW-OUTPUT-SECRET", true), failure));
    let history = format!("{:?}", machine.messages());
    for secret in ["RAW-OUTPUT-SECRET", "PRIVATE-VALUE", "PRIVATE-KEY", "PRIVATE-TOOL"] {
        assert!(!history.contains(secret), "history leaked {secret}");
    }
    let result = machine.messages().iter().find_map(|message| match message {
        Message::ToolResult(result) => Some(result),
        _ => None,
    }).expect("model-visible local result");
    assert_eq!(result.tool_name, expected_name);
    assert!(result.is_error);
    assert!(result.details.is_none());
    result.content.iter().filter_map(|block| match block {
        ContentBlock::Text(text) => Some(text.text.as_str()),
        _ => None,
    }).collect::<Vec<_>>().join("\n")
}

#[test]
fn schema_feedback_reports_nested_missing_fields_without_values_or_unknown_keys() {
    let message = rejected_message(batch_catalog(), "openai-responses", "edit_files",
        serde_json::json!({"files":[{"path":"PRIVATE-VALUE", "edits":[{
            "newText":"PRIVATE-VALUE", "PRIVATE-KEY":"PRIVATE-VALUE"
        }]}]}), "edit_files");
    assert!(message.ends_with("Tool edit_files: required field $.files[].edits[].oldText is missing."));
}

#[test]
fn schema_feedback_reports_nested_types_and_preserves_known_alias_identity() {
    let message = rejected_message(batch_catalog(), "openai-responses", "edit_files",
        serde_json::json!({"files":[{"path":"PRIVATE-VALUE", "edits":[{
            "oldText":"PRIVATE-VALUE", "newText":{"PRIVATE-KEY":"PRIVATE-VALUE"}
        }]}]}), "edit_files");
    assert!(message.ends_with("Tool edit_files: $.files[].edits[].newText must have type string."));
    let message = rejected_message(catalog(&["read"]), "anthropic-messages", "Read",
        serde_json::json!({"file_path":123}), "read");
    assert!(message.ends_with("Tool read: $.path must have type string."));
}

#[test]
fn schema_feedback_unknown_names_and_unknown_keys_remain_private() {
    let message = rejected_message(catalog(&["read"]), "openai-responses", "PRIVATE-TOOL",
        serde_json::json!({"PRIVATE-KEY":"PRIVATE-VALUE"}), REJECTED_TOOL_NAME);
    assert_eq!(message, ToolFailureDiagnostic::schema(ToolFailureReason::UnknownTool).model_message());
    let message = rejected_message(catalog(&["read"]), "openai-responses", "read",
        serde_json::json!({"path":"PRIVATE-VALUE","PRIVATE-KEY":"PRIVATE-VALUE"}), "read");
    assert!(message.ends_with("Tool read: use the required fields and types in the tool schema."));
}

#[test]
fn schema_feedback_never_changes_valid_batch_acceptance_or_arguments() {
    let arguments = serde_json::json!({"files":[{"path":"src/lib.rs", "edits":[{
        "oldText":"before", "newText":"after"
    }]}]});
    let normalized = batch_catalog().canonicalize("openai-responses", ToolCall {
        id:"valid".to_string(), name:"edit_files".to_string(), arguments:arguments.clone(),
    });
    assert_eq!(normalized.call.name, "edit_files");
    assert_eq!(normalized.call.arguments, arguments);
    assert!(normalized.rejection.is_none());
    assert!(normalized.schema_feedback.is_none());
}

#[test]
fn schema_feedback_root_type_and_missing_field_are_actionable() {
    let message = rejected_message(batch_catalog(), "openai-responses", "edit_files",
        serde_json::json!(["PRIVATE-VALUE"]), "edit_files");
    assert!(message.ends_with("Tool edit_files: $ must have type object."));
    let message = rejected_message(batch_catalog(), "openai-responses", "edit_files",
        serde_json::json!({}), "edit_files");
    assert!(message.ends_with("Tool edit_files: required field $.files is missing."));
}

#[test]
fn schema_feedback_cannot_override_policy_or_other_failure_categories() {
    let normalized = batch_catalog().canonicalize("openai-responses", ToolCall {
        id: "denied".to_string(), name: "edit_files".to_string(), arguments: serde_json::json!({}),
    });
    for failure in [
        ToolFailureDiagnostic::policy_denial(),
        ToolFailureDiagnostic::execution(ToolFailureReason::ToolReportedFailure),
        ToolFailureDiagnostic::schema(ToolFailureReason::UnknownTool),
    ] {
        let message = crate::machine::messages::tool_result_message(
            "denied", "edit_files", tool_output("PRIVATE-VALUE", true), Some(failure.clone()),
            normalized.schema_feedback.clone(),
        );
        let ContentBlock::Text(text) = &message.content[0] else { panic!("text failure") };
        assert_eq!(text.text, failure.model_message());
        assert!(!text.text.contains("$.files"));
    }
}

#[test]
fn schema_feedback_preserves_rejected_read_batch_barriers_and_result_identity() {
    let mut machine = machine(catalog(&["read"]));
    let _ = machine.on_start(EngineTime::ZERO);
    let first = complete(&mut machine, llm_responded(assistant("openai-responses", vec![
        ("first", "read", serde_json::json!({"path":"a"})),
        ("invalid", "read", serde_json::json!({})),
        ("last", "read", serde_json::json!({"path":"b"})),
    ])));
    assert_eq!(run_tools(&first), ["first"]);
    let second = complete(&mut machine, tool_finished("first", tool_output("first", false)));
    assert_eq!(run_tools(&second), ["invalid"]);
    let (call, failure) = dispatched(&second);
    assert_eq!(call.name, "read");
    let failure = failure.unwrap().clone();
    let third = complete(&mut machine, tool_failed("invalid", tool_output("ignored", true), failure));
    assert_eq!(run_tools(&third), ["last"]);
    let _ = complete(&mut machine, tool_finished("last", tool_output("last", false)));
    let messages = machine.messages().iter().filter_map(|message| match message {
        Message::ToolResult(result) => Some(result),
        _ => None,
    }).collect::<Vec<_>>();
    assert_eq!(messages.iter().map(|result| result.tool_call_id.as_str()).collect::<Vec<_>>(), ["first", "invalid", "last"]);
    let ContentBlock::Text(text) = &messages[1].content[0] else { panic!("text failure") };
    assert!(text.text.ends_with("Tool read: required field $.path is missing."));
    assert!(!format!("{:?}", messages[2]).contains("required field"));
}
