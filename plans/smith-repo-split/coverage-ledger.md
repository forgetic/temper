# Smith split coverage ledger

Phase 6 is complete: Temper no longer contains `crates/temper-agents`,
`pi_agent_rust`, provider/auth code, in-process product-manager LLM responders,
or `temper-testing` real-agent fixtures. Temper owns process protocols,
validation, runner authority, transcripts, proposal acceptance, fake deterministic
tests, and production process wiring. Smith owns concrete pi-SDK-backed provider,
product-manager, and workflow-role decision behavior.

## Ownership after Phase 6

| Area | Temper coverage now | Smith coverage now |
| --- | --- | --- |
| Provider/auth/model calls | None; Temper treats responder args/env as opaque and clears child env except allow-listed names. | `cargo test --workspace --all-targets provider oauth anthropic_oauth`; ignored ChatGPT/Anthropic live tests. |
| One-turn structured decisions | Process reply validation in `temper-runner`. | `cargo test --workspace --all-targets workflow_role_decision` plus provider live smokes. |
| Product-manager profile behavior | Generic `ConversationRequest`/`ConversationReply`, transcripts, inert proposals, and filing. | `cargo test --workspace --all-targets product_manager`; `smith-product-manager-responder`. |
| Workflow-role behavior | Manifest authority, authorized action validation, `RoleTools`, external-tool binding, process adapter. | `smith-workflow-role-decision` prompt/mapping/provider implementation. |
| Forgejo + real LLM proof | Temper process adapter and Forgejo support are used from Smith's ignored e2e. Temper's own Forgejo multiprocess suite uses fake agents only. | `TEMPER_FORGEJO_E2E=1 TEMPER_FORGEJO_AGENTS=1 cargo test -p smith-temper-agent-cli --test forgejo_workflow_role_e2e -- --ignored --test-threads=1`. |
| Operator launchers | `examples/reference-delivery` and `examples/dogfood` select Smith process responders by config. | Smith binaries and provider preflight. |

## Removed Temper commands

These commands are intentionally gone after Phase 6 and must not be used as
coverage gates in Temper:

- `cargo test -p temper-agents ...`
- `TEMPER_FORGEJO_E2E=1 TEMPER_FORGEJO_AGENTS=1 cargo test -p temper-agents --test forgejo_engineer_e2e ...`
- `TEMPER_FORGEJO_E2E=1 TEMPER_FORGEJO_AGENTS=1 cargo test -p temper-testing --test forgejo_multiprocess ... happy_path_converges_with_real_agents`
- `temper-testing-worker --agents real`
- production `temper-worker --auth/--codex-model/--auth-file`
- product-chat `--auth/--codex-model/--auth-file`

## Active Temper coverage

| Command | Protects |
| --- | --- |
| `cargo test -p temper-interaction process_boundary_request_and_reply_json_round_trip` | Stable interactive responder wire fixtures. |
| `cargo test -p temper-interaction process_responder` | Process I/O, timeout, nonzero exit, malformed stdout, duplicate proposal ids, and env allow-listing. |
| `cargo test -p temper-runner role_decision` | Stable workflow-role decision wire fixtures and provider-neutral request/reply model. |
| `cargo test -p temper-runner role_decision_process` | Valid action execution, env allow-listing, timeout, nonzero exit, malformed/duplicate/unknown reply fields, version mismatch, unauthorized action downgrade, and `coding_workspace` handoff through `RoleTools`. |
| `cargo test -p temper-production product_chat` | Transcript/session/local API/filing behavior through a configured process responder. |
| `cargo test -p temper-production worker_args worker_role_agent` | Required role-decision process config, env/CLI selection, redaction, and process-bound worker construction. |
| `cargo test -p temper-production coding_workspace_tests::local_git_workspace_accepts_product_code_or_docs_diff` and `cargo test -p temper-production pr_diff_guard` | Meaningful PR/diff honesty guards for executable workspace providers. |
| `cargo test -p temper-testing --test multiprocess` and `cargo test -p temper-testing --test multi_repo_multiprocess -- --ignored` | Hermetic process-split rehearsal with fake agents. |
| `TEMPER_FORGEJO_E2E=1 cargo test -p temper-testing --test forgejo_multiprocess -- --ignored --test-threads=1` | Real Forgejo + real CI convergence with fake agents. |
| `TEMPER_FORGEJO_E2E=1 cargo test -p temper-testing --test forgejo_workspace_pr -- --ignored --test-threads=1` | Forgejo e2e for meaningful PR head / coding workspace. |
| `cd examples/reference-delivery && ./run.sh start` | Operator-facing production worker path through Smith's role decision process responder. |
| `cd examples/dogfood && ./run.sh product-chat` | Live product-chat session through Smith's product-manager process responder. |
| `cd examples/dogfood && ./run.sh preflight` | Live dogfood safety/coding-workspace preflight; Smith cannot bypass it. |

## Active Smith coverage

Run from `~/src/rust/smith`:

| Command | Protects |
| --- | --- |
| `cargo test --workspace --all-targets` | Hermetic Smith provider, product-manager, workflow-role decision, and CLI coverage. |
| `cargo test --workspace --all-targets product_manager` | Product-manager request mapping, response parsing, draft/proposal validation, prompt export, and Temper fixture compatibility. |
| `cargo test --workspace --all-targets workflow_role_decision` | Temper workflow-role fixture compatibility, manifest prompt/context mapping, bound external-tool metadata, authorized/no-action mapping, unauthorized model action downgrade, and protocol-version rejection. |
| `TEMPER_CHATGPT_OAUTH=1 cargo test --test chatgpt_oauth_live -- --ignored --nocapture` | Live ChatGPT/OpenAI Codex OAuth smoke and refresh/write-back. |
| `TEMPER_ANTHROPIC_OAUTH=1 cargo test --test anthropic_oauth_live -- --ignored --nocapture` | Live Anthropic OAuth smoke with Claude Code identity handling. |
| `TEMPER_FORGEJO_E2E=1 TEMPER_FORGEJO_AGENTS=1 cargo test -p smith-temper-agent-cli --test forgejo_workflow_role_e2e -- --ignored --test-threads=1` | Real Forgejo + real LLM proof through Temper's process adapter, coding workspace, and `RoleTools`. |

## Phase 6 parity checklist

- Temper workspace and lockfile contain no `temper-agents`, `pi_agent_rust`, or
  pi-SDK transitive pins.
- Production role workers require `--role-decision-command` /
  `TEMPER_WORKER_ROLE_DECISION_COMMAND`.
- Product-manager chat requires `--responder-command` /
  `TEMPER_PRODUCT_CHAT_RESPONDER_COMMAND`.
- Temper fake/multiprocess tests remain deterministic and provider-free.
- Smith remains the only repository with provider/auth/model tests and ignored
  real-provider gates.
