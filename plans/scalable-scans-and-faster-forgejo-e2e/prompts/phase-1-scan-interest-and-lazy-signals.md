# Phase 1 prompt — Scan interest model and lazy gate signals

## Goal

Stop every role scan from eagerly reading every runtime gate signal. A role tick
should evaluate only that role's subscribed queues and should fetch CI, review,
and dependency signals only for candidate artifacts whose queue/transition needs
those signals.

This phase may still list broad issue/PR candidates; Phase 2 narrows listing.
The key regression to kill here is: a no-op role tick that has no CI-gated queue
must not call `Forge::list_ci_jobs`.

## Required reading

- `plans/scalable-scans-and-faster-forgejo-e2e/README.md`
- `crates/temper-runner/src/scan.rs`
- `crates/temper-workflow/src/execute/signals.rs`
- `crates/temper-workflow/src/plan/queue.rs`
- `crates/temper-workflow/src/validated.rs` (`GateCondition`)
- `docs/reference/workflow-layer.md`

## Implementation tasks

1. Add a small runtime-signal interest type, probably in `temper-workflow`, e.g.
   `SignalNeeds { dependencies: bool, ci: bool, review: bool }`.
2. Add helpers that derive signal needs from:
   - a queue condition (`DependenciesResolved`, `CiPassed`, `CiFailed`,
     `ReviewApproved`, `ReviewChangesRequested`);
   - a transition's required gates; and
   - a role's subscribed queues.
   Label/state conditions should not request runtime signals.
3. Split queue matching into two stages:
   - cheap kind/label matching;
   - condition matching after fetching only the needed signals.
   Add a public or crate-visible helper if needed; do not duplicate queue logic
   by hand.
4. Add `Executor::read_gate_signals_with_needs` (or equivalent) so callers can
   load only dependency, CI, or review signals as required. Preserve the old
   all-signals behavior where existing callers still need it.
5. Update `scan_role` / `scan_inner` so it does not call `read_gate_signals` for
   every artifact up front. It should fetch signals only after an artifact passes
   cheap queue matching for at least one queue under consideration.
6. Update transition execution planning so it reads only signals required by the
   transition being executed, not always CI+reviews for every PR.

## Tests to add or adjust

- A runner/workflow test using a counting fake Forge proving a role with no
  CI-gated subscribed queue performs zero `list_ci_jobs` calls.
- A test proving `merge_ready`/CI-gated queues still fetch CI and behave
  correctly.
- A test proving review-gated queues fetch reviews but not CI when CI is not
  needed.
- A dependency-gate test proving dependency signals still load when required.
- Existing reference-delivery tests must continue to pass.

## Validation

Run at least:

```sh
cargo fmt --all
cargo test -p temper-workflow
cargo test -p temper-runner
cargo test -p temper-testing --test multiprocess
cargo dev-check
```

## Done when

- No-op role scans avoid unrelated CI/review/dependency reads.
- Existing behavior is preserved for queues/transitions that do require those
  signals.
- This plan README is updated with Phase 1 status and notable findings.
