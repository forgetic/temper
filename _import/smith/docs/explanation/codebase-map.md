# Codebase map

| Path | Look here for |
| --- | --- |
| `crates/smith-agent-protocol/` | The serde-only worker ↔ agent process protocol (workspace context in, step-progress stream + result out). Smith owns this contract; agents implement it (see [the agent / orchestration split](agent-process-split.md)). |
| `crates/smith-io-engine/` | Smith's copy of the sans-IO completion-engine driver (`Machine` core + `Executor` shell + `drive` loop). |
| `crates/smith-worker/` | Worker-process library and `smith-worker` binary for the Temper worker/daemon wire protocol loop, the persistent per-`(repo, role)` git workspace plane (clone/fetch, branch preparation, role-authored commits, role-token branch pushes), and the out-of-process agent runner that spawns the configured `--agent-command` and relays its step-progress stream. |
| `examples/` | Smith-owned operator demos (`basic-delivery` and `reference-delivery`), both booting the two-tier Temper daemon / Smith worker topology with the sibling anvil checkout's `anvil-agent` as the default coder. |
| `deploy/` | The Smith worker tier of the two-tier topology: the `smith-worker.service` unit template, the `smith-worker-launcher` ExecStart shim, `~/.config/smith` config templates, and the idempotent install script (which also builds/installs `anvil-agent` from the sibling anvil checkout). The Temper daemon tier deploys from the sibling `temper/deploy/`. |
| `docs/` | Diátaxis docs, ADRs, and Smith-owned agent lessons. |

The coding agent itself (LLM loop, providers/auth, responders) lives in the
sibling **`anvil`** repository and runs out-of-process behind
`smith-agent-protocol`; smith links no agent or LLM code.

## Temper dependency posture

Smith keeps only serialization-only Temper protocol dependencies in production:
`temper-worker-protocol` for the `smith-worker`↔daemon wire protocol. (The ADR
0002 stdio JSON process boundary and its `temper-process-protocol` dependency
moved to anvil with the responders.)

Smith does not call the Forge API; `smith-worker`'s git-plane workspaces are
the intentional exception for role-credentialed clone/fetch/commit/push
operations.

Deployment matches this split: the Temper daemon (from `temper/deploy/`) is the
sole Forge API writer and holds the per-role API tokens, while `deploy/` here
ships the protocol-speaking `smith-worker` tier, which holds per-role git
credentials and talks to the daemon over `temper-worker-protocol`.
