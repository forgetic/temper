use serde_json::{Map, Value, json};

use super::AllowedCodebaseMemoryTool;
use super::scope::WorkspaceScope;

pub(super) fn default_project_key(mcp_name: &str, input_schema: &Value) -> Option<&'static str> {
    if mcp_name == "list_projects" {
        return None;
    }
    let properties = input_schema.get("properties").and_then(Value::as_object);
    if properties.is_some_and(|properties| properties.contains_key("repo")) {
        Some("repo")
    } else {
        Some("project")
    }
}

pub(super) fn scoped_parameters(
    input_schema: &Value,
    allowed: AllowedCodebaseMemoryTool,
    scope: &WorkspaceScope,
) -> Value {
    if allowed.mcp_name == "list_projects" {
        return json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false,
        });
    }

    let mut schema = input_schema.clone();
    if !schema.is_object() {
        schema = json!({ "type": "object", "properties": {} });
    }
    let aliases = scope.documented_aliases();
    let description = format!(
        "Workspace project alias. Omit to use the primary repo `{}`. Accepted aliases: {}. Filesystem paths are rejected.",
        scope.primary().canonical_alias,
        aliases.join(", ")
    );
    if let Some(object) = schema.as_object_mut() {
        object
            .entry("type".to_string())
            .or_insert_with(|| Value::String("object".to_string()));
        let properties = object
            .entry("properties".to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        if let Some(properties) = properties.as_object_mut() {
            properties.insert(
                "project".to_string(),
                json!({
                    "type": "string",
                    "description": description,
                    "enum": aliases,
                }),
            );
        }
    }
    schema
}

pub(super) fn description_for(
    allowed: AllowedCodebaseMemoryTool,
    server_description: &str,
    scope: &WorkspaceScope,
) -> String {
    let base = match server_description.trim() {
        "" => format!("Call codebase-memory MCP tool `{}`.", allowed.mcp_name),
        description => description.to_string(),
    };
    format!(
        "{base}\n\nWorkspace scoped: default project `{}`; accepted `project`/`repo` aliases: {}. Unknown aliases and filesystem paths are rejected.\n\nRead-only wrapper around codebase-memory MCP tool `{}`.",
        scope.primary().canonical_alias,
        scope.documented_aliases().join(", "),
        allowed.mcp_name
    )
}
