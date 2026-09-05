# Mapped live decision-gap recovery

This active checked-in scenario maps the self-contained feature and plan
`ai/temper#1275` on
`agent/pr-for-feature-1275`. It updates the scenario in place without changing
its `introduced_by = "#1075"` provenance. Feature `ai/temper#1091`, plan
`ai/temper#1092`, and source branch `agent/pr-for-feature-1091`, plus the former
feature `ai/temper#1069`, plan `ai/temper#1070`, and source branch
`agent/pr-for-feature-1069`, remain explicit historical audit metadata. The historical `mapped-live-graph-consumption`,
`mapped-live-graph-convergence`, and `mapped-live-ordinary-tool-convergence`
mappings also keep their original feature, plan, and branch identities.

## Live contract

A real Forgejo instance, host Actions runner, standalone Temper process, Jig
engineer, and deterministic current-root provider perform one minimal
retry-affinity repair. Stable call order selects the first root as active while
a second typed root remains available for a route pivot. The first root also precedes one
same-turn shell barrier, which is denied without execution and retains #1082's
closed `excluded_never_executed_local_policy_denial` classification.

One cross-root probe remains local without allowance spend and surfaces the
four-allowance recovery guidance. The next immutable recovery batch admits
first-root implementation evidence while cross-root caller and focused-test
attempts remain local without allowance spend. Its next trace returns no eligible caller, exhausting only that root's
implementation route. Recovery pivots to the second root, where implementation,
an eligible caller trace, and exact caller source consume three calls. With one
allowance left, the first root remains eligible as the independent focused-test
route. Its exact test source completes the forest instead of producing
`decision_anchor_recovery_exhausted`.

Three subsequent graph attempts remain local. The matching patch and an
unrelated mutation are denied before an exact ordinary route read. Conventional
classified shell discovery, that exact source read, exactly one matching patch,
host submission, Actions, merge, and source closure remain available.

The ephemeral validator checks eight successful provider reads in the exact
root/route-pivot groups and proves that locally denied calls never reach MCP. It
also checks the no-compatible-action variant: the actual
remaining implementation, caller, and focused-test kinds terminate with zero
allowance, no compatible action, `stop_without_product`, and no landable result
or repeated denial loop. The scripted patch, host validation command, and process
canary prove the exact one-file repair and non-execution contract.

## Privacy-safe evidence

Checked-in declarations and aggregate evidence retain only tool counts and
ordering, complete correlation/lineage stages, closed decision-evidence kinds,
missing-kind/action/allowance fields, current-root binding facts, closed shell
classification, approved checkpoint categories, and gate outcomes. Provider
values, selectors, opaque roots, source, prompts, commands, arguments,
credentials, paths, target digests, provider output, host-gate output, and
diagnostic traces remain ephemeral. Generated runtime evidence must not be
committed.

## Validation

From the exact assembled #1275 feature head, first run the preserved mappings
and this strengthened mapping separately:

```sh
cargo dev-scenario-check
cargo dev-scenario-run scenarios/mapped-live-graph-consumption
cargo dev-scenario-run scenarios/mapped-live-graph-convergence
cargo dev-scenario-run scenarios/mapped-live-ordinary-tool-convergence
cargo dev-scenario-run scenarios/mapped-live-decision-gap-recovery
cargo dev-scenario-run scenarios/mapped-live-exact-source-selection
cargo dev-scenario-validate-feature \
  --feature ai/temper#1275 \
  --landing-base origin/main \
  --source-branch agent/pr-for-feature-1275 \
  --pr <scenario-pr-number> \
  --sha "$(git rev-parse HEAD)" \
  --output-dir target/focused-validation-1275
```

Then freeze the unchanged
`benchmarks/agent-sessions/codebase-memory-routing-repair/benchmark.toml` and run,
without selective reruns or reordering, one wholly fresh enabled smoke, five
fresh enabled repetitions, five fresh disabled repetitions, and five fresh
forced-unavailable repetitions. Invoke `temper-benchmark verify` exactly once
over those immutable roots and the exact aggregate commit.

Every included run must pass correctness, the byte-exact patch, host validation,
at least 50% typed relevance, complete compound-shell classification, and
unavailable fallback with zero immediate graph retry. The aggregate must also
show at least 20% enabled median discovery improvement, identical annotations,
privacy, and exact-commit gates. A manifest, candidate, provider, model,
annotation, or root change invalidates the full matrix. Commit only the
privacy-reviewed aggregate evidence; failed roots, raw diagnostics, and runtime
evidence stay outside the scenario corpus.
