# Mapped live decision-gap recovery

This active checked-in scenario maps feature `ai/temper#1069` and plan
`ai/temper#1070` on `agent/pr-for-feature-1069`. It is additive: the historical
`mapped-live-graph-consumption`, historical `mapped-live-graph-convergence`, and
historical `mapped-live-ordinary-tool-convergence` mappings keep their original
feature, plan, and branch identities.

## Live contract

A real Forgejo instance, host Actions runner, standalone Temper process, Jig
engineer, and deterministic current-root provider perform one minimal
retry-affinity repair. Two independent roots lead to implementation refinement,
caller trace, and a typed focused-test source. Caller source evidence remains
missing when two duplicate refinements exhaust the ordinary non-progress budget.

The runtime then reports exactly `caller` as missing, permits only
`targeted_current_root_graph_call`, and retains allowance four. A broad search
and duplicate refinement are denied locally with those same closed fields and
never reach MCP. One typed current-root caller source is admitted, completes the
chain, and causes three post-completion graph attempts to be denied locally.
Conventional reading, the exact one-file repair, host submission, Actions, merge,
and source closure remain available.

The ephemeral validator checks the exact eight-call provider chain, one admitted
recovery source, two local recovery denials, and closed aggregate checkpoints.
It also checks the protocol's exhausted state: caller remains named, allowance
is zero, the only permitted action is `stop_without_product`, and no result can
be represented as landable evidence.

## Privacy-safe evidence

Checked-in declarations and aggregate evidence retain only tool counts and
ordering, complete correlation/lineage stages, closed decision-evidence kinds,
missing-kind/action/allowance fields, current-root binding facts, checkpoint
categories, and gate outcomes. Provider values, selectors, source, prompts,
commands, credentials, paths, target digests, host-gate output, and diagnostic
traces remain ephemeral. Generated runtime evidence must not be committed.

## Validation

From the exact assembled feature head, run the preserved mappings and this one
separately:

```sh
cargo dev-scenario-check
cargo dev-scenario-run scenarios/mapped-live-graph-consumption
cargo dev-scenario-run scenarios/mapped-live-graph-convergence
cargo dev-scenario-run scenarios/mapped-live-ordinary-tool-convergence
cargo dev-scenario-run scenarios/mapped-live-decision-gap-recovery
cargo dev-scenario-validate-feature \
  --feature ai/temper#1069 \
  --landing-base origin/main \
  --source-branch agent/pr-for-feature-1069 \
  --pr <aggregate-pr-number> \
  --sha "$(git rev-parse HEAD)" \
  --output-dir target/focused-validation
```

The fresh enabled smoke, immutable enabled/disabled/forced-unavailable five-run
matrix, and gate-bearing benchmark verifier remain separate final gates. Keep
failed runs and all diagnostic detail outside the checked-in scenario corpus.
