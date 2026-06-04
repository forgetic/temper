# Bounded reconciliation and correlation lookups — implementation plan

This plan addresses the top two inefficiencies called out in `/home/free/efficiency`:

1. mechanical reconciliation currently lists every issue and pull request with
   `IssueQuery::default()` / `PullRequestQuery::default()`, including closed or
   merged history and dependency enrichment; and
2. idempotent issue/PR creation currently finds an existing correlation key by
   listing every issue or pull request in every state with full detail.

Hand the prompt files to agents **one phase at a time, in order**. Each phase
should land green, update this README's status, and add regressions that would
fail if the broad default-list behavior returned.

## Goals

- Normal mechanical ticks are bounded by active workflow surface area, not total
  repository history.
- Incomplete journal commands are reconciled by exact target reads, even when the
  target no longer has workflow labels.
- Dependency details are loaded only for reconciliation candidates whose recovery
  logic can inspect dependency gates.
- Idempotent issue/PR creation uses state, label, and body/correlation filters
  and summary detail; normal creation paths must not list all historical items.
- A deliberate deep-audit path remains available for rare operator diagnostics,
  but it is separate from the normal mechanical tick.
- Forge/reference docs and backend contract tests describe the new query and
  reconciliation behavior.

## Non-goals and constraints

- Do not make webhooks authoritative. Polling and audit paths must still converge
  from Forge state alone.
- Do not remove exact safety re-reads before mutations.
- Do not hide broad scans inside helper names. If a deep audit lists all history,
  the call site and docs must say so explicitly.
- Do not add Forgejo-specific concepts to `temper-forge`. A public Forge query
  change must be portable and documented in `docs/reference/forge-interface.md`.
- Do not address CI breadth, engineer PR-condition queues, or label-id caching in
  this plan except where touched by tests.

## Design sketch

### Bounded reconciliation

The normal reconciler input should be assembled from:

1. exact snapshots for incomplete journal command targets;
2. workflow-labelled issue/PR candidates discovered with state+label summary
   list queries; and
3. exact full-detail reloads only for candidates whose artifact kind has
   dependency-gated recovery transitions.

The existing pure `Reconciler::scan` remains the decision engine. The change is
how normal runtime code loads `ArtifactSnapshot`s before calling it. A separate
`DeepAudit` mode may keep the old all-history behavior for rare diagnostics.

### Targeted correlation lookup

The creation helpers should search for the correlation key through a bounded
query plan:

1. use labels from the create input when available;
2. search relevant states explicitly (`open`/`closed` for issues,
   `open`/`closed`/`merged` for PRs);
3. request summary list detail; and
4. include a portable body-substring filter once the Forge query contract grows
   one.

Forgejo should use provider-side filters where supported and otherwise apply the
portable body filter after the narrowest available state/label provider query.
The fallback must never silently become `IssueQuery::default()` or
`PullRequestQuery::default()` on the normal path.

## Phases

Status legend: ☐ pending · ☑ done

1. ☑ **Phase 1 — Reconciliation scope and exact journal targets.**
   `prompts/phase-1-reconciliation-scope-and-journal-targets.md`

   Landed bounded journal-only reconciliation, exact incomplete-command target
   loads, deterministic snapshot deduplication, and explicit deep-audit loading.
   Notable finding: runner mechanical ticks still call the deep-audit helper
   intentionally until Phase 3 wires bounded candidate discovery into the normal
   worker path.

2. ☑ **Phase 2 — Bounded reconciliation candidates and lazy dependency detail.**
   `prompts/phase-2-bounded-reconciliation-candidates.md`

   Landed workflow-labelled reconciliation candidate discovery from
   `ValidatedWorkflow`, summary state+label list queries for open/closed issues
   and open/closed/merged PRs, stable candidate deduplication, and exact reloads
   for dependency-gated artifact kinds before dependency status is derived.
   Notable finding: `reconcile_bounded` remains the explicit caller-supplied
   candidate entry point while `reconcile` now performs built-in bounded
   discovery; the mechanical worker still calls deep audit until Phase 3.

3. ☑ **Phase 3 — Wire the mechanical worker and deep-audit mode.**
   `prompts/phase-3-wire-mechanical-worker-and-audit.md`

   Normal `MechanicalWorker::tick` and multi-repo poll/wake ticks now run
   bounded reconciliation. The explicit `tick_deep_audit` path is wired to
   production/testing audit ticks (`--audit-ms`, default 600000 ms, `0` disables)
   and logs mode/snapshot/finding/applied/advisory counts. Multi-repo mechanical
   wakes still visit all configured repositories for cross-repo dependency
   convergence, but each per-repo scan remains bounded unless audit fires.

4. ☑ **Phase 4 — Portable body filter query contract.**
   `prompts/phase-4-body-filter-query-contract.md`

   Landed portable `body_contains` filters on issue and pull-request list
   queries, documented exact-substring semantics (`Some("")` is no filter), and
   covered in-memory, filesystem, and Forgejo backends with contract tests.
   Notable finding: Forgejo 7.0.x does not provide reliable exact body search, so
   the backend applies the filter client-side after preserving existing
   state/label provider narrowing; labelled PR queries still use the issue label
   index and never fall back to `/pulls?state=all`.

5. ☑ **Phase 5 — Targeted correlation lookups in executor and role tools.**
   `prompts/phase-5-targeted-correlation-lookups.md`

   Landed shared correlation lookup planning for executor and runner role-tool
   helpers: normal create lookups now query explicit issue/PR states with
   summary detail, create labels when available, and an escaped metadata body
   marker before exact metadata confirmation. Parent repair still uses the
   compare-and-swap body update after a targeted match, and crash-after-create
   retries remain covered. Notable finding: compatibility discovery of legacy
   artifacts that lack the create labels is intentionally not part of the normal
   path.

6. ☑ **Phase 6 — Forgejo regressions, docs, and performance acceptance.**
   `prompts/phase-6-forgejo-docs-and-acceptance.md`

   Landed Forgejo mock-contract regressions for bounded reconciliation and
   labelled correlation lookup request shapes, updated durable reference docs,
   and recorded before/after caveats in `findings.md`. Notable finding: Forgejo
   7.0.x still lacks reliable exact provider-side body search, so the backend
   keeps state/label provider narrowing and applies `body_contains` client-side.

## Whole-plan acceptance criteria

- ☑ A normal mechanical tick over a repo with many closed unlabelled issues/PRs
  does not issue default all-state issue/PR list queries.
- ☑ An incomplete journal command for an unlabelled target is still reconciled by
  exact `get_*_by_number` reads.
- ☑ Dependency-gated blocked work still unblocks after exact dependency reads; a
  summary candidate list must not produce false `BlockedWithoutDependencies`.
- ☑ `Executor::ensure_issue`, `Executor::ensure_issue_with_parent`,
  `Executor::ensure_pull_request`, and the `RoleTools` correlation find helpers
  do not call default all-history lists on the normal labelled path.
- ☑ Crash-after-create retry tests still prove no duplicate issue/PR for one
  correlation key.
- ☑ Forgejo mock tests prove the hot paths use explicit state/label/body filters
  and do not start with `/issues?state=all` or `/pulls?state=all` broad scans.
- ☑ Documentation states which paths are bounded and which operator-invoked paths
  are deep audits.

## Relevant starting points

- `crates/temper-workflow/src/reconcile.rs`
- `crates/temper-workflow/src/execute/ensure.rs`
- `crates/temper-runner/src/worker.rs`
- `crates/temper-runner/src/agent.rs`
- `crates/temper-runner/src/scan/candidate.rs`
- `crates/temper-forge/src/forge.rs`
- `crates/temper-forge-memory/src/operations.rs`
- `crates/temper-forge-filesystem/src/operations.rs`
- `crates/temper-forge-forgejo/src/{issues,pulls}.rs`
- `docs/reference/forge-interface.md`
- `docs/reference/workflow-layer.md`
- `docs/reference/forgejo-backend.md`
- `docs/reference/in-memory-backend.md`
- `docs/reference/filesystem-backend.md`
