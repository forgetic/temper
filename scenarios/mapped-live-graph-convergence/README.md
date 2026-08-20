# Mapped live graph convergence

This active checked-in scenario maps feature `ai/temper#1026` and plan
`ai/temper#1027` on `agent/pr-for-feature-1026`. It is distinct from and does
not replace the historical `mapped-live-graph-consumption` mapping for
`#1009`/`#1010` on `agent/pr-for-feature-1009`.

## Live contract

A real Forgejo instance, host Actions runner, standalone Temper process, and Jig
engineer perform one minimal retry-affinity repair. The first current-root chain
reaches an unavailable source before completion. The wrapper emits a fixed safe
category, the agent falls back conventionally, and no graph call immediately
retries the provider.

The agent then retains two useful independent search roots and consumes a
refinement, caller trace, one deliberately duplicate refinement, and current-root
implementation and focused-test sources. Completing that evidence injects the
generic privacy-safe convergence instruction. The Jig then attempts broad,
duplicate, and post-decision graph work. All three attempts receive the closed
local `graph_lifecycle_denial` category with reason `exploration_closed` and none reaches the temporary provider.
Conventional source reads remain available before the exact one-file repair,
host validation, Actions pass, PR merge, and source-issue closure.

The fixture provider mints every symbol and binding value at runtime. Its
validator checks the exact closed provider-call/checkpoint order, including the
pre-completion unavailable result and the useful duplicate that did reach MCP.
The shorter MCP inventory plus three local categories prove post-decision work
did not invoke the provider.

## Privacy-safe evidence

Checked-in declarations and aggregate evidence retain only tool counts and
ordering, complete typed correlation/lineage stages, current-root binding facts,
closed fixture checkpoints, local-denial categories, and gate outcomes. Prompts,
provider results, commands, source, target values, digests, credentials, local
paths, run identifiers, and diagnostic traces remain private. Generated runtime
evidence must not be committed to this scenario bundle.

## Validation

From the exact assembled feature head, run the preserved historical mapping and
this new mapping separately:

```sh
cargo dev-scenario-check
cargo dev-scenario-run scenarios/mapped-live-graph-consumption
cargo dev-scenario-run scenarios/mapped-live-graph-convergence
cargo dev-scenario-validate-feature \
  --feature ai/temper#1026 \
  --landing-base origin/main \
  --source-branch agent/pr-for-feature-1026 \
  --pr <aggregate-pr-number> \
  --sha "$(git rev-parse HEAD)" \
  --output-dir target/focused-validation
```

The benchmark smoke, frozen fresh 5×3 live matrix, and typed exact-head verifier
remain separate required gates. Commit a new privacy-reviewed aggregate report
only after every declared acceptance gate passes; failed or unavailable gates
must not claim deployment approval.
