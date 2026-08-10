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
    } else if properties.is_some_and(|properties| properties.contains_key("project")) {
        Some("project")
    } else {
        None
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
        let mut normalizes_project = false;
        if let Some(properties) = properties.as_object_mut() {
            normalizes_project =
                properties.contains_key("repo") || properties.contains_key("project");
            if normalizes_project {
                for key in ["project", "repo"] {
                    properties.insert(
                        key.to_string(),
                        json!({
                            "type": "string",
                            "description": description,
                            "enum": aliases,
                        }),
                    );
                }
            }
        }
        if normalizes_project {
            if let Some(required) = object.get_mut("required").and_then(Value::as_array_mut) {
                required.retain(|value| value != "project" && value != "repo");
            }
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
        "{base}\n\nDecision checkpoint: a bounded successful targeted current-root result is followed by a `Decision anchor`. Use that provider result with the work-item requirements before choosing a dependent refinement, trace, or source read in a later model turn. The anchor is absent for unrelated discovery, failures, truncated or ambiguous output, and unavailable tools; genuinely independent discovery remains parallel-safe.\n\nWorkspace scoped: default project `{}`; accepted `project`/`repo` aliases: {}. Unknown aliases and filesystem paths are rejected.\n\nRead-only wrapper around codebase-memory MCP tool `{}`.",
        scope.primary().canonical_alias,
        scope.documented_aliases().join(", "),
        allowed.mcp_name
    )
}
