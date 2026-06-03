# Phase 4 — Operator docs and observability smoke proof

You are implementing Phase 4 of `plans/observability/README.md` in Temper. The
goal is to make the observability story durable for operators and to prove that
the reference-delivery topology leaves an intelligible trail when it moves or
stalls.

## Session bootstrap

1. Confirm you are in `/home/free/src/rust/temper`.
2. Read `README.md`, `AGENTS.md`, `docs/README.md`, and
   `docs/reference/development-conventions.md`.
3. Read `plans/observability/README.md`, `plans/observability/evidence.md`, and
   all prior phase changes.
4. Read/update-relevant docs:
   - `examples/reference-delivery/README.md`
   - `docs/how-to/run-cross-repo-reference-delivery-demo.md`
   - `docs/reference/workflow-role-decision-process-protocol.md`
   - `docs/reference/workflow-layer.md`
   - `docs/reference/llm-agents.md`
   - Smith's sibling observability plan for cross-repo wording:
     `~/src/rust/smith/plans/observability/README.md`
5. Inspect existing gated tests and smoke helpers under `crates/temper-testing/`
   and `examples/reference-delivery/run.sh`.

## Task

Document and prove the observability workflow.

1. **Operator guide.** Update reference-delivery docs so an operator knows where
   to look for:
   - worker startup/capability summary;
   - scan/work-item events;
   - Smith decision request/reply correlation;
   - transition/effect outcomes;
   - mechanical reconciliation findings;
   - validator diagnostics for missing fan-out or zero dependencies.

2. **Protocol/reference docs.** Update Temper-owned reference docs to describe
   the authority-neutral observability fields that may appear in
   `work_item_context` and the rule that Smith may log/capture them but receives
   no Forge mutation tools.

3. **Smoke proof.** Add or extend a deterministic smoke/e2e test so a small
   workflow run asserts the presence of enough observability events to diagnose
   movement. If testing a stuck incident shape is feasible, add a fixture that
   deliberately creates a blocked parent with zero dependencies and asserts the
   validator/reconciler diagnostic.

4. **Evidence update.** Append a short note to `plans/observability/evidence.md`
   describing how the new observability would have diagnosed the original
   incident.

5. **Plan closure.** Update `plans/observability/README.md` statuses and whole
   plan acceptance criteria only when all phases are complete.

## Constraints

- Keep docs focused; split pages before they grow too large.
- Do not make live Forgejo/provider tests required by default.
- Do not duplicate Smith implementation details in Temper docs; link to the
  Smith plan/docs for provider-owned observability.
- Keep logs redacted and bounded.

## Done

Run and record at least:

```sh
cargo fmt --all
cargo test --workspace --all-targets
cargo dev-clippy
cargo dev-check
```

Run the focused smoke/e2e test you added. If Forgejo prerequisites are available,
run the reference-delivery validator path and include representative log/validator
snippets. Follow `docs/how-to/end-a-development-session.md` for the handoff.
