# End-to-end big picture

This page is the deployment-topology view: once the workflow core is complete
and a `harness-forge-forgejo` backend exists, *where does each piece run and how
do they fit together?* It does not restate the conceptual layering (see the
"Layer model" in [Agentic workflows](agentic-workflows.md)), the triggering
decision ([ADR 0009](../adr/0009-triggering-model-webhook-accelerated-poll-backstopped.md)),
the workflow semantics ([Reference delivery workflow](reference-workflow.md)),
or the proven safety properties ([robustness guarantees](../reference/robustness-guarantees.md)).
It is the picture that ties them together for an operator.

## The four planes

Harness is arranged so that **the Forge is the only authoritative state.**
Nothing above it holds durable truth, which is what makes the system crash-safe
and freely restartable. Four planes:

```text
┌─ Forge plane ────────────────────────────────────────────────┐
│  Forgejo @ git.ekanayaka.io                                   │
│  issues · PRs · labels · comments · merges · Actions (CI)     │
│  + workflow metadata blocks in bodies (kind / parents / deps  │
│    / correlation_key / lease)        ← single source of truth │
└───────────────────────────────────────────────────────────────┘
        ▲ query / mutate (harness-forge-forgejo)   ▲ git push, CI
        │                                          │
┌─ Control plane ──────────────────────┐   ┌─ Worker plane ─────┐
│  harness runner / controller         │   │ disposable agents  │
│  • holds Compiled/ValidatedWorkflow  │──▶│ (LLM or human)     │
│  • trigger loop (poll + webhook)     │   │ • role prompt      │
│  • classify → plan → dispatch        │   │ • role tools only  │
│  • Executor + recover::Applier       │   │ • git workspace    │
│  • Reconciler (leases / repairs)     │   │ • lease heartbeat  │
└──────────────────────────────────────┘   └────────────────────┘
        ▲ ChangeHints
┌─ Signal plane ───────────────────────────────────────────────┐
│  Forgejo webhooks (edge, lossy)  +  poll timer (level, truth) │
│  +  CI conclusions read from list_ci_jobs                     │
└───────────────────────────────────────────────────────────────┘
```

The three conceptual layers map onto this directly: `harness-forge` is the Forge
plane interface, `harness-workflow` is the brain inside the control plane, and
"agent runners" are the worker plane. The signal plane is the triggering model
made operational.

### Consequence of the authoritative-state invariant

Because truth lives only in the Forge, the runner is effectively stateless: it
can be killed and restarted, or run as several instances, and the
level-triggered poll reconstructs everything. Leases are durable (they live in
metadata blocks), and the reconciler re-derives partial transitions from fresh
Forge state, so a durable command journal is a fast-recovery optimization, not a
correctness requirement.

## Where each piece runs

- **Forgejo**: the existing server. Labels are provisioned once from the
  compiled `LabelManifest`; the workflow's bot users are registered; webhooks
  point at the runner.
- **The runner**: one long-lived service is enough to start. It holds the loaded
  `ValidatedWorkflow`/`CompiledWorkflow`, a `harness-forge-forgejo` client, the
  trigger loop, and instances of `Executor`, `Reconciler`, and
  `recover::Applier`. Stateless, so restart/scale-out is safe.
- **Agent workers**: ephemeral, one per claimed work item (or a small per-role
  pool sized by the role's `concurrency` hint — `engineer: 3`, `reviewer: 2` in
  the reference fixture). Each needs model/API access, a git workspace if the
  role touches code, the harness tool layer pointed at Forgejo, and a worker
  identity for its lease.

## Mechanical vs. judgment dispatch

The runner's central decision is whether a needed change is *mechanical* (the
runtime does it for free) or *judgment* (it costs an actor):

- **Mechanical** changes need no actor and run entirely in the runner via
  `Reconciler` + `recover::Applier`: dependency unblocks, partial-transition
  repairs, expired-lease requeues, post-merge label survival. No agent is
  spawned.
- **Judgment** changes are dispatched to an agent of the queue's subscribing
  role. The queue → subscriber mapping in each `QueueManifest` is the dispatch
  table; queue activation policy (`min_depth`/`max_age`) decides *when* a cohort
  is worth a worker.

In both cases mutations cross the same boundary: the agent receives a
role-scoped `RoleTools` facade, and its only state-changing operations are
`Executor::execute` for that role's authorized transitions plus the documented
idempotent pull-request creation seam. The executor re-loads fresh state and
re-plans before mutating. Agents do their real work (write code, push a branch,
run a review) freely; they touch *workflow state* only through `RoleTools`. See
the runtime guarantees in [the workflow-layer reference](../reference/workflow-layer.md).

## From core-complete to a running deployment

What exists today is the whole decision core (spec → validate → compile →
classify → plan → execute → reconcile → apply, with leases, journaling, and
proven safety properties), runnable against the reference backends, plus the
initial `harness-runner` worker-plane primitives: read-only scans that turn fresh
Forge state into active-queue work items, `Agent`/`RoleTools`, and tickable
`Worker`/`RoleWorker` units. The remaining path to a live deployment, in
dependency order:

1. **`harness-forge-forgejo`** — implement the existing trait against Forgejo's
   API, including the `Version`/`expected_version` CAS primitive
   ([ADR 0013](../adr/0013-portable-optimistic-concurrency.md)) and
   `list_ci_jobs` over Forgejo Actions, keeping observable-contract parity with
   the reference backends ([ADR 0008](../adr/0008-in-memory-backend-and-backend-naming.md)).
2. **The runner/controller** — partially started in `harness-runner`: `scan`
   now reconstructs active judgment work and `RoleWorker` can tick one role, but
   the trigger loop and mechanical-vs-judgment dispatch still need to be built.
   Nothing composes all workers in a loop today; tests drive one worker or the
   workflow `Executor`/`Reconciler` directly.
3. **Agent-provider adapter** — wire a compiled `PromptManifest` to a model and
   map model tool calls onto the production `RoleTools` facade.
4. **Webhook adapter + `ChangeHint`** — the ADR 0009 follow-up: Forgejo-specific
   receipt/verify/parse normalized into a hint the runner coalesces; stays off
   the `Forge` trait.
5. **Deployment config** — `ExecutionContext` role→user bindings, per-worker
   credentials/identities, and the target repo.

## Open design seams

- **No explicit mechanical/judgment flag.** The runner infers it (reconciler-owned
  actions are mechanical; queue-subscribed transitions are judgment). This works
  for the reference workflow, but making it an explicit spec property would
  harden the seam where free runtime work meets actor cost.
- **Fan-out creation rides `ensure_issue`.** Breaking one design into many code
  issues currently uses the idempotent create helper rather than a modeled
  `create_issue` transition effect, so that creation sits outside the same
  authorization/tool boundary as every other state change.
