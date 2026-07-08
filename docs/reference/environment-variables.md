# Environment variable inventory

Temper officially supports as few environment variables as possible. This page
is the authoritative inventory. It leads with the **official supported set** —
the only environment variables a production deployment is expected to use — then
documents the narrow logging exception, the single agent process-boundary
secret, the env names that live only in config/bindings, and finally the
test/demo-support knobs. A closing section records the legacy deployment/agent
variables that have been **removed** and what replaced them.

This page is aligned with the "Official environment variables" section of the
long-term UX/config spec. If the two disagree, the spec wins.

## Official supported environment variables

These are the only environment variables operators are expected to set for a
production deployment.

### External / standard variables Temper honors

| Variable | Purpose |
| --- | --- |
| `HOME` | Derives default config/state paths and `~` expansion when XDG vars are absent. |
| `XDG_CONFIG_HOME` | Base directory for default config/credentials discovery (wins over `HOME`). |
| `XDG_STATE_HOME` | Base directory for default mutable state/workspace (default worker workspace is `$XDG_STATE_HOME/temper/workspace`). |
| `CREDENTIALS_DIRECTORY` | systemd credential directory; secret source when `--secrets` is absent. |
| `JOURNAL_STREAM` | systemd/journald detection for log-sink selection. |
| `RUST_LOG` | `tracing_subscriber::EnvFilter` directives, e.g. `info` or `temper=debug`; unset/invalid defaults to `info`. |
| `NO_COLOR` | When set to any non-empty value, disables ANSI color in human stderr output regardless of TTY (<https://no-color.org>). |

These are read only at explicit boundaries: `temper-config` resolves base
directories (`HOME` / `XDG_*` / `CREDENTIALS_DIRECTORY`) from the snapshot the
binary captures at startup; the logging crate reads `RUST_LOG` / `JOURNAL_STREAM`
/ `NO_COLOR` at its own init boundary.

For systemd deployments, prefer `LoadCredential=` over exported secret env vars.
When `--secrets` is omitted, systemd's `CREDENTIALS_DIRECTORY` becomes the
selected secret source; it may contain `credentials.toml`, one file per named
secret such as `webhook-secret`, or both. The service command should still pass
non-secret deployment selection on argv, for example
`temper --config /etc/temper/config.toml serve engine`.

### Temper-specific variable

| Variable | Purpose | Shape |
| --- | --- | --- |
| `TEMPER_AGENT_PROVIDER_CREDENTIALS_JSON` | The provider credential — the **only** environment input the spawned `temper agent` reads. `temper serve worker` builds it from the resolved provider credential and injects it into the child agent process. It is not a deployment-configuration mechanism. | `{"type":"api-key","api_key":"…"}` or `{"type":"oauth","access_token":"…","refresh_token":"…","expires_at_unix_seconds":N}` |

### Logging-format exception

| Variable | Purpose | Behavior |
| --- | --- | --- |
| `TEMPER_LOG_FORMAT` | Log-sink format override read by the logging crate. | Exact token `json` (trimmed, case-insensitive) forces JSON lines to stderr and beats journald auto-detection; any other/unset value falls back to auto-detection. |

`TEMPER_LOG_FORMAT` predates this inventory and is a pure logging-boundary
convenience (it sets a format, not deployment state), so it is kept and
documented here rather than removed. It is the only `TEMPER_*` deployment-time
variable outside the agent credential above.

## Agent / worker process boundary

The worker spawns `temper agent` (the hidden, function-call-like coding agent)
once per job. Every **non-secret** input is a command-line flag; exactly **one**
secret crosses as an environment variable — `TEMPER_AGENT_PROVIDER_CREDENTIALS_JSON`
(above).

### `temper agent` command-line flags (non-secret)

The worker sets these when it spawns the agent; an operator can pass them by hand
for debugging. The worker reads model/provider/iteration knobs from the resolved
deployment config and renders them onto the command line — none of them are
environment variables.

| Flag | Purpose | Default |
| --- | --- | --- |
| `--context <FILE>` | Worker-written `WorkspaceContext` JSON path (required). | — (set per job by the worker) |
| `--result <FILE>` | `WorkspaceResult` JSON path the agent must write (required). | — (set per job by the worker) |
| `--workspace <DIR>` | Prepared coordination-scoped workspace root. | process cwd |
| `--provider <anthropic\|chatgpt\|deepseek>` | Provider adapter to use. | `chatgpt` |
| `--model <ID>` | Main model id. | provider built-in default |
| `--investigate-model <ID>` | Cheaper read-only subagent model id. | provider built-in default |
| `--provider-url <URL>` | Provider base-URL override (e.g. a local fake LLM). | provider built-in URL |
| `--max-iterations <N>` | Maximum model/tool iterations. | compiled default |
| `--subagents <on\|off>` | Enable investigate/read-only subagents. | `off` |
| `--capture-dir <DIR>` | Optional prompt-overlay / debug-capture dir. | `$XDG_CONFIG_HOME/anvil`, else `$HOME/.config/anvil` |

For an OAuth provider credential the agent materializes the tokens into a
temporary `auth.json` its OAuth loader reads (and refreshes) for the run; no
worker-written auth file or model/url env crosses the boundary.

### Git author and push identity (worker-prepared, no env)

For writable jobs, the worker configures the git author identity
(`user.name`/`user.email`) and push credential (`http.extraheader`) in each
writable repo's **local `.git/config`** before spawning the agent. The agent uses
normal git commands in the prepared checkout; no per-role token env reaches the
agent's argv or environment.

### Still-internal child env

The agent inherits the parent process environment and then has the one secret
provider-credential var overlaid.

| Variable | Purpose |
| --- | --- |
| `GIT_TERMINAL_PROMPT` | Disable interactive git prompts for child git processes. |
| `PATH` | Resolves bare `git`, `temper-agent`, and external agent commands; document as a deployment prerequisite. |

If strict hermeticity is required, use `env_clear`/allowlists at the
child-process boundary and re-add only `PATH`, `HOME`, and
`TEMPER_AGENT_PROVIDER_CREDENTIALS_JSON`.

## Env names that live in config/bindings (not fixed env knobs)

These are not Temper-defined environment variables; they are **config/binding
fields that name an env var holding a secret**. The value is a secret carried in
the environment, but the name is chosen in the bindings/config file, so the
supported surface is the binding field, not a fixed `TEMPER_*` variable.

| Binding field | Purpose |
| --- | --- |
| `service.token_env` | Names an env var containing the interaction HTTP bearer token (optional on loopback; required for non-loopback auth). |
| `profiles.*.human_token_env` | Names an env var containing the human-side Forge token. |
| `profiles.*.agent_token_env` | Names an env var containing the agent-side Forge token. |
| `responders.*.env_allowlist[]` | Env names copied into external responder subprocesses; missing names are silently dropped and the child env is otherwise cleared. |
| `ProcessCall.env` / `ProcessCall.clear_env` | Generic child-process env-export capability in `temper-engine-io`; caller-provided arbitrary keys, `clear_env=true` starts from an empty env. |

The `temper-interaction repl`/`serve` entry points read these named env values at
their own process boundary. The former fixed fallbacks
`TEMPER_INTERACTION_SPEC`, `TEMPER_INTERACTION_BINDINGS`, and
`TEMPER_INTERACTION_PROFILE` have been **removed** — pass `--spec` / `--bindings`
/ `--profile` (or set them in the bindings/config) instead.

## Test-, demo-, and tooling-support variables

Everything in this section is **test/demo/tooling support**: it is allowed to
exist because it gates live/network tests, drives example launchers, or feeds the
provisioning/validator helper tools. None of it is part of the production
deployment surface and none of it should appear in operator-facing how-to docs as
a core knob.

### Provisioning and validator tool env (operator/test tooling)

These are consumed by `temper provision-forgejo`, the reference-delivery
validator, and the testing worker — operator-adjacent tooling, not the production
daemon.

| Variable or pattern | Purpose |
| --- | --- |
| `TEMPER_FORGEJO_ADMIN_TOKEN` | Admin token for `temper provision-forgejo` / the testing provisioner; required, non-empty, never on argv. |
| `TEMPER_FORGEJO_TOKEN` | Forgejo token required by `validate-reference-delivery` and by `temper-testing-worker --backend forgejo`. |
| `TEMPER_FORGEJO_USERNAME` / `TEMPER_FORGEJO_PASSWORD` | Testing-worker Forgejo web-UI credential pair (optional). |
| `TEMPER_WORKFLOW_FILE` | Workflow-file fallback for the provisioning/testing workers (`--workflow` wins; empty ignored). |
| `TEMPER_WAKE_DEBOUNCE_MS` | Testing-worker local wake-debounce override. |
| `TEMPER_FORGEJO_CI_DIAGNOSTICS` | Testing-worker Forgejo CI web-UI diagnostics (any non-blank value enables). |

`provision-forgejo --out` writes a mode-0600 `credentials.toml` file in the same
schema the daemon and validator read. It contains generated `[forge.users.*]`
entries for workflow roles and `bot` (user/email/password/token). Durable values
belong in the credentials file / secret manager; launchers should pass the file
with `--secrets` or read short-lived child-process env from it rather than
checking generated tokens into source control.

### Demo launcher variables

`examples/basic-delivery/run.sh` is intentionally fixed and no longer sources a
launcher config/env file. It sets only child-process implementation variables
internally (`GOMAXPROCS`, `GITEA_WORK_DIR`, `TEMPER_INIT_ADMIN_PASSWORD`,
`TEMPER_INIT_PROVIDER_KEY`, and short-lived helper variables for its Python
snippets).

`examples/reference-delivery/run.sh` is also fixed and no longer sources
`config/temper.env` or local secret files. Its normal `start` path sets the same
child-process implementation variables as the basic demo, plus the local jig
process wiring used by the reviewer-gated reference workflow. Its `multi-repo`
path additionally sets per-process `TEMPER_FORGEJO_TOKEN`,
`TEMPER_FORGEJO_USERNAME`, `TEMPER_FORGEJO_PASSWORD`, and
`TEMPER_FORGEJO_CI_DIAGNOSTICS` while launching deterministic
`temper-testing-worker --backend forgejo` role/mechanical workers. Those values
come from the run-local `credentials.toml` written by demo provisioning and are
not operator configuration knobs.

Forgejo Actions workflows use GitHub-compatible env names supplied by Forgejo
Actions (`GITHUB_API_URL`, `GITHUB_REPOSITORY`, `GITHUB_SHA`, `GITHUB_TOKEN`).

### Test-only variables

| Variable or pattern | Purpose |
| --- | --- |
| `TEMPER_TEST_CONVERGENCE_TIMEOUT_SECS` | Overrides ignored Forgejo e2e convergence timeouts. |
| `TEMPER_SCENARIO_TEMPER_BIN` / `TEMPER_BIN` | Scenario CLI live-run fallback path for the standalone `temper` binary when `--temper-bin` is not supplied (`TEMPER_BIN` is compatibility-only). |
| `TEMPER_SCENARIO_RUN_ID` | Scenario harness correlation id injected into live `temper` processes and captured with structured JSON event facts. |
| `TEMPER_TESTING_DAEMON_WORKER_BIN` | Overrides the root e2e path to `temper-testing-daemon-worker`. |
| `TEMPER_E2E_GIT_USER` / `TEMPER_E2E_GIT_TOKEN` | Git identity / optional push token for the daemon test worker. |
| `TEMPER_ANTHROPIC_OAUTH`, `TEMPER_CHATGPT_OAUTH`, `TEMPER_DEEPSEEK_REQUEST_ORACLE` | Opt-in gates for ignored live provider/request-oracle tests. |
| `ANVIL_TEST_PROVIDER_BASE_URL`, `TEMPER_AGENTS_*_TOKEN_URL` | Provider base-URL / OAuth-endpoint overrides used by provider tests and the responder `ProviderEnv` path; test/fixture knobs, not a production deployment surface. |
| `TEMPER_SIM_SEED_BASE` | Base seed for the simulation fresh-seed batch; invalid/unset uses default. |
| `TEMPER_INTERACTION_PROCESS_RESPONDER_TEST_ALLOWED` / `_BLOCKED` | Interaction process-responder env-filtering tests. |
| `TEMPER_RUNNER_ROLE_DECISION_ALLOWED` / `_BLOCKED` | Legacy role-selector process env-filtering tests; not a production role-agent interface. |
| `SMITH_FAKE_AGENT_VERDICT`, `SMITH_FAKE_AGENT_FILE`, `SMITH_FAKE_AGENT_CONTENT`, `SMITH_FAKE_AGENT_BODY` | `temper-worker` fake-agent test controls. |
| `FORGEJO_DEFAULT_REPO` | Scrubbed by e2es for hermeticity; no production reader. |
| `CARGO_*` | Cargo-provided compile-time/test mechanics; excluded from the runtime inventory. |
| `TMPDIR` / `TEMP` / `TMP` | Platform temp vars affecting `std::env::temp_dir()` in tests only. |

### Script variables

`scripts/check-rust-file-size.sh` supports `RUST_FILE_SIZE_HARD_MAX_LOC`
(default `800`), `RUST_FILE_SIZE_JUSTIFICATION_LOC` (default `600`),
`RUST_FILE_SIZE_ALLOWLIST`, and `RUST_FILE_SIZE_JUSTIFICATIONS`.
`scripts/check-no-ambient-env.sh` has an internal `AMBIENT_ENV_ALLOWLIST` shell
variable, but exporting that name before running the script has no effect.

## Removed legacy variables

The deployment-shape and agent env-override variables that earlier Temper
releases read have been **removed**; they are no longer read and have no effect.
Configure these through the config/credentials files, the `temper agent` flags,
or the single provider-credentials JSON instead.

| Removed variable(s) | Replaced by |
| --- | --- |
| `TEMPER_CONFIG` | `--config` flag, plus the `~/.config/temper` default location. No environment variable selects the config files. |
| `TEMPER_FORGE_URL`, `FORGEJO_URL` | `[forge] url` in `temper.toml`. |
| `TEMPER_FORGE_TOKEN`, `FORGEJO_ACCESS_TOKEN` | A `token` under `[forge.users.<admin>]` in `credentials.toml` (with `[forge] admin` naming the admin). |
| `FORGEJO_USERNAME`, `FORGEJO_PASSWORD` | Web-UI / per-role credentials in `credentials.toml`. |
| `TEMPER_WORKFLOW` | `[engine]` workflow settings in `temper.toml`. |
| `TEMPER_ENGINE_BIND`, `TEMPER_ENGINE_PORT` | `[engine]` bind/port settings in `temper.toml`. |
| `TEMPER_DAEMON_URL`, `TEMPER_WORKSPACE` | Worker daemon URL / workspace settings in `temper.toml`. |
| `TEMPER_AGENTS_*` (model/url/auth-file overrides) | The resolved provider config rendered onto the `temper agent` command line; the OAuth credential rides in `TEMPER_AGENT_PROVIDER_CREDENTIALS_JSON`. |
| `TEMPER_INTERACTION_SPEC`, `TEMPER_INTERACTION_BINDINGS`, `TEMPER_INTERACTION_PROFILE` | `--spec` / `--bindings` / `--profile` flags (or the bindings/config file). |

Note: `temper-engine` still reads `TEMPER_FORGEJO_TOKEN_<ROLE>` for its
Forge-API token use, and the responder binaries still read provider env via the
kept `ProviderEnv` API. These are deliberate process-internal/responder paths and
are tracked separately from the deployment-config surface above; they are not
operator-facing deployment knobs.
