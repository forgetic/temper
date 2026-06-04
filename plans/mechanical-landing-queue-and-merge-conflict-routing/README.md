# Mechanical landing queue and merge-conflict routing — implementation plan

This plan refines the reference-delivery merge path so landing is mostly
mechanical: an implementation PR that has native reviewer approval and current-
head green CI is merged by the mechanical worker, while merge conflicts are
routed back to the engineer for a rebase/conflict-resolution push. That push
must get fresh green CI for its new head, but it does not require another review
pass.

This plan is intended to run **after**
`plans/bounded-reconciliation-and-correlation-lookups/`. Treat that plan's
bounded normal mechanical tick and targeted query behavior as a hard constraint:
wake-driven landing must not reintroduce broad all-history issue/PR scans or use
deep audit as a hot path.

Hand the prompt files to agents **one phase at a time, in order**. Each phase
should land green, update this README's status, and add regressions for the new
behavior.

## Goals

- Replace the reference workflow's owner-driven merge queue with a mechanical
  landing queue for approved, green implementation PRs.
- Execute landing deterministically through workflow transitions, not through an
  LLM role or ad-hoc Forge mutations.
- Keep native review and current-head CI as the merge gates; every merge attempt
  must re-read fresh Forge state before mutating.
- Route merge conflicts to an engineer queue by projecting explicit workflow
  state such as `merge-conflict` / `needs-rebase`.
- Let an engineer resolve a merge conflict and requeue the PR for landing without
  requesting a new review, while still requiring green CI for the new head SHA.
- Keep per-repo merge attempts serial inside the mechanical worker. Do **not**
  add FIFO/head-of-line blocking across unrelated PRs.
- Preserve webhook/change hints as latency optimizers only; polling remains the
  correctness backstop.

## Non-goals and constraints

- No FIFO landing queue or durable queue-position primitive in this plan.
- No conflict-attempt counter or automatic escalation after N conflicts.
- No provider-specific branch-protection policy in workflow core. If a provider
  cannot distinguish content conflicts from other merge rejections, document the
  conservative behavior and keep the door open for richer reasons later.
- No head-SHA-scoped review model. The intended reference behavior is that a
  conflict-resolution push keeps the existing approval; CI remains head-scoped.
- Do not weaken existing CI-failure behavior accidentally. If the reference
  workflow still wants non-conflict CI fixes to go back through review, the
  landing label must be cleared on that route and re-added only by review
  approval.
- Do not use the bounded reconciliation deep-audit path for normal landing
  discovery.

## Design sketch

### Workflow expression

Add a small declarative automation contract so a queue can be serviced by the
mechanical worker without an agent decision. A concrete shape can evolve during
Phase 1, but the intended manifest is:

```json
{
  "id": "landing",
  "artifact": "implementation_pr",
  "labels": ["landing"],
  "condition": { "kind": "ci_passed" },
  "automation": {
    "actor": "mechanical",
    "transition": "land_pr",
    "on_merge_conflict": "route_merge_conflict"
  }
}
```

`actor` should reference a declared workflow role so the existing planner can
reuse role authorization checks. This role is an automation authority, not an LLM
role worker. Validation should ensure the transition exists, matches the queue's
artifact kind, and is authorized for the actor. The optional conflict fallback
must be authorized for the same actor and operate on the same artifact kind.

### Reference-delivery state

Introduce routing labels/states along these lines:

- `landing`: this PR is approved for mechanical landing once current-head CI is
  green.
- `merge-conflict` (or `needs-rebase`): the last mechanical merge attempt found
  the PR non-mergeable and the engineer must push a resolved head.

The reviewer approval transition adds `landing`. The landing queue requires
`landing` plus `ci_passed`; the `land_pr` transition still requires both
`review_gate` and `ci_gate`, so stale queue reads cannot merge early.

On successful landing, `land_pr` merges the PR, removes `landing`, and adds the
existing post-merge `landed` and `alignment` labels.

On merge conflict, the mechanical worker runs the fallback transition, which
removes `landing`, adds `merge-conflict`, and optionally comments with brief
engineer guidance. Removing `landing` prevents an immediate retry loop.

The engineer conflict-resolution transition removes `merge-conflict` and adds
`landing` without requesting reviewers. Because CI gates are computed from the
current PR head SHA, the PR will not be mechanically retried until the new head
has green CI.

### Mechanical runtime

A normal mechanical tick should:

1. run the bounded reconciliation path from the preceding plan;
2. scan declared automated queues using the same bounded queue-candidate logic as
   role scans;
3. execute each queue's configured transition through `Executor`, with fresh
   gate-signal rechecks; and
4. if `MergePullRequest` returns a typed merge-conflict error, execute the
   queue's configured conflict fallback transition.

Within one repository, merge attempts should be serial. Across repositories, the
existing multi-repo worker may continue to iterate repositories in configured or
hinted order, but every per-repo scan must remain bounded.

## Phases

Status legend: ☐ pending · ☑ done

1. ☑ **Phase 1 — Workflow automation contract.**
   `prompts/phase-1-workflow-automation-contract.md`

   Landed optional queue `automation` metadata across raw, validated, and
   compiled workflow models, with validation for actor/transition references,
   actor authorization, artifact-kind compatibility, and merge-conflict fallback
   compatibility. Notable finding: the automation actor stays separate from
   queue subscribers, so declaring mechanical servicing does not by itself create
   an LLM/process role worker or grant external tools.

2. ☑ **Phase 2 — Mechanical automated-queue execution.**
   `prompts/phase-2-mechanical-automated-queue-execution.md`

   Normal mechanical ticks now run bounded reconciliation first and then service
   compiled automated queues through the bounded queue-candidate scanner. Each
   automated item executes through `Executor::execute` with the declared actor
   and transition; stale preconditions and gate misses are logged/counted as
   unchanged while unexpected execution failures still fail the worker. Notable
   finding: merge-conflict-specific fallback remains Phase 3, so current Phase 2
   treats provider merge errors as visible execution errors rather than routing
   them.

3. ☑ **Phase 3 — Typed merge-conflict fallback.**
   `prompts/phase-3-typed-merge-conflict-fallback.md`

   Merge rejections are now classified at the executor boundary: a merge
   `Conflict` response triggers a fresh PR read, already-merged targets continue
   post-merge projection, missing/closed targets are stale, and open/unmerged
   targets return `ExecutionError::MergeConflict`. Mechanical automated queues
   with `on_merge_conflict` route that typed outcome through the declared
   fallback transition, with structured logs for the original transition,
   fallback transition, target, and provider-message summary. Notable finding:
   Forgejo's merge endpoint does not distinguish content conflicts from branch
   protection or other rejection causes, so open/unmerged Forgejo conflicts are
   conservatively engineer-routable for now.

4. ☑ **Phase 4 — Reference-delivery workflow update.**
   `prompts/phase-4-reference-delivery-workflow-update.md`

   The reference-delivery fixture and demo copy now declare the automation-only
   `mechanical` authority, use an automated `landing` queue for approved/current-
   head-green PRs, route merge conflicts to `merge-conflict`, and let engineers
   requeue conflict resolutions without another review request. The fake owner no
   longer services normal merges; owner alignment and architect landed
   reconciliation remain unchanged. Notable finding: memory/filesystem tests need
   synthetic PR-head projection to prove old green CI does not satisfy a
   conflict-resolution head, while real Forgejo gets the new head SHA from the
   engineer's branch push.

5. ☐ **Phase 5 — Wake behavior, docs, and acceptance.**
   `prompts/phase-5-wake-docs-and-acceptance.md`

## Whole-plan acceptance criteria

- A PR cannot be mechanically merged until native reviewer approval and
  current-head CI have both passed.
- An approved PR whose old head had green CI does not merge after a conflict-
  resolution push until the new head has its own green CI job.
- A merge conflict removes the PR from the active landing queue and routes it to
  the engineer; normal mechanical ticks do not retry it until the engineer
  resolves and requeues it.
- The engineer conflict-resolution path requeues for landing without requesting
  a new review.
- An unrelated approved/green PR can still land while another PR is waiting in
  `merge-conflict`; there is no global FIFO/head-of-line block.
- Normal mechanical landing scans use bounded state/label/condition queries and
  do not call deep audit or default all-history issue/PR list queries.
- Webhook/change hints for PR label/review/CI changes wake the mechanical worker
  promptly in production/testing paths, while polling alone still converges.
- Reference-delivery docs, fixture, fake agents, scenario tests, and robustness
  docs describe the new merge path.

## Relevant starting points

- Preceding plan: `plans/bounded-reconciliation-and-correlation-lookups/`
- `crates/temper-workflow/src/spec.rs`
- `crates/temper-workflow/src/validated.rs`
- `crates/temper-workflow/src/validate.rs`
- `crates/temper-workflow/src/compile.rs`
- `crates/temper-workflow/src/plan.rs`
- `crates/temper-workflow/src/execute/`
- `crates/temper-runner/src/worker.rs`
- `crates/temper-runner/src/scan.rs`
- `crates/temper-runner/src/multi_repo.rs`
- `crates/temper-runner/src/trigger.rs`
- `crates/temper-forge/src/forge.rs`
- `crates/temper-forge-forgejo/src/pulls.rs`
- `crates/temper-workflow/fixtures/reference-delivery.json`
- `examples/reference-delivery/config/workflow.json`
- `docs/explanation/reference-workflow.md`
- `docs/reference/workflow-layer.md`
- `docs/reference/robustness-guarantees.md`
- `docs/reference/production-worker.md`
