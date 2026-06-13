# Temper/Smith process boundary

Smith exists so Temper can stay provider-neutral while still using concrete LLM
responders. Since the agent/orchestration split (see
[the agent / orchestration split](agent-process-split.md)), the concrete LLM
behavior itself lives in the sibling **anvil** repo; smith is the orchestration
tier in between.

## Ownership

Temper owns:

- Forge, workflow, runner, and interaction contracts;
- process adapters and protocol validation;
- workflow authority, `RoleTools`, gates, leases, and transition execution;
- interactive transcripts, proposal validation, explicit acceptance, and Forge
  mutations;
- fake/conformance responders for hermetic tests.

Smith owns:

- the worker/daemon loop and the per-`(repo, role)` git workspace plane;
- the worker ↔ agent process protocol (`smith-agent-protocol`).

Anvil owns:

- pi-SDK provider wiring and auth-file handling;
- ChatGPT/Anthropic/DeepSeek model-specific behavior;
- dogfood/example product-manager interactive responder implementation;
- manifest-driven workflow-role decision implementation;
- live provider tests and real-agent e2e tests.

Temper must not take a Rust dependency on Smith. Smith and anvil can depend on
Temper protocol crates to implement the wire contracts; anvil depends on
smith's protocol crate, never the reverse.

## Post-Phase-4a dependency posture

Smith's production Temper dependency is a serialization-only protocol contract:
`temper-worker-protocol`, the versioned worker↔daemon wire protocol —
`smith-worker`'s only Temper dependency. (`temper-process-protocol`, the
spawn-boundary stdio JSON contract of ADR 0002, moved to anvil with the
responder and coding-agent binaries that speak it.)

The legacy direct code dependencies
and imports from `temper-forge*`, `temper-runner`, `temper-testing`, and
`temper-workflow` are gone from Smith's manifests and code. Smith still never
calls the Forge API; the deliberate exception is the git plane in
`smith-worker`'s role-credentialed workspaces, which uses clone/fetch/commit/push
rather than Forge API crates.

## Two-tier deployment topology

The deployed shape mirrors the dependency posture. The **Temper daemon**
(deployed from `temper/deploy/`) is the sole Forgejo **API** writer: it owns
webhook intake, the poll backstop, queue scanning, lease management, mechanical
landing, and every role-attributed Forge mutation, holding the per-role Forge
API tokens. The **Smith worker tier** (deployed from this repo's `deploy/` as
one `smith-worker.service`) registers `(repo, role)` capabilities, long-polls
the daemon over the versioned `temper-worker-protocol` wire contract, runs the
coding agent in persistent per-`(repo, role)` git workspaces, and pushes
branches as the role using only git credentials — it never holds a Forge API
token. The wire protocol crate is the entire compile-time contract between the
tiers.

## Why a process boundary

Provider SDKs, auth-file schemas, subscription quirks, and live model behavior
change faster than Temper's workflow contracts. A process protocol lets Temper
send a provider-neutral JSON request and receive one JSON reply without importing
provider SDKs or credential logic.

The authority boundary remains simple:

```text
Temper request + allowed actions/proposals
        │
        ▼
anvil LLM responder process
        │ no Forge token, no mutation tools
        ▼
Temper validation + state mutation
```

Interactive users and external clients enter through Temper's generic
interaction services, not through a responder process directly. If the
responder crashes, returns malformed JSON, times out, or chooses an
unauthorized action, Temper handles that as a process-adapter failure or no-op
according to the Temper-owned contract.
