# Worker versus Codex benchmark validation

This scenario maps feature and plan `ai/temper#1285` on
`agent/pr-for-feature-1285`. It combines real delivery evidence with independent
deterministic tests of the benchmark harness. It does not run GPT-6 Astra,
measure either coding agent, or establish a performance improvement.

The inherited basic-delivery workflow starts real Forgejo, a host
`forgejo-runner`, standalone Temper, and Jig architect/engineer roles. The
engineer reads the report contract, writes one `benchmark-contract.json`, runs
its validator and negative-case tests, and submits the report. Host CI repeats
those report checks before the PR merges and its source issue closes. The
report explicitly says that model performance and real provider usage were not
measured; it carries no fabricated durations, token counts, or parity claim.

The required after-convergence assertion then loads the actual benchmark
sources and tests directly from `benchmarks/worker-codex/` in the checked-out
Temper repository. It runs `test_metrics.py`, `test_mcp_metrics.py`,
`test_mcp_proxy.py`, and `test_campaign.py`, covering comparator rejection/success, request correlation,
stdio forwarding, failed-attempt retention, configuration drift, and complete
patch capture. The proxy tests use a deterministic toy stdio server; they do
not contact a live codebase-memory provider or model. The scenario contains no
copy of the benchmark implementation and adds no scenario-runner behavior.

Before importing those modules, the assertion requires the live evidence's
checkout SHA to equal the current feature head, and compares all imported
benchmark Python sources/tests, task/README, and scenario inputs with their tracked bytes at
that head. It also requires the real topology, passing manifest assertions,
completed host CI, one actual merge SHA, a closed source issue, and observed
Jig turns. Every required test suite must execute at least one test and pass
without skips. A closed checkpoint report is retained in the script assertion
artifact directory and stdout, without model/provider timing claims.

CI validates the delivered declaration; the host assertion runs the actual
benchmark accounting/proxy/campaign tests. These are distinct checks, both
required by this mapped scenario. Real-model performance remains the separate
frozen five-pair campaign described in
[`benchmarks/worker-codex/`](../../benchmarks/worker-codex/README.md).

After integrating and committing the benchmark modules, run from the assembled
feature head:

```sh
cargo dev-scenario-check
cargo dev-scenario-run scenarios/worker-codex-benchmark
cargo dev-scenario-validate-feature \
  --feature ai/temper#1285 \
  --landing-base origin/main \
  --source-branch agent/pr-for-feature-1285 \
  --pr <aggregate-pr-number> \
  --sha "$(git rev-parse HEAD)" \
  --output-dir target/focused-validation
```

The scenario requires Python 3.11 or newer and Git on the host. Commit the
scenario and benchmark sources before live validation: dirty or untracked
inputs intentionally fail the source-identity assertion. Generated logs and
checkpoint reports belong in validation artifacts, outside this scenario.

The hook locates its checkout from its own script directory. Its context
contains the already-evaluated manifest assertions; the CLI appends the hook's
own outcome only after the command returns.
