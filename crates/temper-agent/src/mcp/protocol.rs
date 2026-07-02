//! MCP descriptor/result parsing and JSON rendering helpers.

use serde_json::{Value, json};

use super::client::McpError;

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
    McpToolCallResult { text, is_error }
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
