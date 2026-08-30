# Mapped live decision-gap recovery

This active checked-in scenario maps feature `ai/temper#1091` and plan
`ai/temper#1092` on `agent/pr-for-feature-1091`. It updates the scenario in place
without changing its `introduced_by = "#1075"` provenance. The former feature
`ai/temper#1069`, plan `ai/temper#1070`, and source branch
`agent/pr-for-feature-1069` remain explicit historical audit metadata for the
original decision-gap contract. The historical `mapped-live-graph-consumption`,
`mapped-live-graph-convergence`, and `mapped-live-ordinary-tool-convergence`
mappings also keep their original feature, plan, and branch identities.

## Live contract

A real Forgejo instance, host Actions runner, standalone Temper process, Jig
engineer, and deterministic current-root provider perform one minimal
retry-affinity repair. Stable call order selects the routing root as active while
a second typed root owns focused behavior. The first root also precedes one
same-turn shell barrier, which is denied without execution and retains #1082's
closed `excluded_never_executed_local_policy_denial` classification.

Two focused-test reads bound to the behavioral sibling reach the provider on
separate turns. They exhaust normal exploration but cannot advance or outrank
the active routing root. The next model turn submits one immutable mixed
recovery batch: cross-root caller and focused-test reads surround a compatible
active-root trace. Both cross-root calls are denied locally from the same
pre-batch snapshot, without MCP invocation or allowance spend. Only the trace
reaches MCP, changing the diagnostic from
`trace,implementation,caller,focused_test` with allowance four to
`implementation,caller,focused_test` with allowance three.

A later active-root batch admits one implementation, caller, and focused-test
source while denying an already-satisfied trace from the same immutable
snapshot. Batch settlement is independent of provider completion order. The
three admitted calls consume the three remaining slots and complete the chain.
Three subsequent graph attempts remain local; conventional classified shell and
source reads, exactly one patch, host submission, Actions, merge, and source
closure remain available.

The ephemeral validator checks eight successful provider reads in the exact
root/sibling/trace groups and accepts every completion order for the final three
active-root sources. It also checks the no-compatible-action variant: the actual
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

From the exact assembled #1091 feature head, first run the preserved mappings
and this strengthened mapping separately:

```sh
cargo dev-scenario-check
cargo dev-scenario-run scenarios/mapped-live-graph-consumption
cargo dev-scenario-run scenarios/mapped-live-graph-convergence
cargo dev-scenario-run scenarios/mapped-live-ordinary-tool-convergence
cargo dev-scenario-run scenarios/mapped-live-decision-gap-recovery
cargo dev-scenario-validate-feature \
  --feature ai/temper#1091 \
  --landing-base origin/main \
  --source-branch agent/pr-for-feature-1091 \
  --pr <scenario-pr-number> \
  --sha "$(git rev-parse HEAD)" \
  --output-dir target/focused-validation
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
