# ADR 0007: Define the workflow layer and agent compilation boundary

## Status

Accepted

## Context

Harness needs to support evolving agentic workflows instead of one hard-coded process.
A starting workflow uses architect, engineer, reviewer, tester, and owner roles, but future workflows may add more roles, parallel workers, richer gates, or different escalation paths.

The existing Forge layer is intentionally backend-agnostic and should remain focused on collaboration primitives. The crate scaffolded as `harness-core` had a name too vague for the next layer, whose purpose is workflow orchestration.

Workflows must also be robust when agents crash, repeat tool calls, or resume after partial side effects.

## Decision

Rename `harness-core` to `harness-workflow` when implementation begins. This rename has been carried out; the crate is now `harness-workflow`, still a minimal placeholder with no workflow functionality yet.

Define the workflow layer as a deterministic state-machine and compilation layer on top of `harness-forge`. It owns:

- declarative workflow specifications
- static workflow validation
- queue, transition, gate, lease, and invariant primitives
- compilation to role prompts and narrow role-specific tool manifests
- runtime transition enforcement against Forge artifacts
- idempotency, command journaling, reconciliation, and crash recovery
- simulation and crash-injection support for validation

A workflow specification should combine structured machine-readable rules with prose role charters. Structured rules define artifacts, labels, state dimensions, queues, transitions, gates, invariants, and recovery policy. Prose charters guide judgment-heavy agent behavior such as design taste, review standards, and owner values.

Labels are the public Forge projection of workflow state, not the complete internal model. The workflow layer should parse Forge artifacts into typed workflow artifacts, detect impossible label combinations, and repair or escalate drift through a reconciler.

Agents should not receive broad Forge tools by default. The workflow compiler should generate the smallest useful tool surface for each role, with each tool enforcing workflow preconditions and applying only authorized transitions.

## Rust validation strategy

Use Rust types to make invalid internal usage difficult:

- keep raw loaded specs separate from `ValidatedWorkflow`
- require `ValidatedWorkflow` for compilation and runtime APIs
- use explicit enums or typed IDs for state dimensions, transitions, roles, queues, gates, and effects
- use exhaustive `match` handling for workflow effects
- use capability or typestate patterns where workflows are generated into Rust code
- support build-time validation for checked-in workflow specs later

Rust types do not remove the need for runtime guards because Forge state can be edited externally and distributed workers can race. Every transition must re-check fresh Forge state.

## Robustness strategy

The workflow runtime should assume agents can crash at any point. Durable Forge-visible state plus workflow metadata must be enough to recover.

Required runtime principles:

- transitions are idempotent and safe to retry
- create operations use correlation keys to avoid duplicate issues, PRs, or comments
- claims are leases with expiration, not permanent ownership
- transition execution is journaled as planned, applying, completed, or failed
- a reconciler periodically scans Forge artifacts for expired leases, partial transitions, impossible states, and orphaned relationships
- ambiguous drift is routed to an owner or operator queue instead of being silently hidden

## Consequences

`harness-workflow` may depend on `harness-forge`; `harness-forge` must not depend on workflow or agent crates.

The first implementation should be phased: rename the crate, add typed spec primitives, add validation, add compilation outputs, then add runtime execution and recovery. Agent execution remains a compiled consumer of workflow manifests, not part of the initial workflow crate.
