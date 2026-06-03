# Phase 3 — Reconciliation invariants and Forge-state validator

You are implementing Phase 3 of `plans/observability/README.md` in Temper. The
goal is to make the incident in `plans/observability/evidence.md` fail loudly:
a blocked cross-repo parent with no dependency relations and no children should
be an explicit diagnostic, not a mystery.

## Session bootstrap

1. Confirm you are in `/home/free/src/rust/temper`.
2. Read `README.md`, `AGENTS.md`, `docs/README.md`, and
   `docs/reference/development-conventions.md`.
3. Read:
   - `plans/observability/README.md`
   - `plans/observability/evidence.md`
   - Phase 1 and Phase 2 changes
   - `docs/reference/workflow-layer.md`
   - `docs/reference/cross-repo-workflows.md`
   - `docs/how-to/run-cross-repo-reference-delivery-demo.md`
4. Inspect:
   - `crates/temper-workflow/src/{reconcile,recover,plan}.rs`
   - `crates/temper-runner/src/worker.rs`
   - `crates/temper-workflow/tests/{recovery,reconciliation,reference_delivery}.rs`
   - `examples/reference-delivery/run.sh`
   - `crates/temper-production/src/provision*.rs` and Forgejo CLI helpers used by
     the launcher/validator

## Task

Add named diagnostics for invariant violations and teach the operator validator
to inspect Forge state.

1. **Blocked-without-dependencies diagnostic.** Add a reconciler finding or
   structured mechanical-worker warning for a classified blocked artifact that
   has zero dependency relations. It should explain that dependency-gated
   unblocking intentionally cannot proceed without at least one recorded
   dependency.

2. **Cross-repo parent missing fan-out diagnostic.** Detect reference-delivery
   cross-repo parent issues that are blocked but have fewer child dependencies or
   parent/child metadata links than the configured repo set implies. Keep the
   core workflow layer provider-neutral; demo-specific expectations can live in
   the reference-delivery validator or production demo support.

3. **Mechanical-worker observability.** When reconciliation produces no unblock
   for a blocked artifact, log enough relation/dependency counts to explain why.
   Preserve the existing correctness rule: do not unblock a blocked artifact
   with zero dependency relations.

4. **Forge-state validator.** Extend `examples/reference-delivery/run.sh
   validate-multi-repo` so it checks Forge state, not only logs. It should
   inspect the configured source parent and target repos for:
   - parent exists in the source repo;
   - blocked parent has nonzero dependencies;
   - expected child count equals the configured repo count when cross-repo intake
     is enabled;
   - children carry parent back-reference/correlation metadata;
   - merged/closed children eventually allow the parent to unblock.

   Prefer existing Temper/Forgejo binaries or documented Forgejo APIs already
   wrapped by the repo; do not shell out with raw token-bearing curl.

5. **Failure message.** Ensure the incident shape fails with a message like:

   ```text
   missing: cross-repo parent acme/service#1 expected 2 child dependencies, found 0
   diagnosis: architect blocked the parent but no fan-out side effects were recorded
   ```

6. **Tests.** Add deterministic workflow tests for the new invariant and shell or
   Rust tests for validator parsing where practical. Keep live Forgejo checks
   env-gated.

7. **Plan status.** Mark Phase 3 complete only after validation passes.

## Constraints

- Do not weaken the dependency gate: zero dependencies must not auto-unblock.
- Keep demo-specific cross-repo expectations out of generic workflow semantics.
- Do not log full issue bodies or tokens.
- Do not use raw `curl`/`wget` against Forgejo in repo code or tests; use
  existing Temper/Forgejo abstractions or add a narrow safe helper.

## Done

Run and record at least:

```sh
cargo fmt --all
cargo test -p temper-workflow --all-targets
cargo test -p temper-runner --all-targets
cargo test -p temper-production --all-targets
cargo dev-clippy
cargo dev-check
```

If local Forgejo prerequisites are available, run:

```sh
cd examples/reference-delivery
POLL_MS=120000 RUN_SECS=300 ./run.sh start   # background it if needed
./run.sh validate-multi-repo
./run.sh stop
```

Follow `docs/how-to/end-a-development-session.md` and include validator output in
the handoff.
