# Mapped live focused-test source relevance

This checked-in scenario is the dedicated mapping for feature `ai/temper#1130`
and plan `ai/temper#1131` on `agent/pr-for-feature-1130`. It inherits the
existing retry-affinity repository, workflow, CI, and real live topology without
changing the historical `mapped-live-decision-gap-recovery` mapping for #1091.

## Live contract

A real Forgejo instance, host Actions runner, standalone Temper process, Jig
engineer, and deterministic current-root provider perform one minimal repair.
The engineer consumes typed implementation and caller sources, exhausts ordinary
focused-test discovery with one complete empty traversal, and takes one semantic
fallback on that same active root.

The fallback reports the closed `eligible_selector_returned` outcome with
complete carry-forward lineage. A typed source attempt against a different
returned purpose is denied locally and never reaches MCP. The later exact source
consumer explicitly carries `focused_test`, completes the evidence chain, and
precedes conventional reading, one exact patch, host submission, Actions, merge,
and source closure.

The ephemeral validator checks the exact provider inventory and closed checkpoint
order. Together with manifest event assertions, it proves implementation,
caller, and focused-test kinds, empty-to-eligible fallback progress, same-scope
carry-forward ordering, exact source consumption, and the fail-closed mismatch.

## Privacy boundary

Checked-in declarations and aggregate evidence retain only safe tool counts,
closed checkpoint categories, correlation and lineage completeness, lineage
stage, focused-test discovery outcome, decision kind, ordering, current-root
binding facts, local-denial category, and gate outcomes. Runtime prompts,
selectors, opaque roots, source, paths, provider output, credentials, arguments,
host output, and diagnostic traces remain ephemeral. Generated runtime evidence
must not be committed.

## Validation

From the exact assembled #1130 feature head, run:

```sh
cargo dev-scenario-check
cargo dev-scenario-run scenarios/mapped-live-focused-test-source-relevance
cargo dev-scenario-validate-feature \
  --feature ai/temper#1130 \
  --landing-base origin/main \
  --source-branch agent/pr-for-feature-1130 \
  --pr <scenario-pr-number> \
  --sha "$(git rev-parse HEAD)" \
  --output-dir target/focused-validation
./.temper/pre-pr
cargo dev-benchmark-harness
```

The routing-repair benchmark fixture, manifest, provider, task, expected patch,
acceptance policy, verifier, privacy policy, and thresholds remain frozen. The
unchanged harness summary must retain implementation, caller, and focused-test
decision kinds, focused-test source consumption, and exact selection of
`repo/src/route.rs`, with deterministic counts, complete coverage, privacy,
exact patching, and all fail-closed cases. Keep every generated run artifact and
diagnostic outside the scenario corpus.
