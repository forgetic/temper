# Workflow layer reference

This page defines the intended contract for the planned `harness-workflow` crate. The current placeholder crate may still be named `harness-core` until the rename phase is completed.

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

- duplicate role, queue, artifact, transition, gate, or state IDs
- references to undeclared roles, queues, labels, artifacts, or states
- transitions whose effects contradict declared mutually exclusive dimensions
- gates that cannot be satisfied by any declared transition
- role tool declarations that exceed the role's transition authority
- artifact mappings that cannot be represented by the Forge interface

Validation should also warn about unreachable queues, terminal states with no explanation, and labels that are declared but unused.

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
