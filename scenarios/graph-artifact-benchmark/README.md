# Graph artifact benchmark delivery

This active scenario maps feature `ai/temper#1281`, plan `ai/temper#1278`, and
branch `agent/pr-for-feature-1281`. Run it with:

```sh
cargo dev-scenario-run scenarios/graph-artifact-benchmark
```

The scenario uses real Forgejo, a host Actions runner, standalone Temper, and
Jig fake-LLM architect/engineer roles. It requires Python 3, `zstd`, and `timeout`
on the host. Its ordinary delivery workflow is copied from `basic-delivery`.

The seed adapter compiles the exact nine Python modules from
`benchmarks/codebase-memory-artifacts/` into the scenario binary, installs them
only when the seed explicitly declares `.temper-artifact-harness`, and records
their SHA-256 identities. There is no second benchmark implementation under the
scenario directory.

The architect reads the current report consumer, its product tests, and the
repository purpose before returning the ready-code specification. This exercises
the ordinary read-only triage tool loop.

The engineer executes the real harness's explicit `fixture` command, delivers
its JSON report, and fixes a report consumer that trusts an unqualified decision
field. Product tests require synthetic reports to defer even if their decision
claims a pilot. They also require complete accounting, correctness, and disabled
default persistence before a native report can render a bounded pilot candidate.
The host CI regenerates the fixture report, checks equality with the delivered
report, and runs these product tests before mechanical merge and source closure.

After convergence, the adapter fetches the actual merged default branch and
checks the merge SHA, harness bytes, and unchanged validation/CI fixtures. It
reruns the same fixture, checks the delivered report again, and runs the product
tests. The required assertion retains twelve closed checkpoints from that
verification in its stdout. No raw provider response, source bytes, private
runtime path, credential, prompt, or diagnostic trace is copied into those facts.

The fixture exercises all 25 matched main samples and four missing/corrupt/
truncated/incompatible artifact cases using real SQLite/zstd files and the
harness's shared manifest, current-root/source/call/coverage checks, quarantine,
and report projection. An imported foreign producer root must be refused before
recovery; recovered samples cannot be counted as clean imports. The report
keeps coverage best effort, synthetic timings unmeasured, the decision `defer`,
and default persistence disabled.

These are scenario contract and delivery observations. Native provider timings,
resource usage, adoption thresholds, and distribution decisions are established
by the separate frozen experiment and native report documented in
[`docs/explanation/codebase-memory-artifacts/`](../../docs/explanation/codebase-memory-artifacts/).
The scenario does not establish a speedup, a remote artifact download cost, a
Git custom-merge-driver result, or compatibility with a future native build.
