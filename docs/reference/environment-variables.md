# Environment variable inventory

This page inventories the environment variables and environment-variable
patterns supported by Temper today. It separates real operator-facing knobs from
process-protocol variables, generated secret files, demo/test harness knobs, and
standard platform variables.

Recommendations use these labels:

- **Keep env** — appropriate because the value is a secret, logging/platform
  convention, or process boundary/protocol value.
- **Move to config/CLI/parameter** — currently supported, but ordinary operators
  should prefer TOML config, command-line flags, or explicit in-process
  parameters.
- **Test/demo only** — keep out of production docs and APIs unless deliberately
  promoted.

Role env suffixes use a role key: uppercase the role id and replace every
non-`[A-Z0-9]` character with `_` (for example `code-reviewer` becomes
`CODE_REVIEWER`).

## Core config and deployment

These are read through the unified `temper` environment snapshot and by the slim
service binaries (`temper-engine`, `temper-worker`) via `temper-config`.

Deployment-shape env overrides have been removed from `temper-config`: the
config and credentials file locations come only from `--config` / `--credentials`
/ `--secrets` (plus the XDG/HOME default location), and every resolved deployment
field (forge URL/token, web-UI and per-role credentials, engine bind/port,
workflow file, worker daemon URL and workspace root) comes only from the TOML
config and credentials files. The former `TEMPER_CONFIG`, `TEMPER_CREDENTIALS`,
`TEMPER_FORGE_URL`, `FORGEJO_URL`, `TEMPER_FORGE_TOKEN`, `FORGEJO_ACCESS_TOKEN`,
`FORGEJO_USERNAME`, `FORGEJO_PASSWORD`, `TEMPER_FORGEJO_{TOKEN,USER,EMAIL}_<ROLE>`,
`TEMPER_WORKFLOW`, `TEMPER_ENGINE_BIND`, `TEMPER_ENGINE_PORT`, `TEMPER_DAEMON_URL`,
and `TEMPER_WORKSPACE` overrides are no longer read and have no effect on
deployment configuration. Only the standard base-directory variables remain (for
default-location discovery and `~` expansion):

| Variable or pattern | Purpose | Precedence / default | Recommendation |
| --- | --- | --- | --- |
| `XDG_CONFIG_HOME` | Base directory for default config/credentials discovery. | Wins over `HOME` for config defaults. | Keep env (standard XDG). |
| `XDG_STATE_HOME` | Base directory for default mutable state/workspace. | Wins over `HOME`; default worker workspace is `$XDG_STATE_HOME/temper/workspace`. | Keep env (standard XDG). |
| `HOME` | Fallback base for config/state defaults and `~` path expansion. | Used when XDG vars are absent; if absent, `~` remains literal and workspace can fall back to `.temper/workspace`. | Keep env (platform convention). |

Note: because `temper-config` resolves a full deployment for every service,
worker/agent-only env names may be read during `temper-engine` startup even when
the engine adapter does not consume the resolved values.

## Logging and platform sink selection

| Variable | Purpose | Behavior | Recommendation |
| --- | --- | --- | --- |
| `RUST_LOG` | `tracing_subscriber::EnvFilter` directives. | Optional; unset or invalid defaults to `info`. Applies to JSON, journald, and stderr sinks. | Keep env (Rust/logging convention). |
| `TEMPER_LOG_FORMAT` | Log sink format override. | Exact token `json` (trimmed, case-insensitive) forces JSON lines to stderr and beats journald auto-detection; unknown values fall back to auto. | Keep env. |
| `JOURNAL_STREAM` | systemd/journald detection. | On Linux, if present and JSON is not forced, logging prefers journald if available. | Keep env as system-provided protocol; do not treat as user knob. |

## Agent, worker, and provider process boundaries

The worker spawns `temper agent` (the hidden, function-call-like coding agent)
once per job. Every **non-secret** input is a command-line flag; exactly **one**
secret crosses as an environment variable, the provider credential.

### `temper agent` command-line flags (non-secret)

The worker sets these when it spawns the agent; an operator can pass them by hand
for debugging. The worker reads model/provider/iteration knobs from the resolved
deployment config and renders them onto the command line.

| Flag | Purpose | Default |
| --- | --- | --- |
| `--context <FILE>` | Worker-written `WorkspaceContext` JSON path (required). | — (set per job by the worker) |
| `--result <FILE>` | `WorkspaceResult` JSON path the agent must write (required). | — (set per job by the worker) |
| `--workspace <DIR>` | Prepared checkout / workspace root. | process cwd |
| `--provider <anthropic\|chatgpt\|deepseek>` | Provider adapter to use. | `chatgpt` |
| `--model <ID>` | Main model id. | provider built-in default |
| `--investigate-model <ID>` | Cheaper read-only subagent model id. | provider built-in default |
| `--provider-url <URL>` | Provider base-URL override (e.g. a local fake LLM). | provider built-in URL |
| `--max-iterations <N>` | Maximum model/tool iterations. | compiled default |
| `--subagents <on\|off>` | Enable investigate/read-only subagents. | `off` |
| `--deadline-unix-seconds <N>` | Job deadline / lease-expiry hint for the checkpoint backstop. | unset |
| `--checkpoint-interval <DURATION>` | Checkpoint backstop cadence, e.g. `60s`, `5m`. | `300s` |
| `--capture-dir <DIR>` | Optional prompt-overlay / debug-capture dir (was `ANVIL_CONFIG_DIR`). | `$XDG_CONFIG_HOME/anvil`, else `$HOME/.config/anvil` |

### Secret env var consumed by `temper agent`

| Variable | Purpose | Shape |
| --- | --- | --- |
| `TEMPER_AGENT_PROVIDER_CREDENTIALS_JSON` | The provider credential, the agent's **only** environment input. The worker builds it from the resolved provider credential (api-key, or OAuth tokens) and injects it into the spawned agent process. | `{"type":"api-key","api_key":"…"}` or `{"type":"oauth","access_token":"…","refresh_token":"…","expires_at_unix_seconds":N}` |

For an OAuth credential the agent materializes the tokens into a temporary
pi-format `auth.json` its OAuth loader reads (and refreshes) for the run; no
worker-written auth file or `TEMPER_AGENTS_*` model/url env crosses the boundary
anymore.

### Git checkpoint identity (no env)

The agent commits + pushes checkpoints against the prepared checkout. The worker
configures the git author identity (`user.name`/`user.email`) and the push
credential (`http.extraheader`) in each writable repo's **local `.git/config`**
before spawning the agent, so the agent needs no `TEMPER_FORGEJO_*_<ROLE>` env
and the push token never reaches the agent's argv or environment.

### Still-internal child env

| Variable | Purpose | Recommendation |
| --- | --- | --- |
| `GIT_TERMINAL_PROMPT` | Disable interactive git prompts for child git processes. | Keep internal child env. |
| `PATH` | Resolves bare `git`, `temper-agent`, and external agent commands. | Document as deployment prerequisite; prefer absolute paths for hermetic services. |

The agent inherits the parent process environment and then has the one secret
provider-credential var overlaid. If strict hermeticity is required, use
`env_clear`/allowlists at the child-process boundary and re-add only `PATH`,
`HOME`, and `TEMPER_AGENT_PROVIDER_CREDENTIALS_JSON`.

## Interaction service and process responders

| Variable or pattern | Purpose | Behavior | Recommendation |
| --- | --- | --- | --- |
| `TEMPER_INTERACTION_SPEC` | Fallback path for `temper-interaction repl/serve --spec`. | CLI flag wins; trim-empty env is ignored. | Move to CLI/config. |
| `TEMPER_INTERACTION_BINDINGS` | Fallback path for `--bindings`. | CLI flag wins; trim-empty env is ignored. | Move to CLI/config. |
| `TEMPER_INTERACTION_PROFILE` | Fallback profile for `temper-interaction repl --profile`. | CLI flag wins; trim-empty env is ignored. | Move to CLI/config. |
| `service.token_env` binding field | Names an env var containing the interaction HTTP bearer token. | Optional on loopback; required for non-loopback auth when configured. | Keep env for secret value, but keep name in bindings/config. |
| `profiles.*.human_token_env` binding field | Names env var containing the human-side Forge token. | Required field; referenced env value must be non-empty. | Keep env for secret value, name in bindings/config. |
| `profiles.*.agent_token_env` binding field | Names env var containing the agent-side Forge token. | Required field; referenced env value must be non-empty. | Keep env for secret value, name in bindings/config. |
| `responders.*.env_allowlist[]` binding field | Env names copied into external responder subprocesses. | Missing names are silently dropped; child env is otherwise cleared. | Keep allowlist model; consider separate required env list. |
| `ProcessCall.env` / `ProcessCall.clear_env` | Generic child-process env export capability in `temper-engine-io`. | Caller-provided arbitrary keys; `clear_env=true` starts from empty env. | Treat as capability; document fixed keys at call sites. |

## Provisioning, generated secrets, and validator tools

| Variable or pattern | Purpose | Behavior | Recommendation |
| --- | --- | --- | --- |
| `TEMPER_FORGEJO_ADMIN_TOKEN` | Admin token for `temper provision-forgejo` / testing provisioner. | Required, non-empty, never accepted on argv. | Keep env for secret. |
| `TEMPER_WORKFLOW_FILE` | Workflow file fallback for provisioning/testing workers. | `--workflow` wins; empty env ignored; default may be bundled workflow. | Move to CLI/config. |
| `TEMPER_FORGEJO_TOKEN` | Validator or testing-worker Forgejo token. | Required by `validate-reference-delivery` and by `temper-testing-worker --backend forgejo`. | Keep env for secret. |
| `TEMPER_FORGEJO_USERNAME` | Testing-worker Forgejo web-UI username. | Optional; only useful with password. | Test-only secret-pair env. |
| `TEMPER_FORGEJO_PASSWORD` | Testing-worker Forgejo web-UI password. | Optional; only useful with username. | Test-only secret-pair env. |
| `TEMPER_WAKE_DEBOUNCE_MS` | Testing-worker local wake debounce override. | Optional positive milliseconds; blank/invalid/zero uses default. | Test-only; move to explicit test config if reused. |
| `TEMPER_FORGEJO_CI_DIAGNOSTICS` | Testing-worker Forgejo CI web-UI diagnostics. | Any non-blank value enables diagnostics. | Test-only. |

`provision-forgejo --out` writes a POSIX-sourceable, mode-0600 `roles.env` file:

| Generated variable | Purpose | Recommendation |
| --- | --- | --- |
| `TEMPER_FORGEJO_OWNER` | Convenience provisioned owner/org. | Move to explicit config; do not rely on this for multi-repo runs. |
| `TEMPER_FORGEJO_REPO` | Convenience provisioned repo. | Move to explicit config; not authoritative for multi-repo runs. |
| `TEMPER_FORGEJO_USER_<ROLE>` | Provisioned role login. | Move durable value to credentials. |
| `TEMPER_FORGEJO_TOKEN_<ROLE>` | Provisioned role token. | Keep env only for generated secret handoff; durable value belongs in credentials/secret manager. |
| `TEMPER_FORGEJO_PASSWORD_<ROLE>` | Provisioned role web/git password. | Keep env only for generated secret handoff. |
| `TEMPER_FORGEJO_BOT_USER` | Automation bot login. | Move to credentials/config. |
| `TEMPER_FORGEJO_BOT_TOKEN` | Automation bot REST token. | Keep generated secret; pass to daemon as `FORGEJO_ACCESS_TOKEN` today. |
| `TEMPER_FORGEJO_BOT_PASSWORD` | Automation bot web-UI password. | Keep generated secret; pass to daemon as `FORGEJO_PASSWORD` today. |

`TEMPER_FORGEJO_EMAIL_<ROLE>` is consumed by worker/agent paths but is not
currently generated in `roles.env`; if role emails should be operator-visible,
either emit it or remove optional export comments in launchers.

## Demo launcher variables

The `examples/basic-delivery` and `examples/reference-delivery` launchers source
`config/temper.env` plus gitignored `secrets/.env`. These are demo shell knobs,
not core library APIs.

| Variable or pattern | Purpose | Recommendation |
| --- | --- | --- |
| `OWNER`, `NAME`, `DEFAULT_BRANCH` | Demo repository identity/default branch. | Demo-only shell config. |
| `WORKFLOW_FILE`, `INTAKE_TITLE`, `INTAKE_BODY_FILE` | Demo workflow and seeded intake issue. | Demo-only shell config. |
| `BASE_URL`, `DAEMON_BIND`, `WEBHOOK_URL` | Demo Forgejo and daemon endpoints. | Demo-only shell config. |
| `DAEMON_POLL_CADENCE_SECS`, `DAEMON_MECHANICAL_CADENCE_SECS`, `DAEMON_LEASE_TTL_SECS`, `RUN_SECS` | Demo daemon cadences/run backstop. | Demo-only shell config; core values belong in TOML config. |
| `TEMPER_FORGEJO_GOMAXPROCS` | Demo CPU cap for spawned Go Forgejo/runner processes. | Demo-only; exports `GOMAXPROCS`. |
| `TEMPER_FORGEJO_BINARY`, `TEMPER_FORGEJO_RUNNER_BINARY` | Demo/fixture paths to pinned Forgejo binaries. | Demo/test-only. |
| `TEMPER_RUN_BIN`, `TEMPER_BUILD_PACKAGE`, `TEMPER_SKIP_BUILD` | Demo Temper binary/build controls. | Demo-only. |
| `TEMPER_RUN_AUTH`, `RUN_MAX_ITERATIONS` | Demo coding-agent provider/auth and max iterations. | Move real deployments to config/CLI. |
| `TEMPER_WORKSPACE_ROOT` | Demo workspace-root override. | Demo-only. |
| `TEMPER_BASIC_DELIVERY_SCRIPT_DIR`, `TEMPER_BASIC_DELIVERY_ORIGINAL`, `TEMPER_BASIC_DELIVERY_SNAPSHOT` | Basic demo re-exec/snapshot internals. | Demo-internal. |
| `TEMPER_REFERENCE_DELIVERY_SCRIPT_DIR`, `TEMPER_REFERENCE_DELIVERY_ORIGINAL`, `TEMPER_REFERENCE_DELIVERY_SNAPSHOT` | Reference demo re-exec/snapshot internals. | Demo-internal. |
| `REPOS`, `SERVED_ROLES`, `CROSS_REPO_INTAKE`, `CROSS_REPO_INTAKE_TITLE` | Reference-delivery multi-repo/role/cross-repo controls. | Demo-only shell config. |
| `GOMAXPROCS`, `GITEA_WORK_DIR` | Env passed to Forgejo/runner child processes. | Child-process implementation details. |

Forgejo Actions workflows use GitHub-compatible env names supplied by Forgejo
Actions: `GITHUB_API_URL`, `GITHUB_REPOSITORY`, `GITHUB_SHA`, and `GITHUB_TOKEN`.
The checked-in demo CI files also echo/use `GITHUB_SHA`.

## Test-only variables

| Variable or pattern | Purpose |
| --- | --- |
| `TEMPER_TEST_CONVERGENCE_TIMEOUT_SECS` | Overrides ignored Forgejo e2e convergence timeouts. |
| `TEMPER_TESTING_DAEMON_WORKER_BIN` | Overrides root e2e path to `temper-testing-daemon-worker`. |
| `TEMPER_E2E_GIT_USER` | Required git identity for daemon test worker. |
| `TEMPER_E2E_GIT_TOKEN` | Optional git push token for daemon test worker. |
| `TEMPER_ANTHROPIC_OAUTH`, `TEMPER_CHATGPT_OAUTH`, `TEMPER_DEEPSEEK_REQUEST_ORACLE` | Opt-in gates for ignored live provider/request-oracle tests. |
| `TEMPER_SIM_SEED_BASE` | Base seed for simulation fresh-seed batch; invalid/unset uses default. |
| `TEMPER_INTERACTION_PROCESS_RESPONDER_TEST_ALLOWED`, `TEMPER_INTERACTION_PROCESS_RESPONDER_TEST_BLOCKED` | Interaction process-responder env filtering tests. |
| `TEMPER_RUNNER_ROLE_DECISION_ALLOWED`, `TEMPER_RUNNER_ROLE_DECISION_BLOCKED` | Legacy role-selector process env filtering tests; not a production role-agent interface. |
| `SMITH_FAKE_AGENT_VERDICT`, `SMITH_FAKE_AGENT_FILE`, `SMITH_FAKE_AGENT_CONTENT`, `SMITH_FAKE_AGENT_BODY` | `temper-worker` fake-agent test controls. |
| `TEMPER_FORGEJO_BINARY`, `TEMPER_FORGEJO_RUNNER_BINARY` | Ignored Forgejo fixture/demo binary override paths. |
| `FORGEJO_DEFAULT_REPO` | Scrubbed by e2es for hermeticity; no current production reader found in this repository. |
| `CARGO_*` compile-time/test variables | Cargo-provided test mechanics; excluded from supported runtime inventory. |
| `TMPDIR`/`TEMP`/`TMP` platform temp vars | Indirectly affect `std::env::temp_dir()` in tests only. |

## Script variables

`scripts/check-rust-file-size.sh` supports these overrides:

| Variable | Default | Purpose |
| --- | --- | --- |
| `RUST_FILE_SIZE_HARD_MAX_LOC` | `800` | Hard nonblank LOC limit. |
| `RUST_FILE_SIZE_JUSTIFICATION_LOC` | `600` | LOC threshold requiring a justification entry. |
| `RUST_FILE_SIZE_ALLOWLIST` | `scripts/rust-file-size-allowlist.txt` | Hard-rule allowlist path. |
| `RUST_FILE_SIZE_JUSTIFICATIONS` | `scripts/rust-file-size-justifications.txt` | Justifications file path. |

`check-no-ambient-env.sh` has an internal `AMBIENT_ENV_ALLOWLIST` shell variable,
but exporting that name before running the script has no effect.

## Summary: conversion priorities

1. **Convert non-secret deployment overrides to TOML config or CLI flags**:
   `TEMPER_FORGE_URL`, `FORGEJO_URL`, `TEMPER_WORKFLOW`, `TEMPER_ENGINE_BIND`,
   `TEMPER_ENGINE_PORT`, `TEMPER_DAEMON_URL`, and `TEMPER_WORKSPACE`.
2. **Keep secrets out of argv, but prefer credentials files/secret managers over
   ambient shell env**: `TEMPER_FORGE_TOKEN`, `FORGEJO_ACCESS_TOKEN`,
   `FORGEJO_PASSWORD`, per-role `TEMPER_FORGEJO_TOKEN_<ROLE>`, and provider API
   keys.
3. **Keep process-protocol env only at process boundaries**:
   `TEMPER_CODING_WORKSPACE_CONTEXT`, `TEMPER_CODING_WORKSPACE_RESULT`, provider
   env passed to spawned agents, and per-role checkpoint identity env. If the
   protocol evolves, move deadline/checkpoint settings and identity metadata into
   explicit context fields.
4. **Demote test-looking ambient provider knobs**: `ANVIL_TEST_PROVIDER_BASE_URL`,
   `TEMPER_AGENTS_*_TOKEN_URL`, and live-test gates should remain test-only or
   be renamed/promoted deliberately.
5. **Do not document demo shell config as core API**: keep the example launchers'
   variables in example docs only.
