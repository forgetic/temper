# Cross-repo workflows — implementation plan

> **Scheduling note:** do not execute this plan until
> `plans/multi-repo-workers/` is complete. That plan builds the fixed
> worker-pool fleet that serves many repositories independently. Cross-repo
> support is a strict layer on top of that fleet: it needs workers that already
> cover every involved repository and a single identity with permission across
> them. Starting before the fleet exists would mean building the fleet
> implicitly and entangled, with worse failure-localization.

Today a workflow issue is processed entirely within its own repository: the
architect breaks it down into child issues in the **same** repo, and every
relation/dependency reference is a bare item number scoped to that one repo.
This plan lets a single intake issue produce work that spans **multiple**
repositories.

Hand the prompt files to the agent loop **one phase at a time, in order**. Each
phase should land green and update this README's status.

**Status:** complete. All six phases are done; the cross-repo reference workflow
is implemented, tested, documented, and demonstrated by the reference-delivery
example.

## Goal

An operator files one workflow issue in repository A. If the work spans more
than one repository, the architect produces a plan that creates child issues in
the appropriate repositories (possibly A, B, C…). Each child issue is then
serviced by the normal per-repo roles — engineer claims one child issue and
produces one PR in that child's repo, reviewer/tester act per-repo — exactly as
today. The original intake issue stays blocked until every child across every
repo has landed, then it resolves.

The execution roles do not change. Cross-repo is a property of **planning and
links**, concentrated in:

- a repo-qualified artifact reference that can point across repositories;
- architect fan-out that creates child issues in other repos, idempotently;
- dependency aggregation that resolves a parent against children in other repos.

## Non-goals

- Atomic cross-repo merges, transactions, or two-phase commit. Children land
  independently; the parent only aggregates their terminal states.
- Cross-repo code movement, shared branches, or monorepo semantics.
- Per-repository workflow definitions. All repos still use the compiled
  reference workflow.
- Routing/fairness beyond what `multi-repo-workers` already provides.
- Changing the `Forge` trait unless a phase proves an unavoidable portable gap
  (cross-repo native dependency links are the most likely candidate; see Phase 4).

## Design constraints

- Keep `harness-forge` backend-agnostic. A cross-repo reference is a portable
  `(RepositoryId, ItemNumber)` pair, not a Forgejo/GitHub URL.
- Preserve same-repo behavior as the default. Every reference-model change must
  keep an unqualified/same-repo reference working unchanged so existing tests and
  single-repo deployments are untouched.
- Idempotency is non-negotiable. The architect re-runs every tick (level
  triggered); fan-out must never duplicate child issues across re-scans, even
  across process restarts and target-repo races.
- Authority to write to a target repo is gated by the worker token's Forge
  permission on that repo, not by the worker's assigned scan shard (the
  forward-compatibility constraint recorded in `plans/multi-repo-workers/`
  Phase 3).
- Aggregation must read fresh Forge state from each child's repo; never infer a
  child's terminal state from cached or parent-local data.
- Keep planner purity: the planner still receives a reduced `DependencyStatus`
  and only tests set membership. Cross-repo resolution happens in the runtime
  reader, not the planner.

## Phases

Status legend: ☐ pending · ☑ done

1. ☑ **Phase 1 — Repo-qualified `ArtifactRef` reference model.**
   `prompts/phase-1-artifact-ref.md`

   Introduce a portable repo-qualified reference and thread it through the
   classifier's relations, the metadata schema, and the dependency target set,
   defaulting to same-repo so all existing behavior is unchanged. Land the ADR
   that this model hangs off. Memory-backed unit tests only. Done: ADR 0021
   records `ArtifactRef` as the workflow-layer `(RepositoryId, ItemNumber)`
   reference with a same-repo shorthand; metadata relation fields parse old
   bare numbers and new repo-qualified objects; classification and dependency
   gate signals now preserve repo-qualified targets while runtime resolution
   still handles same-repo targets only until Phase 4.

2. ☑ **Phase 2 — Cross-repo idempotent issue creation + global correlation.**
   `prompts/phase-2-cross-repo-create.md`

   Add an `Agent`/`RoleTools` capability to create-or-find an issue in a named
   target repo keyed by a globally-unique correlation key, with permission-gated
   authority. Prove no duplication across re-scans and target-repo races. Done:
   `global_child_correlation_key` defines the length-prefixed global parent +
   child-intent key, `RoleTools::ensure_issue_in_repo` targets an explicit repo
   and embeds a repo-qualified parent back-reference, and memory/filesystem
   tests prove repeated calls, pre-existing-key repair, visibility errors, and
   distinct-handle target-repo races converge on one child issue.

3. ☑ **Phase 3 — Architect fan-out planning.**
   `prompts/phase-3-architect-fanout.md`

   Give the architect a plan format whose child items carry a target repo, the
   create-in-repo tool, and the parent→child cross-repo links. Update the
   architect prompt/decision parsing. Prove a single intake issue fans out into
   child issues across repos. Done: real architect decisions now accept optional
   child issue plans with per-child `target_repo`, create ready code children via
   `RoleTools::ensure_issue_in_repo` using `global_child_correlation_key`, and
   add parent metadata dependencies pointing at repo-qualified children; the fake
   architect mirrors this through deterministic `harness:architect-plan` blocks,
   with memory-backed tests proving cross-repo fan-out is idempotent and plain
   same-repo triage is unchanged.

4. ☑ **Phase 4 — Cross-repo dependency aggregation.**
   `prompts/phase-4-cross-repo-aggregation.md`

   Generalize the dependency-state reader to resolve each target in its own
   repo (repo-qualified metadata fallback; the portable native dependency-link
   trait remains same-repository), and prove the parent intake issue
   unblocks/resolves only when every cross-repo child has landed. Reconciler
   `Unblock` works across repos. Done: dependency aggregation now reads every
   repo-qualified target from its resolved repository, treats transient child
   reads as not-landed with a recorded `DependencyStatus` read failure, keeps the
   planner pure, and memory/filesystem tests prove all-children-landed unblock,
   post-apply single firing, and transient read failure safety.

5. ☑ **Phase 5 — End-to-end cross-repo scenario.**
   `prompts/phase-5-e2e.md`

   Add a reference-delivery scenario where one intake issue spans two repos and
   converges on the multi-repo fleet. Deterministic filesystem/process test plus
   a gated real-Forgejo variant. Done: `cross_repo_fanout_converges` seeds one
   planned intake in repo A, creates children in repos A and B, verifies both
   child PRs merge in their own repos before the parent resolves, and now runs in
   the in-process memory/filesystem fleet, the filesystem multiprocess fleet,
   the gated Forgejo multiprocess twin (fake and real agents), and the gated
   Forgejo webhook/wake multi-repo regression.

6. ☑ **Phase 6 — Examples and operator docs.**
   `prompts/phase-6-examples-and-docs.md`

   Done: `docs/explanation/cross-repo-workflows.md` explains architect fan-out,
   repo-qualified references, per-repo execution, and parent aggregation;
   `docs/reference/cross-repo-workflows.md` records the global child correlation
   key and relation/dependency contracts;
   `docs/how-to/run-cross-repo-reference-delivery-demo.md` gives the operator
   recipe. `examples/reference-delivery/` now defaults to a
   two-repo cross-repo intake demo (`CROSS_REPO_INTAKE=auto`), provisioning all
   repos while seeding one parent issue in the source repo and documenting the
   walkthrough.

## Acceptance criteria

- A portable repo-qualified reference exists; same-repo references behave
  exactly as before, with an ADR recording the decision.
- Architect fan-out creates child issues across repos with zero duplication
  across re-scans, restarts, and target-repo contention.
- A parent intake issue resolves only after every cross-repo child has landed,
  proven on memory and filesystem backends.
- One end-to-end scenario converges a two-repo intake issue on a single fixed
  worker fleet (deterministic test), with a gated real-Forgejo twin.
- Execution roles (engineer/reviewer/tester) are unchanged; only planning,
  references, and aggregation are touched.
- `cargo fmt --all`, `cargo dev-clippy`, and `cargo dev-check` pass at each
  phase; default tests remain hermetic.

## Relevant starting points

- `plans/multi-repo-workers/README.md` — must be complete before this starts.
- `crates/harness-workflow/src/artifact.rs` — `ArtifactSource` (repo-implicit today).
- `crates/harness-workflow/src/classify.rs` — `ClassifiedRelation.target: ItemNumber`.
- `crates/harness-workflow/src/dependency_state.rs` — resolves targets in one repo.
- `crates/harness-workflow/src/relation.rs` — `RelationKind`.
- `crates/harness-workflow/src/metadata.rs` — metadata schema/correlation keys.
- `crates/harness-runner/src/agent.rs` — `ensure_pull_request`/correlation seam.
- `crates/harness-agents/src/architect.rs` — fan-out decision/prompt.
- `crates/harness-forge/src/forge.rs` — dependency links (`add_dependency`, etc.).
- ADR index under `docs/adr/` — Phase 1 lands the cross-repo reference ADR.
