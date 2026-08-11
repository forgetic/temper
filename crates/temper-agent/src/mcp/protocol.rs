//! MCP descriptor/result parsing and JSON rendering helpers.

use serde_json::{Value, json};

use super::client::McpError;

/// Typed result parts are wrapper-private and deliberately bounded. They keep
/// MCP's result boundaries available to trusted local policy without becoming
/// tool details, activity metadata, or another model-visible rendering path.
const MAX_TYPED_RESULT_PARTS: usize = 32;
const MAX_TYPED_RESULT_BYTES: usize = 16 * 1024;

/// One MCP tool descriptor returned by `tools/list`.
#[derive(Clone, Debug, PartialEq)]
pub struct McpToolDescriptor {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// Textual result of an MCP `tools/call` response.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct McpToolCallResult {
    pub text: String,
    pub is_error: bool,
    /// Original typed MCP result boundaries, retained only inside this crate
    /// for trusted wrapper-local lineage derivation. `None` means the provider
    /// offered an oversized or malformed part collection.
    pub(crate) typed_parts: Option<Vec<McpToolResultPart>>,
}

/// One raw typed MCP result part. This has crate visibility so only the local
/// MCP wrapper can inspect it; it is intentionally not serializable.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum McpToolResultPart {
    Content(Value),
    StructuredContent(Value),
}

#[derive(Debug)]
pub(super) struct ToolListPage {
    pub(super) tools: Vec<McpToolDescriptor>,
    pub(super) next_cursor: Option<String>,
}

pub(super) fn parse_tool_list(result: &Value) -> Result<ToolListPage, McpError> {
    let tools = result
        .get("tools")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            McpError::Protocol("tools/list result must contain `tools` array".to_string())
        })?
        .iter()
        .map(parse_tool_descriptor)
        .collect::<Result<Vec<_>, _>>()?;
    let next_cursor = result
        .get("nextCursor")
        .and_then(Value::as_str)
        .map(str::to_string);
    Ok(ToolListPage { tools, next_cursor })
}

fn parse_tool_descriptor(value: &Value) -> Result<McpToolDescriptor, McpError> {
    let name = value
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| McpError::Protocol("MCP tool descriptor missing string `name`".to_string()))?
        .to_string();
    let description = value
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let input_schema = value
        .get("inputSchema")
        .cloned()
        .unwrap_or_else(default_object_schema);
    Ok(McpToolDescriptor {
        name,
        description,
        input_schema,
    })
}

pub(super) fn parse_call_tool_result(result: Value) -> McpToolCallResult {
    let is_error = result
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let text = collect_result_text(&result);
    McpToolCallResult {
        text,
        is_error,
        typed_parts: collect_typed_result_parts(&result),
    }
}

fn collect_typed_result_parts(result: &Value) -> Option<Vec<McpToolResultPart>> {
    let mut parts = Vec::new();
    if let Some(content) = result.get("content") {
        for block in content.as_array()? {
            parts.push(McpToolResultPart::Content(block.clone()));
        }
    }
    if let Some(structured) = result.get("structuredContent") {
        parts.push(McpToolResultPart::StructuredContent(structured.clone()));
    }
    if parts.len() > MAX_TYPED_RESULT_PARTS {
        return None;
    }

    let mut total_bytes = 0usize;
    for part in &parts {
        let value = match part {
            McpToolResultPart::Content(value) | McpToolResultPart::StructuredContent(value) => {
                value
            }
        };
        total_bytes = total_bytes.checked_add(serde_json::to_vec(value).ok()?.len())?;
        if total_bytes > MAX_TYPED_RESULT_BYTES {
            return None;
        }
    }
    Some(parts)
}

fn collect_result_text(result: &Value) -> String {
    let mut pieces = Vec::new();
    if let Some(content) = result.get("content").and_then(Value::as_array) {
        for block in content {
            if let Some(text) = block.get("text").and_then(Value::as_str) {
                pieces.push(text.to_string());
            } else if !block.is_null() {
                pieces.push(render_json(block));
            }
        }
    }
    if pieces.is_empty() {
        if let Some(structured) = result.get("structuredContent") {
            pieces.push(render_json(structured));
        } else {
            pieces.push(render_json(result));
        }
    }
    pieces.join("\n")
}

fn default_object_schema() -> Value {
    json!({ "type": "object", "properties": {} })
}

pub(super) fn render_json(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "<unrenderable JSON>".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn call_result_keeps_model_text_while_retaining_structured_parts_privately() {
        let result = parse_call_tool_result(json!({
            "content": [
                {"type": "text", "text": "model-visible first"},
                {"type": "resource", "resource": {"uri": "memory://private"}},
                {"type": "text", "text": "model-visible second"}
            ],
            "structuredContent": {
                "results": [{"qualifiedName": "crate::private::selection"}]
            },
            "isError": false
        }));

        assert_eq!(
            result.text,
            "model-visible first\n{\"resource\":{\"uri\":\"memory://private\"},\"type\":\"resource\"}\nmodel-visible second"
        );
        assert!(!result.is_error);
        assert!(matches!(
            result.typed_parts.as_deref(),
            Some([
                McpToolResultPart::Content(_),
                McpToolResultPart::Content(_),
                McpToolResultPart::Content(_),
                McpToolResultPart::StructuredContent(_),
            ])
        ));
    }

    #[test]
    fn oversized_typed_parts_do_not_survive_the_private_boundary() {
        for content in [
            (0..=MAX_TYPED_RESULT_PARTS)
                .map(|index| json!({"type": "text", "text": format!("part-{index}")}))
                .collect::<Vec<_>>(),
            vec![json!({"type": "text", "text": "x".repeat(MAX_TYPED_RESULT_BYTES)})],
        ] {
            let result = parse_call_tool_result(json!({
                "content": content,
                "isError": false,
            }));

            assert!(!result.text.is_empty());
            assert_eq!(result.typed_parts, None);
        }
    }
}
