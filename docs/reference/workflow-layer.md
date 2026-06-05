# Workflow layer reference

This page is the entry point for the `temper-workflow` contract. The workflow
layer owns policy and orchestration over `temper-forge`; it must not contain
concrete Forge backend code, LLM-provider code, or direct repository mutation
outside the `Forge` trait.

The detailed contract is split by concern:

- [Workflow specification and compilation](workflow-specification.md) — raw,
  validated, compiled, and runtime phases; spec primitives; role prompt
  manifests; static validation; label manifests.
- [Workflow classification and planning](workflow-classification-planning.md) —
  metadata blocks, artifact classification, queue matching/activation, runtime
  signals, gates, and transition planning.
- [Workflow runtime execution](workflow-runtime.md) — fresh-read execution,
  effect ordering, idempotency, creates, reviews, merges, and agent-facing role
  tools.
- [Workflow recovery](workflow-recovery.md) — leases, command journaling,
  reconciliation, and applying recovery actions.

Related reference pages:

- [Forge interface](forge-interface.md) defines the portable backend API used by
  the runtime.
- [Workflow runtime robustness guarantees](robustness-guarantees.md) records the
  safety properties and tests that prove retry/recovery behavior.
- [Cross-repository workflow contracts](cross-repo-workflows.md) defines
  repo-qualified artifact references and global child correlation keys.

## Contract at a glance

A workflow is a state machine over Forge artifacts. Current state is derived from
labels, metadata blocks, native dependency links, native CI jobs, native reviews,
pull-request merge state, and dependency relations. The Forge remains the
source of truth; the workflow layer reloads current Forge state before any
mutation.

The crate should provide:

- workflow specification types;
- validation from raw specs to `ValidatedWorkflow`;
- compilation to role prompts, tool manifests, queue manifests, transition
  manifests, and label manifests;
- artifact classification and pure queue/transition planning;
- runtime transition enforcement through the `Forge` trait;
- idempotency, leases, journaling, reconciliation, and test helpers.

Runtime and compiler APIs must not accept an unvalidated specification. The
normal phases are:

1. `RawWorkflowSpec` loaded from serde input;
2. `ValidatedWorkflow` with normalized ids and checked references;
3. `CompiledWorkflow` with manifests for runners and label provisioning;
4. runtime execution using a validated/compiled workflow plus backend handles.

## Implemented surface

The current workflow crate validates and compiles the reference-delivery
workflow, classifies issues and pull requests, evaluates queues, plans
transitions, executes labels/assignees/comments/PR creates/reviewer requests/
reviews/merges, and provides lease, journal, reconciliation, and recovery
application primitives. Queue automation metadata drives mechanical servicing
through the same executor path used by role workers.

Still intentionally limited:

- `invariant` and spec-level `recovery_policy` are not modeled as first-class
  spec primitives.
- Lease effects in transition specs are placeholders; lease mutation currently
  goes through `LeaseManager`.
- Generated Rust workflow code and generated tool bodies are not implemented.
- Escalation/diagnosis recovery actions are advisory; workflow-specific adapters
  may project them to labels or comments.

The `reference-delivery.json` fixture transcribes
[the reference delivery design](../explanation/reference-workflow.md) into these
primitives and is the main executable conformance example. Remaining product
workflow gaps are tracked in
[reference-workflow-gaps.md](../explanation/reference-workflow-gaps.md).
