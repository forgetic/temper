// SPDX-License-Identifier: MPL-2.0

use crate::schema::SCHEMA_VERSION;
use serde_json::Value;

/// Returns the canonical JSON Schema for the current non-secret Temper config.
///
/// Objects are closed except for provider maps, whose values remain constrained.
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
                ("observability", observability_config_schema()),
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
            (
                "standalone_shutdown_budget_secs",
                positive_integer_schema(
                    "Absolute internal shutdown budget for temper serve standalone, in seconds.",
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

fn observability_config_schema() -> Value {
    closed_object_schema(
        "Operator-visible telemetry and durable activity settings.",
        [("agent_traces", agent_trace_config_schema())],
    )
}

fn agent_trace_config_schema() -> Value {
    closed_object_schema(
        "Agent-session trace capture, retention, quota, and query authorization.",
        [
            (
                "capture",
                enum_string_schema(
                    "Trace capture level.",
                    ["off", "metadata", "transcript", "diagnostic"],
                ),
            ),
            (
                "retention_days",
                bounded_positive_integer_schema(
                    "Number of days completed traces remain eligible for retention.",
                    u32::MAX as u64,
                ),
            ),
            (
                "max_run_bytes",
                bounded_positive_integer_schema(
                    "Hard byte budget for one canonical run.",
                    i64::MAX as u64,
                ),
            ),
            (
                "capture_thinking",
                bool_schema("Allow bounded thinking deltas in diagnostic capture only."),
            ),
            (
                "read_token",
                string_schema(
                    "Named secret reference authorizing transcript-bearing query routes.",
                ),
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
            ("ci_failure_evidence", forge_ci_failure_evidence_schema()),
        ],
    )
}

fn forge_ci_failure_evidence_schema() -> Value {
    let mut schema = closed_object_schema(
        "Authenticated generic source of protected-workflow ordinary CI-failure statements.",
        [
            (
                "endpoint",
                string_schema("Absolute HTTPS or loopback HTTP evidence endpoint."),
            ),
            (
                "issuer",
                string_schema("Authorized signed-statement issuer identity."),
            ),
            (
                "protected_producers",
                string_array_schema("Allowlisted protected producer identities."),
            ),
            (
                "bearer_token",
                string_schema("Named secret for endpoint acquisition authentication."),
            ),
            (
                "hmac_key",
                string_schema("Named secret for statement HMAC-SHA256 verification."),
            ),
        ],
    );
    schema["required"] = Value::Array(
        [
            "endpoint",
            "issuer",
            "protected_producers",
            "bearer_token",
            "hmac_key",
        ]
        .into_iter()
        .map(Value::from)
        .collect(),
    );
    schema
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
                string_schema("Path to a workflow definition JSON/YAML file."),
            ),
            (
                "repos",
                string_array_schema("Repositories to orchestrate, each owner/name."),
            ),
            ("roles", string_array_schema("Workflow roles to drive.")),
            (
                "poll_cadence_secs",
                positive_integer_schema("Poll-backstop cadence in seconds."),
            ),
            (
                "ci_poll_cadence_secs",
                integer_schema(
                    "Dedicated CI-status poll cadence in seconds; bounds webhook-less red repair and green landing detection; 0 disables it.",
                    None,
                ),
            ),
            (
                "ci_missing_grace_secs",
                positive_integer_schema(
                    "Grace period before missing current-head CI is actionable; detection and parking are inactive when ci_poll_cadence_secs is 0.",
                ),
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
                positive_integer_schema("Heartbeat interval in milliseconds."),
            ),
            (
                "max_no_progress_secs",
                positive_integer_schema("Maximum seconds without agent progress."),
            ),
            (
                "max_run_secs",
                positive_integer_schema("Optional independent maximum run duration in seconds."),
            ),
            (
                "graceful_cancellation_grace_secs",
                positive_integer_schema("Cooperative cancellation grace period in seconds."),
            ),
            (
                "forced_termination_grace_secs",
                positive_integer_schema("Forced termination grace period in seconds."),
            ),
            (
                "session_failure_limit",
                bounded_positive_integer_schema(
                    "Terminal agent runs allowed in one durable model session.",
                    32,
                ),
            ),
            (
                "fresh_session_limit",
                integer_schema(
                    "Fresh sessions allowed in one model-failure epoch.",
                    Some(32),
                ),
            ),
            (
                "provider_deferral_limit",
                bounded_positive_integer_schema(
                    "Provider deferrals allowed before human parking.",
                    32,
                ),
            ),
            (
                "provider_deferral_delay_secs",
                positive_integer_schema("Delay before automatic provider recovery."),
            ),
            (
                "model_recovery_slo_secs",
                positive_integer_schema("Wall-clock SLO for one model-failure epoch."),
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
            ("deadlines", agent_deadline_config_schema()),
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

fn agent_deadline_config_schema() -> Value {
    closed_object_schema(
        "First-party model and tool operation deadlines.",
        [
            (
                "tool_timeout_secs",
                positive_integer_schema("Maximum duration of one tool invocation in seconds."),
            ),
            (
                "model_connect_timeout_secs",
                positive_integer_schema("Maximum model connect/first-event wait in seconds."),
            ),
            (
                "model_idle_timeout_secs",
                positive_integer_schema("Maximum wait between model stream events in seconds."),
            ),
            (
                "model_retry_max_attempts",
                bounded_positive_integer_schema("Total attempts allowed for one model turn.", 32),
            ),
            (
                "model_retry_base_delay_ms",
                positive_integer_schema("Base same-turn model retry delay in milliseconds."),
            ),
            (
                "model_retry_max_delay_ms",
                positive_integer_schema("Maximum same-turn model retry delay in milliseconds."),
            ),
            (
                "model_retry_jitter_percent",
                integer_schema("Symmetric retry jitter percentage.", Some(100)),
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
                string_schema("MCP server command to spawn for the bridge."),
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
            ("retention", codebase_memory_retention_config_schema()),
        ],
    )
}

fn codebase_memory_retention_config_schema() -> Value {
    closed_object_schema(
        "Host-controlled bounded retention for obsolete Temper-owned provider projects.",
        [
            ("enabled", bool_schema("Enable worker-owned maintenance.")),
            (
                "max_obsolete_projects",
                integer_schema(
                    "Maximum obsolete ephemeral projects retained by count.",
                    Some(10_000),
                ),
            ),
            (
                "max_age_days",
                bounded_positive_integer_schema("Maximum obsolete project age in days.", 3_650),
            ),
            (
                "maintenance_interval_secs",
                positive_integer_schema("Delay between worker maintenance passes."),
            ),
            (
                "maintenance_timeout_secs",
                bounded_positive_integer_schema(
                    "Absolute provider-operation budget for one pass.",
                    300,
                ),
            ),
            (
                "inventory_page_size",
                bounded_positive_integer_schema(
                    "Maximum records requested per inventory page.",
                    200,
                ),
            ),
            (
                "max_inventory_pages",
                bounded_positive_integer_schema("Maximum inventory pages followed per pass.", 100),
            ),
            (
                "max_deletions_per_run",
                bounded_positive_integer_schema("Maximum provider projects deleted per pass.", 100),
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
                string_schema(
                    "Secret-name reference for provider credentials used by this profile.",
                ),
            ),
            ("deadlines", agent_deadline_config_schema()),
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

fn bounded_positive_integer_schema(description: &'static str, maximum: u64) -> Value {
    schema_object([
        ("description", Value::from(description)),
        ("type", Value::from("integer")),
        ("minimum", Value::from(1)),
        ("maximum", Value::from(maximum)),
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
