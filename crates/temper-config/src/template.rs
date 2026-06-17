// SPDX-License-Identifier: MPL-2.0

//! Starter templates emitted by `temper config init`.
//!
//! These are heavily commented, ready-to-edit TOML files at the current
//! [`SCHEMA_VERSION`](crate::SCHEMA_VERSION). Both are parseable as-is (every
//! example value is a placeholder), so `temper config validate` reports the
//! still-needed values rather than a parse error.

/// A commented starter **config** file.
pub fn config_template() -> String {
    "\
# temper config — non-secret deployment settings.
# Secrets (forge tokens/passwords, LLM credentials) live in the credentials file.
schema_version = 1

[forge]
# Forge backend. Only \"forgejo\" is supported today.
type = \"forgejo\"
url = \"http://localhost:3000\"
# The admin/default user: the key into [forge.users.<admin>] in the credentials
# file whose token becomes the daemon's default forge identity.
admin = \"bot\"
# The user whose web-UI password authenticates CI status reads (ADR 0019).
ci_user = \"bot\"

[engine]
# Bind address for the daemon's HTTP surface. `port` is shorthand for
# 127.0.0.1:<port>; set `bind` for a non-loopback host.
port = 4000
# Workflow definition (JSON). Omit to use the bundled reference-delivery workflow.
# workflow = \"/path/to/my-workflow.json\"
repos = [\"acme/widgets\"]
roles = [\"architect\", \"engineer\", \"code-reviewer\"]
poll_cadence_secs = 300
# Mechanical backstop (label transitions / PR landing). On by default;
# webhooks are the primary reaction path and this is the level-triggered
# safety net. Omit for the default cadence; set 0 to disable.
# mechanical_cadence_secs = 120
lease_ttl_secs = 300
# webhook_secret_file = \"/path/to/webhook-secret\"

[worker]
# Per-job agent workspace root. Omit to use the XDG state default
# ~/.local/state/temper/workspace ($XDG_STATE_HOME/temper/workspace). A leading
# ~ is expanded at load time.
# workspace = \"~/.local/state/temper/workspace\"
# Distributed topology only — where `temper daemon --service worker` reaches the
# engine. Defaults to http://127.0.0.1:<engine.port>.
# daemon_url = \"http://engine-host:4000\"
# Defaults to the cross-product of engine.repos x engine.roles.
# capabilities = [\"acme/widgets:engineer\"]

[agent]
provider = \"anthropic\"
max_iterations = 250
enable_subagents = false

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
