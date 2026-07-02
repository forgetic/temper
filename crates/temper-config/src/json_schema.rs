// SPDX-License-Identifier: MPL-2.0

//! JSON Schema export for the current non-secret Temper config model.

use crate::schema::SCHEMA_VERSION;
use serde_json::Value;

/// Returns the canonical JSON Schema for the current Temper `config.toml` model.
///
/// The schema mirrors [`crate::Config`] and its nested non-secret sections. Object
/// sections use `additionalProperties: false` wherever serde currently applies
/// `deny_unknown_fields`; provider maps keep arbitrary provider names while
/// constraining each provider profile's value shape.
pub fn config_json_schema() -> Value {
    schema_object([
        (
            "$schema",
            Value::from("https://json-schema.org/draft/2020-12/schema"),
        ),
        (
            "$id",
            Value::from("https://temper.local/schemas/config.schema.json"),
        ),
        ("title", Value::from("Temper config.toml")),
        (
            "description",
            Value::from(
                "JSON Schema for the non-secret Temper config.toml file model accepted by this build.",
            ),
        ),
        ("type", Value::from("object")),
        ("additionalProperties", Value::Bool(false)),
        (
            "$defs",
            property_map([(
                "credentials_named_secrets",
                credentials_named_secrets_schema(),
            )]),
        ),
        (
            "required",
            Value::Array(vec![Value::from("schema_version")]),
        ),
        (
            "properties",
            property_map([
                ("schema_version", schema_version_schema()),
                ("deployment", deployment_config_schema()),
                ("workflow", workflow_config_schema()),
                ("paths", paths_config_schema()),
                ("forge", forge_config_schema()),
                ("engine", engine_config_schema()),
                ("worker", worker_config_schema()),
                ("agent", agent_config_schema()),
            ]),
        ),
    ])
}

fn schema_version_schema() -> Value {
    schema_object([
        (
            "description",
            Value::from("Config schema version accepted by this Temper build."),
        ),
        ("type", Value::from("integer")),
        ("const", Value::from(SCHEMA_VERSION)),
    ])
}

fn credentials_named_secrets_schema() -> Value {
    object_schema(
        "Credentials TOML named secrets map. Compact raw values are accepted under `[secrets]`; structured `[secrets.<name>]` tables may carry target-era metadata plus one payload field such as `token`, `value`, `secret`, `key`, or `api_key`.",
        Vec::<(&'static str, Value)>::new(),
        schema_object([(
            "oneOf",
            Value::Array(vec![
                string_schema("Raw named secret payload."),
                closed_object_schema(
                    "Structured named secret payload.",
                    [
                        ("kind", string_schema("Semantic secret kind.")),
                        ("token", string_schema("Forge token payload.")),
                        ("value", string_schema("Generic secret payload.")),
                        ("secret", string_schema("Generic secret payload alias.")),
                        ("key", string_schema("API-key payload alias.")),
                        ("api_key", string_schema("Provider API-key payload.")),
                        ("provider", string_schema("Provider name metadata.")),
                        ("auth", string_schema("Provider auth metadata.")),
                    ],
                ),
            ]),
        )]),
    )
}

fn deployment_config_schema() -> Value {
    closed_object_schema(
        "Target-era deployment metadata.",
        [
            ("name", string_schema("Optional deployment name metadata.")),
            (
                "topology",
                enum_string_schema(
                    "Deployment topology declaration. Runtime branching is not enabled yet.",
                    ["standalone", "distributed"],
                ),
            ),
        ],
    )
}

fn workflow_config_schema() -> Value {
    closed_object_schema(
        "Target-era workflow settings.",
        [(
            "file",
            string_schema("Path to a workflow definition file, resolved relative to config.toml."),
        )],
    )
}

fn paths_config_schema() -> Value {
    closed_object_schema(
        "Target-era runtime path settings.",
        [
            (
                "state_dir",
                string_schema("Mutable state directory, resolved relative to config.toml."),
            ),
            (
                "workspace_dir",
                string_schema("Top-level worker workspace root, resolved relative to config.toml."),
            ),
        ],
    )
}

fn forge_config_schema() -> Value {
    closed_object_schema(
        "Forge backend and connection settings.",
        [
            (
                "type",
                string_schema("Forge backend kind; only forgejo is supported today."),
            ),
            (
                "url",
                string_schema("Forge base URL, for example http://localhost:3000."),
            ),
            (
                "admin",
                string_schema("Default forge user key from credentials.toml."),
            ),
            (
                "ci_user",
                string_schema(
                    "Forge user key whose web UI credentials authenticate CI status reads.",
                ),
            ),
        ],
    )
}

fn engine_config_schema() -> Value {
    closed_object_schema(
        "Orchestrator settings.",
        [
            ("bind", string_schema("Full host:port bind address.")),
            (
                "port",
                integer_schema(
                    "Convenience port for binding 127.0.0.1:<port>.",
                    Some(65535),
                ),
            ),
            (
                "workflow",
                string_schema("Path to a workflow definition JSON file."),
            ),
            (
                "repos",
                string_array_schema("Repositories to orchestrate, each owner/name."),
            ),
            ("roles", string_array_schema("Workflow roles to drive.")),
            (
                "poll_cadence_secs",
                integer_schema("Poll-backstop cadence in seconds.", None),
            ),
            (
                "mechanical_cadence_secs",
                integer_schema(
                    "Mechanical-backstop cadence in seconds; 0 disables it.",
                    None,
                ),
            ),
            (
                "lease_ttl_secs",
                integer_schema("Lease TTL in seconds.", None),
            ),
            (
                "daemon_id",
                string_schema("Stable daemon identity used for lease ownership."),
            ),
            (
                "forge_token",
                string_schema("Secret-name reference for the engine/default Forge API token."),
            ),
            (
                "webhook_secret",
                string_schema("Secret-name reference for the Forgejo webhook HMAC secret."),
            ),
            (
                "webhook_secret_file",
                string_schema("Path to the Forgejo webhook HMAC secret file."),
            ),
        ],
    )
}

fn worker_config_schema() -> Value {
    closed_object_schema(
        "Worker settings.",
        [
            (
                "workspace",
                string_schema("Top-level directory for per-job agent workspaces."),
            ),
            ("worker_id", string_schema("Stable worker identity.")),
            (
                "daemon_url",
                string_schema("Engine URL for distributed worker long-polling."),
            ),
            (
                "git_base_url",
                string_schema("Git base URL the agent pushes branches to."),
            ),
            (
                "max_concurrent_jobs",
                integer_schema(
                    "Maximum jobs run by one worker at once.",
                    Some(4_294_967_295),
                ),
            ),
            (
                "poll_wait_ms",
                integer_schema("Long-poll wait in milliseconds.", None),
            ),
            (
                "heartbeat_interval_ms",
                integer_schema("Heartbeat interval in milliseconds.", None),
            ),
            (
                "capabilities",
                string_array_schema("Explicit owner/name:role capabilities."),
            ),
            (
                "pools",
                array_schema(
                    "Target-era named worker capability classes.",
                    worker_pool_config_schema(),
                ),
            ),
        ],
    )
}

fn worker_pool_config_schema() -> Value {
    closed_object_schema(
        "Target-era named worker capability class.",
        [
            ("name", string_schema("Unique worker pool name.")),
            (
                "roles",
                string_array_schema("Workflow roles this pool runs."),
            ),
            (
                "repos",
                string_array_schema("Repositories this pool runs, each owner/name."),
            ),
            (
                "max_concurrent_jobs",
                positive_integer_schema("Maximum jobs run concurrently by a worker in this pool."),
            ),
            (
                "agent_profile",
                string_schema("Target-era agent profile name used by this pool."),
            ),
            (
                "worker_token",
                string_schema("Secret-name reference for future worker authentication."),
            ),
        ],
    )
}

fn agent_config_schema() -> Value {
    closed_object_schema(
        "Coding agent provider, model, and limit settings.",
        [
            (
                "provider",
                string_schema("Provider profile key under agent.providers."),
            ),
            (
                "max_iterations",
                integer_schema("Maximum model iterations per job.", None),
            ),
            (
                "enable_subagents",
                bool_schema("Enable the in-workspace investigate sub-agent tool."),
            ),
            (
                "config_dir",
                string_schema("Optional agent config directory for prompt overlays."),
            ),
            ("tools", agent_tools_config_schema()),
            (
                "providers",
                object_schema(
                    "Provider profiles keyed by provider name.",
                    Vec::<(&'static str, Value)>::new(),
                    agent_provider_config_schema(),
                ),
            ),
            (
                "profiles",
                object_schema(
                    "Target-era agent profiles keyed by profile name.",
                    Vec::<(&'static str, Value)>::new(),
                    agent_profile_config_schema(),
                ),
            ),
        ],
    )
}

fn agent_tools_config_schema() -> Value {
    closed_object_schema(
        "Agent-local non-secret tool configuration.",
        [("codebase_memory", codebase_memory_tool_config_schema())],
    )
}

fn codebase_memory_tool_config_schema() -> Value {
    closed_object_schema(
        "Codebase-memory MCP tool process-boundary settings.",
        [
            (
                "mode",
                enum_string_schema("Tool mode.", ["off", "auto", "required"]),
            ),
            (
                "command",
                string_schema("MCP server command to spawn in a future bridge."),
            ),
            ("args", string_array_schema("Additional command arguments.")),
            (
                "roles",
                string_array_schema(
                    "Workflow roles that receive this tool; `*` matches all roles.",
                ),
            ),
            (
                "index",
                enum_string_schema("Indexing behavior.", ["off", "background", "blocking"]),
            ),
            (
                "startup_timeout_secs",
                positive_integer_schema("Startup timeout in seconds."),
            ),
            (
                "index_timeout_secs",
                positive_integer_schema("Indexing timeout in seconds."),
            ),
        ],
    )
}

fn agent_profile_config_schema() -> Value {
    closed_object_schema(
        "Target-era named agent execution profile.",
        [
            (
                "command",
                string_array_schema("Agent command argv, for example [\"temper\", \"agent\"]."),
            ),
            (
                "provider",
                enum_string_schema(
                    "Provider kind for this profile.",
                    ["anthropic", "deepseek", "chatgpt"],
                ),
            ),
            ("model", string_schema("Main model selection.")),
            (
                "investigate_model",
                string_schema("Model for read-only investigate sub-agents."),
            ),
            (
                "provider_url",
                string_schema("Optional provider base URL override."),
            ),
            (
                "max_iterations",
                positive_integer_schema("Maximum model iterations per job."),
            ),
            (
                "subagents",
                bool_schema("Enable the in-workspace investigate sub-agent tool."),
            ),
            (
                "credential",
                string_schema("Secret-name reference for future provider credentials."),
            ),
        ],
    )
}

fn agent_provider_config_schema() -> Value {
    closed_object_schema(
        "Non-secret provider profile settings.",
        [
            ("url", string_schema("Optional provider base URL override.")),
            ("models", model_map_schema()),
        ],
    )
}

fn model_map_schema() -> Value {
    closed_object_schema(
        "Provider model selections.",
        [
            ("main", string_schema("Main coding model.")),
            (
                "investigate",
                string_schema("Model for read-only investigate sub-agents."),
            ),
        ],
    )
}

fn closed_object_schema(
    description: &'static str,
    properties: impl IntoIterator<Item = (&'static str, Value)>,
) -> Value {
    object_schema(description, properties, Value::Bool(false))
}

fn object_schema(
    description: &'static str,
    properties: impl IntoIterator<Item = (&'static str, Value)>,
    additional_properties: Value,
) -> Value {
    schema_object([
        ("description", Value::from(description)),
        ("type", Value::from("object")),
        ("additionalProperties", additional_properties),
        ("properties", property_map(properties)),
    ])
}

fn string_schema(description: &'static str) -> Value {
    primitive_schema(description, "string")
}

fn bool_schema(description: &'static str) -> Value {
    primitive_schema(description, "boolean")
}

fn enum_string_schema(
    description: &'static str,
    values: impl IntoIterator<Item = &'static str>,
) -> Value {
    schema_object([
        ("description", Value::from(description)),
        ("type", Value::from("string")),
        (
            "enum",
            Value::Array(values.into_iter().map(Value::from).collect()),
        ),
    ])
}

fn primitive_schema(description: &'static str, kind: &'static str) -> Value {
    schema_object([
        ("description", Value::from(description)),
        ("type", Value::from(kind)),
    ])
}

fn integer_schema(description: &'static str, maximum: Option<u64>) -> Value {
    let mut fields = vec![
        ("description", Value::from(description)),
        ("type", Value::from("integer")),
        ("minimum", Value::from(0)),
    ];
    if let Some(maximum) = maximum {
        fields.push(("maximum", Value::from(maximum)));
    }
    schema_object(fields)
}

fn positive_integer_schema(description: &'static str) -> Value {
    schema_object([
        ("description", Value::from(description)),
        ("type", Value::from("integer")),
        ("minimum", Value::from(1)),
    ])
}

fn string_array_schema(description: &'static str) -> Value {
    array_schema(
        description,
        schema_object([("type", Value::from("string"))]),
    )
}

fn array_schema(description: &'static str, items: Value) -> Value {
    schema_object([
        ("description", Value::from(description)),
        ("type", Value::from("array")),
        ("items", items),
    ])
}

fn property_map(properties: impl IntoIterator<Item = (&'static str, Value)>) -> Value {
    let mut object = serde_json::Map::new();
    for (name, schema) in properties {
        object.insert(name.to_string(), schema);
    }
    Value::Object(object)
}

fn schema_object(fields: impl IntoIterator<Item = (&'static str, Value)>) -> Value {
    let mut object = serde_json::Map::new();
    for (name, value) in fields {
        object.insert(name.to_string(), value);
    }
    Value::Object(object)
}
