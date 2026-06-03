# Phase 1 — Trace context and structured event foundations

You are implementing Phase 1 of `plans/observability/README.md` in Temper. The
goal is to give every workflow-observability event a stable, provider-neutral
identity and to propagate that identity to Smith through the existing
`work_item_context` authority-neutral JSON.

## Session bootstrap

1. Confirm you are in `/home/free/src/rust/temper`.
2. Read `README.md`, `AGENTS.md`, `docs/README.md`, and
   `docs/reference/development-conventions.md`.
3. Read the relevant lessons:
   - `docs/reference/agent-lessons/0015-start-downstream-wake-sockets-before-seeding-work.md`
   - `docs/reference/agent-lessons/0017-cross-repo-demo-needs-closing-architect.md`
   - `docs/reference/agent-lessons/0021-user-defined-roles-own-prompt-behavior.md`
4. Read `plans/observability/README.md` and `plans/observability/evidence.md`.
5. Read the process/workflow contracts:
   - `docs/reference/workflow-role-decision-process-protocol.md`
   - `docs/reference/workflow-layer.md`
   - `docs/reference/cross-repo-workflows.md`
6. Inspect the relevant code with `rg` before editing:
   - `crates/temper-runner/src/role_process_tools.rs`
   - `crates/temper-runner/src/role_decision_process.rs`
   - `crates/temper-runner/src/worker.rs`
   - `crates/temper-runner/src/scan.rs`
   - `crates/temper-production/src/worker.rs`

## Task

Add the foundation for structured observability without changing workflow
authority or introducing a telemetry backend.

1. **Trace/work-item identity.** Add a small provider-neutral identity model in
   `temper-runner` for fields such as `work_item_id`, `decision_id`, `tick_id`,
   `repo`, `role`, `queue`, `artifact_type`, `artifact_number`, and
   `artifact_kind`. Keep IDs deterministic or locally unique enough for logs;
   do not use Forge credentials, provider secrets, or artifact body text.

2. **Work item context propagation.** Extend the JSON built by
   `build_work_item_context` so Smith receives an `observability` object with the
   identity fields Temper can know. This should not add Forge mutation authority
   and should not require Smith to execute tools.

3. **Structured event formatter.** Add a focused helper for stable key/value or
   JSON event rendering. Prefer testable pure functions over ad-hoc string
   interpolation. Include helpers for bounded previews/redaction of reasons and
   long text.

4. **Startup/capability event.** Emit a safe worker startup or role-capability
   summary that includes the worker kind, role, resolved repository set,
   responder mode, authorized action count/names, and bound external-tool ids
   where available. Do not log command argv values that may contain secrets.

5. **Tests.** Add focused unit tests for identity rendering, work-item context
   shape, stable event output, preview truncation, and secret-like value
   exclusion. Update existing fixtures only if the protocol/request shape truly
   changed; prefer keeping protocol compatibility by nesting trace data inside
   `work_item_context`.

6. **Plan status.** Mark Phase 1 complete in `plans/observability/README.md`
   only after validation passes.

## Constraints

- Temper must remain provider-neutral and must not depend on Smith.
- Do not add OpenTelemetry, metrics servers, or log subscribers in this phase.
- Do not log full issue/PR bodies, auth files, tokens, provider secrets, or raw
  Smith argv.
- Keep source files under the repository size guidance; split helpers if needed.

## Done

Run and record at least:

```sh
cargo fmt --all
cargo test -p temper-runner --all-targets
cargo test -p temper-production --all-targets
cargo dev-clippy
cargo dev-check
```

If broader tests fail, diagnose whether the change broke protocol fixtures or
logging assertions. Follow `docs/how-to/end-a-development-session.md` and include
validation results plus any deferred live gates in the handoff.
