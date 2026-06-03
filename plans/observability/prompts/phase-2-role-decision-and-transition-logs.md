# Phase 2 — Role-decision and transition execution logs

You are implementing Phase 2 of `plans/observability/README.md` in Temper. The
goal is to make a worker's `actions=N` summary explain *which* work item moved,
*which* Smith action was selected, and *which* Temper transition/effects ran.

## Session bootstrap

1. Confirm you are in `/home/free/src/rust/temper`.
2. Read `README.md`, `AGENTS.md`, `docs/README.md`, and
   `docs/reference/development-conventions.md`.
3. Read `plans/observability/README.md`, `plans/observability/evidence.md`, and
   the Phase 1 changes.
4. Read:
   - `docs/reference/workflow-role-decision-process-protocol.md`
   - `docs/reference/workflow-layer.md`
   - `docs/how-to/run-cross-repo-reference-delivery-demo.md`
5. Inspect:
   - `crates/temper-runner/src/role_decision_process.rs`
   - `crates/temper-runner/src/role_process_tools.rs`
   - `crates/temper-runner/src/agent.rs`
   - `crates/temper-workflow/src/execute/`
   - `crates/temper-production/src/worker.rs`

## Task

Instrument the production decision and execution path with structured, bounded
logs.

1. **Decision request event.** Before invoking the process responder, log repo,
   artifact, role, queue, workflow id, work item id, decision id, authorized
   actions, and available external-tool ids. Include no request body text beyond
   safe identifiers/counts.

2. **Decision reply event.** After the process returns, log selected action,
   final validation outcome, `no_action` vs authorized action, reason preview,
   and latency. Distinguish process failure, malformed JSON, protocol mismatch,
   unauthorized action downgraded to no-action, and timeout.

3. **Action dispatch event.** When Temper maps a selected action to a manifest
   tool, log the transition id and whether any external executor is required
   (for example `coding_workspace`). If a required executor is unavailable, log
   a clear no-op reason.

4. **Transition execution event.** Around `RoleTools`/executor calls, log the
   target artifact, transition id, gate/precondition failure class, effect list
   summary, postcondition outcome, stale-work no-op, or successful mutation.
   Keep effect summaries compact; do not log full comment bodies or PR bodies.

5. **Tick summary linkage.** Keep the existing production tick summary, but make
   it easy to correlate `completed tick trigger=... actions=N` with the per-item
   events from the same tick/work item.

6. **Tests.** Add focused tests for event rendering and for process-adapter
   branch distinctions. Where direct stderr assertions are brittle, test the pure
   formatting helper and adapter outcome classification.

7. **Plan status.** Mark Phase 2 complete only after validation passes.

## Constraints

- Do not give Smith or the model new authority. It still chooses only one
  manifest action or `no_action`.
- Preserve stale-work behavior: stale execution should be observable but should
  remain a safe no-op.
- Do not expose secrets, raw provider args, full artifact bodies, raw comments,
  or auth paths.
- Avoid changing the process protocol unless Phase 1 explicitly introduced a
  compatible trace shape.

## Done

Run and record at least:

```sh
cargo fmt --all
cargo test -p temper-runner --all-targets
cargo test -p temper-workflow --all-targets
cargo test -p temper-production --all-targets
cargo dev-clippy
cargo dev-check
```

If you can run the reference-delivery demo locally, also perform a short smoke
and capture representative log snippets showing decision and transition events.
Follow `docs/how-to/end-a-development-session.md` for the handoff.
