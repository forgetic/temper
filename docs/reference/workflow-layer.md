# Workflow layer reference

This page defines the intended contract for the `harness-workflow` crate. Phase 2 has landed the spec and validation foundations; the rest of the contract below (compilation, runtime, recovery) is still planned. See "Implementation status" for what exists today.

## Scope

`harness-workflow` owns workflow policy and orchestration on top of `harness-forge`. It must not contain concrete Forge backend code or agent-provider code.

The crate should provide:

- workflow specification types
- validation from raw specs to `ValidatedWorkflow`
- compilation to role prompts, tool manifests, and label manifests
- queue evaluation and transition planning
- runtime transition enforcement through the `Forge` trait
- idempotency, leases, journaling, reconciliation, and test helpers

## Type phases

Runtime and compiler APIs must not accept an unvalidated specification.

Recommended phases:

- `RawWorkflowSpec`: loaded from YAML, JSON, TOML, or generated input
- `ValidatedWorkflow`: normalized, internally consistent workflow
- `CompiledWorkflow`: role prompts, tool manifests, label manifests, and runtime tables
- `RuntimeWorkflow`: compiled workflow plus backend handles and durable state

Validation errors should be diagnostic collections so users can fix multiple spec issues at once.

## Implementation status

Phase 2 implements the first two phases and their supporting types:

- `spec::RawWorkflowSpec` and its raw child structs (`RawRole`, `RawLabel`, `RawArtifactKind`, `RawStateDimension`, `RawState`, `RawQueue`, `RawTransition`, `RawEffect`, `RawGate`) load from serde input.
- `validated::ValidatedWorkflow` is the normalized model. It has no public constructor; the only way to build one is `RawWorkflowSpec::validate` / `validate::validate`, so compiler and runtime APIs added later can require it by type.
- `ids` provides typed ids: `RoleId`, `LabelId`, `ArtifactKindId`, `StateDimensionId`, `StateId`, `QueueId`, `TransitionId`, `GateId`.
- `diagnostics` provides `Diagnostic`, `Severity`, `SymbolKind`, `ReferenceSite`, and the `ValidationErrors` collection.

Not yet modeled: `relation`, `invariant`, `recovery_policy`, concurrency limits, compilation outputs, and runtime execution. Effects in Phase 2 cover only label add/remove.

Gate/transition wiring is modeled in both directions: a transition lists `requires_gates`, and a gate lists `satisfied_by` transitions.

## Spec primitives

A workflow spec contains these logical primitives.

| Primitive | Meaning |
| --- | --- |
| `role` | Actor authority, queues, concurrency limits, and prose charter |
| `artifact_kind` | Logical item mapped to a Forge issue or pull request |
| `state_dimension` | Named state group, often projected as mutually exclusive labels |
| `queue` | Query that selects artifacts needing attention |
| `transition` | Guarded state change authorized for one or more roles |
| `gate` | Condition that unlocks another transition, such as merge readiness |
| `relation` | Typed link between artifacts, such as parent, dependency, or produced PR |
| `invariant` | Condition that must hold during runtime scans |
| `recovery_policy` | What to do with expired leases, partial transitions, and drift |

Labels are a portable Forge projection of workflow state. The workflow layer may use metadata blocks in bodies or comments for information that is not represented by the current Forge interface, such as correlation keys and typed relations.

## Static validation

Validation must reject or diagnose:

- duplicate role, label, artifact, state-dimension, queue, transition, or gate IDs (implemented; state ids are checked for uniqueness within each dimension)
- references to undeclared roles, labels, artifact kinds, queues, transitions, or gates (implemented)
- transitions whose effects contradict declared mutually exclusive dimensions (planned)
- gates that cannot be satisfied by any declared transition (planned)
- role tool declarations that exceed the role's transition authority (planned)
- artifact mappings that cannot be represented by the Forge interface (planned)

Validation should also warn about unreachable queues, terminal states with no explanation, and labels that are declared but unused (planned).

In the Phase 2 model, labels are the only cross-referenced projection of state: artifact mappings, queues, state declarations, and transition effects all reference label ids. States are not referenced by id outside their dimension, so undeclared-state references are not a current diagnostic.

## Runtime guarantees

Every transition execution must:

1. load fresh Forge state for the target artifact
2. classify it according to the validated workflow
3. check role authority and transition preconditions
4. apply effects through an idempotent executor
5. verify postconditions or emit diagnostics

Agents must not mutate Forge state directly when operating under workflow control. Generated tools are the transition boundary.

## Effects

Workflow effects should be represented as a closed Rust enum so executors and reconcilers must handle every variant. Initial effects should cover label updates, assignee updates, issue creation, PR creation, comments, leases, and PR merge operations.

Create effects need correlation keys. Retrying a create effect with the same correlation key must return the existing artifact when it already exists.

## Claims and leases

A claim is a lease, not permanent ownership. Lease metadata should include role, worker or run ID, claim time, heartbeat time, and expiration time.

Expired leases are handled by recovery policy. Common actions are requeue, extend, escalate, or mark for operator review.

## Reconciliation

The reconciler periodically scans Forge artifacts and command journals. It must detect:

- impossible label combinations
- expired leases
- partial transitions
- duplicated artifacts with the same correlation key
- missing required relations
- merged PRs whose linked code issue remains open
- validation failure labels on merged PRs

Repairs should be deterministic when safe. Ambiguous drift should be routed to an owner or operator queue with a diagnostic comment.

## Compilation outputs

Compilation should produce:

- role prompt manifests
- role-specific tool manifests
- queue manifests
- label manifests for Forge setup
- runtime transition tables
- optional generated Rust code for statically checked workflows

Generated tools should expose intent-level operations such as `claim_code_issue` or `record_test_failure`, not generic Forge mutation operations.
