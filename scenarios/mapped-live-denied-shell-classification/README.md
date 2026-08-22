# Mapped live denied-shell classification

This active checked-in scenario is the sole mapping for feature
`ai/temper#1082`, plan `ai/temper#1083`, and
`agent/pr-for-feature-1082`. It is additive: all historical mappings, including
`mapped-live-graph-consumption`, `mapped-live-graph-convergence`,
`mapped-live-ordinary-tool-convergence`, and
`mapped-live-decision-gap-recovery`, retain their original identities and
contracts.

## Live contract

Real Forgejo, the host Actions runner, standalone Temper, Jig, and a
deterministic current-root provider execute one minimal retry repair. In the
first model turn Jig emits a targeted graph read followed by a `bash` process
barrier. Effect-aware batching completes the graph call first. Dispatching the
later barrier in that same turn is then denied locally as
`DecisionAnchorMutation` and never reaches the registry or a process.

The durable shell start has no arguments and retains only the version-one
`excluded_never_executed_local_policy_denial` disposition with
`matching_discovery_segments = 0`. Its same-scope, same-call completion is
zero-duration `policy_denial` / `policy_precondition`. A transient process
canary is checked by the later successful validation call and independently by
the ephemeral harness validator, proving that the denied invocation never
executed.

The successful first result remains a valid decision anchor. In later model
turns, the engineer consumes its selected implementation through one refinement,
one caller/model trace, one typed current-root caller/model source, and one typed
current-root focused-test source. Only after that complete chain does the agent
read conventionally, apply the exact one-file repair, validate it, pass the host
submission gate and Actions, merge the PR, and close the source issue.

## Privacy-safe evidence

Checked-in declarations and aggregate run evidence retain only safe tool names,
closed disposition/category/reason fields, zero segments, counts, ordering,
typed correlation and lineage, current-root binding, non-execution, exact-head,
and gate facts. Raw commands, arguments, source, paths, provider output, prompts,
credentials, process-local fingerprints, host output, and diagnostic traces
remain ephemeral. Generated runtime evidence must not be committed to this
scenario corpus.

## Validation

From the exact assembled feature head, run:

```sh
cargo dev-scenario-check
cargo dev-scenario-run scenarios/mapped-live-denied-shell-classification
./.temper/pre-pr
cargo dev-scenario-validate-feature \
  --feature ai/temper#1082 \
  --landing-base origin/main \
  --source-branch agent/pr-for-feature-1082 \
  --pr <aggregate-pr-number> \
  --sha "$(git rev-parse HEAD)" \
  --output-dir target/focused-validation
```

Keep
`benchmarks/agent-sessions/codebase-memory-routing-repair/benchmark.toml`
unchanged. Candidate `2486d3d757982a6e10d763043463f60f2bdf394e` and its smoke
are rejected. Final aggregate validation must create, without selective reruns,
a fresh enabled smoke followed by fresh five-repetition enabled, disabled, and
forced-unavailable roots. Invoke the gate-bearing `temper-benchmark verify`
exactly once over those immutable roots. Correctness, typed relevance, complete
shell classification, unavailable fallback, at least 20% median improvement,
annotation, privacy, exact-commit, exact-patch, and independent host-validation
gates must all pass. Only privacy-reviewed aggregate evidence is eligible to be
committed.
