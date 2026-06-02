# Smith split coverage ledger

Phase 1 freezes where concrete-agent coverage goes before any test is removed
from Temper. `Move timing` names the earliest phase that may delete/move the
Temper test; until then the current command must remain runnable.

## Concrete-agent code inventory

| Current code | Ownership after split | Notes |
| --- | --- | --- |
| `crates/temper-agents/src/provider.rs`, `src/provider/oauth.rs`, `src/provider/anthropic_oauth.rs` | Smith | Provider IDs, auth-file parsing/refresh, request headers, model knobs, secret redaction. |
| `crates/temper-agents/src/decision.rs` | Smith; Temper keeps protocol validation | pi-SDK one-turn execution and tolerant JSON extraction move; Temper validates process replies. |
| `crates/temper-agents/src/product_manager.rs`, `src/prompts/product_manager.md` | Smith | Concrete product-manager behavior/prompt. Temper keeps generic interaction request/reply/proposals. |
| `crates/temper-agents/src/role.rs`, `src/registry.rs` | Both | Smith owns LLM decision engine; Temper owns runner process adapter, manifest authority, `RoleTools`, and registry wiring to process engines. |
| `crates/temper-production/src/product_chat*.rs` | Temper | Transport/session/proposal acceptance stay; process responder configuration remains Temper-owned. |
| `crates/temper-testing/src/legacy_llm/` and real-agent fixture adapters | Smith or Smith test fixtures by Phase 5 | Fixed reference-delivery LLM behavior must not disappear; Temper may keep fake fixtures. |

## Test and rehearsal mapping

| Current path / command | Coverage kind | Post-split home | Move timing | Expected post-split command |
| --- | --- | --- | --- | --- |
| `cargo test -p temper-agents provider` plus module tests in `src/provider/oauth.rs` and `src/provider/anthropic_oauth.rs` | Hermetic provider/auth/OAuth | Smith | Phase 3 | `cd ~/src/rust/smith && cargo test provider oauth anthropic_oauth` |
| `TEMPER_CHATGPT_OAUTH=1 cargo test -p temper-agents --test chatgpt_oauth_live -- --ignored --nocapture` | Live provider | Smith | Phase 3 | `cd ~/src/rust/smith && TEMPER_CHATGPT_OAUTH=1 cargo test --test chatgpt_oauth_live -- --ignored --nocapture` |
| `TEMPER_ANTHROPIC_OAUTH=1 cargo test -p temper-agents --test anthropic_oauth_live -- --ignored --nocapture` | Live provider | Smith | Phase 3 | `cd ~/src/rust/smith && TEMPER_ANTHROPIC_OAUTH=1 cargo test --test anthropic_oauth_live -- --ignored --nocapture` |
| `cargo test -p temper-agents decision` (`src/decision.rs` unit tests) | Hermetic model-reply parsing | Smith; Temper replacement is protocol validation | Phase 3/5 | Smith: `cargo test decision`; Temper: `cargo test -p temper-runner role_decision` |
| `cargo test -p temper-agents product_manager` | Hermetic product-manager profile DTO/prompt/mapping | Smith for responder; Temper for generic fixtures | Phase 4 | Smith: `cargo test product_manager`; Temper: `cargo test -p temper-interaction process_boundary_request_and_reply_json_round_trip` |
| `cargo test -p temper-production product_chat` | Hermetic product-chat session, local API, explicit filing, process-responder args | Temper | Stays | Same Temper command; Smith adds product-manager process-responder tests. |
| `cargo test -p temper-agents role` (`src/role_tests.rs`) | Hermetic generic role-agent behavior | Both | Phase 5/6 | Smith: `cargo test role_decision`; Temper: process-adapter conformance tests plus `cargo test -p temper-runner role_decision` |
| `cargo test -p temper-agents role_external_tool` | Hermetic external-tool metadata and coding-workspace handoff | Both | Phase 5/6 | Smith: role decision tests for external-tool context; Temper: runner process-adapter + existing `coding_workspace` tests |
| `cargo test -p temper-agents registry` | Hermetic manifest-driven registry, no required-tool gaps | Temper | Stays until process registry exists | `cargo test -p temper-runner` focused process registry tests after Phase 2; no Smith equivalent required except binary manifest smoke. |
| `cargo test -p temper-agents --test no_legacy_workflow_prompts` | Hermetic guard against checked-in workflow-role prompts | Temper; Smith may add packaging guard | Stays | Same Temper command; Smith optional `cargo test no_legacy_workflow_prompts` for its fixtures. |
| `TEMPER_FORGEJO_E2E=1 TEMPER_FORGEJO_AGENTS=1 cargo test -p temper-agents --test forgejo_engineer_e2e -- --ignored` | Forgejo e2e + real LLM | Smith and Temper process e2e | Phase 5 | Smith keeps equivalent test; Temper runs Smith process engine through `TEMPER_FORGEJO_E2E=1 TEMPER_FORGEJO_AGENTS=1 cargo test -p temper-testing --test forgejo_multiprocess -- --ignored --test-threads=1 happy_path_converges_with_real_agents` |
| `TEMPER_FORGEJO_E2E=1 cargo test -p temper-testing --test forgejo_multiprocess -- --ignored --test-threads=1` | Forgejo e2e with fake agents and real CI | Temper | Stays | Same command. |
| `TEMPER_FORGEJO_E2E=1 TEMPER_FORGEJO_AGENTS=1 cargo test -p temper-testing --test forgejo_multiprocess -- --ignored --test-threads=1 happy_path_converges_with_real_agents` | Forgejo e2e + real agents | Both | Phase 5 | Temper uses process adapter and Smith binary; Smith owns real-agent fixture/binary tests. |
| `cargo test -p temper-testing --test multiprocess` and `cargo test -p temper-testing --test multi_repo_multiprocess -- --ignored` | Hermetic process-split rehearsal | Temper | Stays | Same commands. |
| `TEMPER_FORGEJO_E2E=1 cargo test -p temper-testing --test forgejo_workspace_pr -- --ignored --test-threads=1` | Forgejo e2e for meaningful PR head / coding workspace | Temper | Stays | Same command; Smith role engines must keep choosing only manifest actions. |
| `cargo test -p temper-production coding_workspace_tests::local_git_workspace_accepts_product_code_or_docs_diff` and `cargo test -p temper-production pr_diff_guard` | Hermetic real-world PR honesty guard | Temper | Stays | Same commands. |
| `cd examples/reference-delivery && ./run.sh start` then `./run.sh validate-multi-repo` / `./run.sh stop` | Real-world reference-delivery rehearsal | Temper launcher + Smith process binaries | Phase 5/6 switch-over | Same launcher with workflow-role decision process command configured to Smith. |
| `cd examples/dogfood && ./run.sh product-chat` | Real-world live product-chat rehearsal | Temper launcher/session + Smith product-manager responder | Phase 4 | Same launcher with `TEMPER_PRODUCT_CHAT_RESPONDER_COMMAND=~/src/rust/smith/.../smith-product-manager-responder`. |
| `cd examples/dogfood && ./run.sh preflight` | Real-world dogfood safety/coding-workspace preflight | Temper | Stays | Same command; Smith has no authority to bypass the preflight. |

## Temper protocol fixtures added in Phase 1

| Protocol family | Temper type owner | Fixture command |
| --- | --- | --- |
| Interactive responder request/reply | `temper-interaction::{ConversationRequest, ConversationReply}` | `cargo test -p temper-interaction process_boundary_request_and_reply_json_round_trip` reads `crates/temper-interaction/fixtures/interactive-responder-*.json`. |
| Workflow role decision request/reply | `temper-runner::{WorkflowRoleDecisionRequest, WorkflowRoleDecisionReply}` | `cargo test -p temper-runner role_decision` reads `crates/temper-runner/fixtures/workflow-role-decision-*.json`. |

## Temper process-adapter coverage added in Phase 2

| Adapter / config | Temper replacement coverage | Replaces or protects |
| --- | --- | --- |
| Interactive responder process adapter | `cargo test -p temper-interaction process_responder` uses hermetic `/bin/sh` responders for valid, timeout, nonzero exit, malformed stdout, duplicate proposal ids, and env allow-list behavior. | Protects the generic product-manager process boundary before Smith owns concrete product-manager behavior. |
| Workflow role decision process adapter | `cargo test -p temper-runner role_decision_process` uses hermetic fake decision processes for valid action execution, env allow-listing, timeout, nonzero exit, malformed/duplicate/unknown reply fields, version mismatch, unauthorized action no-op, and coding-workspace PR action handoff through `RoleTools`. | Replaces Temper-side generic role-agent action validation/execution coverage from `cargo test -p temper-agents role`; Smith still owns provider/model decision quality later. |
| Production worker process selection | `cargo test -p temper-production worker_args` covers `--role-decision-*` flags, `TEMPER_WORKER_ROLE_DECISION_*` env fallbacks, redacted debug output, and default in-process behavior when unset. | Preserves worker defaults while letting operators point role workers at a future Smith binary. |

## Coverage still waiting for Smith

- Provider/auth/OAuth unit and live-provider tests remain in Temper until Phase 3.
- Concrete product-manager responder behavior and prompt tests remain in Temper until Phase 4.
- Real workflow-role model decision behavior, real-agent Forgejo e2e, and Smith
  workflow-role binaries remain pending until Phase 5.
- Phase 2 did not delete or move any `temper-agents` tests.
