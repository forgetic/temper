// SPDX-License-Identifier: MPL-2.0

//! Starter templates emitted by the legacy `temper config init` helper.
//!
//! These are heavily commented, ready-to-edit TOML files at the current
//! [`SCHEMA_VERSION`](crate::SCHEMA_VERSION). Both are parseable as-is (every
//! example value is a placeholder), so `temper check` reports the still-needed
//! values rather than a parse error.

/// A commented starter **config** file.
pub fn config_template() -> String {
    "\
# temper config — non-secret deployment settings.
# Secrets (forge tokens/passwords, LLM credentials) live in the credentials file.
schema_version = 1

[observability.agent_traces]
# Durable traces live below [paths] state_dir, never below a workstream checkout.
# If no durable state directory can be resolved, tracing is disabled with a warning.
capture = \"metadata\"       # off | metadata | transcript | diagnostic
retention_days = 14
max_run_bytes = 50000000
capture_thinking = false    # valid only with capture = \"diagnostic\"
# Optional named secret enabling transcript-bearing query routes. The name and
# availability may be displayed; the token value is never printed.
# read_token = \"agent-trace-read-token\"

[forge]
# Forge backend. Only \"forgejo\" is supported today.
type = \"forgejo\"
url = \"http://localhost:3000\"
# The admin/default user: the key into [forge.users.<admin>] in the credentials
# file whose token becomes the engine's default forge identity.
admin = \"bot\"
# The user whose web-UI password authenticates CI status reads (ADR 0019).
ci_user = \"bot\"

[engine]
# Bind address for the engine HTTP surface. `port` is shorthand for
# 127.0.0.1:<port>; set `bind` for a non-loopback host.
port = 4000
# Workflow definition (JSON). Omit to use the bundled reference-delivery workflow.
# workflow = \"/path/to/my-workflow.json\"
repos = [\"acme/widgets\"]
roles = [\"architect\", \"engineer\", \"code-reviewer\"]
poll_cadence_secs = 300
# Dedicated CI-status backstop for webhook-less terminal red repair and green
# landing detection. Omit for the 60-second default; set 0 to disable. The
# general role poll above remains the full correctness/liveness backstop.
ci_poll_cadence_secs = 60
# Mechanical backstop (label transitions / PR landing). On by default; it does
# not discover red engineer repair work. Omit for the default cadence; set 0 to
# disable.
# mechanical_cadence_secs = 120
lease_ttl_secs = 300
# Optional target-era references into the selected secret source. Values are
# secret *names*, not literal secrets. Directory secret sources use one file per
# name; credentials.toml may use a [secrets] map for local development.
# forge_token = \"forge-engine-token\"
# webhook_secret = \"webhook-secret\"
# Legacy path-based webhook secrets remain supported:
# webhook_secret_file = \"/path/to/webhook-secret\"

[worker]
# Top-level agent workspace root. Per-job scoped roots are created below this as
# <role>/<safe-coordination-key>. Omit to use the XDG state default
# ~/.local/state/temper/workspace ($XDG_STATE_HOME/temper/workspace). A leading
# ~ is expanded at load time.
# workspace = \"~/.local/state/temper/workspace\"
# Distributed topology only — where `temper serve worker` reaches the engine.
# Defaults to http://127.0.0.1:<engine.port>.
# daemon_url = \"http://engine-host:4000\"
# Defaults to the cross-product of engine.repos x engine.roles.
# capabilities = [\"acme/widgets:engineer\"]
# Agent progress must arrive within this interval. Heartbeats do not count as
# progress and must remain strictly more frequent than this deadline. Inspect
# effective values with `temper config show`; inspect live reports at /v1/state.
max_no_progress_secs = 900
# max_run_secs = 7200       # optional independent whole-run ceiling
# Cooperative cancellation is followed by bounded forced process termination.
graceful_cancellation_grace_secs = 10
forced_termination_grace_secs = 5

[agent]
provider = \"anthropic\"
max_iterations = 250
enable_subagents = false

[agent.deadlines]
tool_timeout_secs = 600
model_connect_timeout_secs = 120
model_idle_timeout_secs = 120

# Agent-local codebase-memory MCP tool settings. Auto mode is best-effort: if
# `codebase-memory-mcp` is not installed, agent runs continue without these
# repository-index tools. The default index behavior starts prepared-workspace
# repo indexing in the background.
[agent.tools.codebase_memory]
mode = \"auto\"          # off | auto | required
# command = \"codebase-memory-mcp\"
# args = []
# roles = [\"*\"]          # or selected workflow roles
# index = \"background\"   # off | background | blocking
# startup_timeout_secs = 5
# index_timeout_secs = 30

[agent.providers.anthropic]
# url = \"https://api.anthropic.com\"
models = { main = \"claude-opus-4-8\", investigate = \"claude-haiku-4-5\" }
"
    .to_string()
}

/// A commented starter **credentials** file.
pub fn credentials_template() -> String {
    "\
# temper credentials — secrets. Keep this file out of version control and
# readable only by the temper service user (chmod 600).
schema_version = 1

# One [forge.users.<name>] block per forge identity. The block key doubles as
# the role name for per-role git identities, and is referenced by
# `forge.admin` / `forge.ci_user` in the config file.
[forge.users.agent]
# user = \"agent\"   # forge login; defaults to the block key
password = \"<agent-password>\"
token = \"<agent-rest-token>\"

[forge.users.engineer]
password = \"<engineer-password>\"
token = \"<engineer-rest-token>\"

[forge.users.bot]
password = \"<bot-password>\"
token = \"<bot-rest-token>\"

# Optional target-era named secrets for local development. Directory secret
# sources use one regular file per secret name instead of this TOML map.
# [secrets]
# forge-engine-token = \"<engine-forge-token>\"
# webhook-secret = \"<webhook-hmac-secret>\"
# agent-trace-read-token = \"<trace-query-bearer-token>\"
#
# Structured entries are also accepted:
# [secrets.agent-provider]
# kind = \"provider-credentials\"
# provider = \"anthropic\"
# auth = \"api-key\"
# api_key = \"<provider-api-key>\"
#
# LLM provider secret, matching [agent.providers.<name>] in the config file.
[agent.providers.anthropic]
type = \"oauth\"
access = \"<oauth-access-token>\"
refresh = \"<oauth-refresh-token>\"
expires = 0
# Or point at an existing pi-format auth.json instead of inline tokens:
# auth_file = \"/home/agent/.pi/agent/auth.json\"
"
    .to_string()
}
