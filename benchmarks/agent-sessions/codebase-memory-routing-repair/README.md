# Controlled codebase-memory routing repair

Benchmark identity: `codebase-memory-routing-repair`.

This production-shaped Rust fixture hides a retry-affinity defect behind a
public delivery facade, a private routing implementation, decoy operational
helpers, and a focused regression test. The checked-in host gate runs the crate
tests and compares the final repository diff with `expected.patch`; changing or
weakening tests cannot make a different patch correct.

## Controlled conditions

Run the same manifest, task, fixture, provider/model profile, host policy, cache
annotation, repetitions, and correctness gates in each condition. Only the
runner-enforced codebase-memory availability changes:

| CLI condition | Availability |
| --- | --- |
| `codebase-memory-enabled` | Harness uses fixture graph evidence; live mode keeps the selected production codebase-memory profile. |
| `codebase-memory-disabled` | Removes codebase-memory from the invocation. |
| `codebase-memory-unavailable` | Starts the compatible fixture provider, then forces graph calls to fail systemically so the stable safe diagnostic and fallback path are observable. |

Example harness run (repeat for all three condition names):

```sh
cargo run -p temper-benchmark-cli -- run \
  --benchmark benchmarks/agent-sessions/codebase-memory-routing-repair/benchmark.toml \
  --mode harness \
  --condition codebase-memory-enabled \
  --agent-bin target/debug/temper-agent \
  --output-dir target/benchmark-harness/codebase-memory-routing-repair/enabled
```

The enabled fixture response names `src/route.rs`, its caller
`src/delivery.rs`, and `tests/alias_retry.rs`. The unavailable script includes
provider text that must never survive the wrapper's typed safe diagnostic.
Enabled and unavailable harness runs share the one graph attempt; the disabled
Jig starts at the same fallback step because that invocation exposes no graph
tool. Every subsequent discovery, patch, validation, submit, and final response
is identical. This static Jig adaptation proves condition plumbing only; live
comparisons keep one model/task and let the model react to the actual tool list.

## Predeclared material-improvement criterion

Before any live trials, material improvement is defined as **at least a 20%
reduction** in the enabled condition's median conventional discovery calls
before decisive selection, or in median discovery latency/token cost, relative
to the disabled condition. A conventional-discovery total is eligible only when
every shell command before selection has complete, parseable classification;
compound lists contribute one count per matching command segment. A quoted,
escaped, incomplete, or unsupported shell command lowers coverage and makes the
total ineligible rather than being counted as zero. Direct-tool component
medians remain diagnostic only in that case. Correctness and host validation
must pass in every included trial, provider/model identity and cache policy must
match, and the unavailable condition must show bounded fallback with no
immediate graph retry. Report sample counts and unavailable coverage with every
median.

Harness output proves condition plumbing, graph evidence classification, safe
bounded failure, exact diff validation, and deterministic correctness only. Jig
timing and token values are not live effectiveness evidence and must not be used
to claim the criterion was met.

The first privacy-reviewed production-provider trial report is
[`live-evidence-2026-08-06.md`](live-evidence-2026-08-06.md). It records failed
or unavailable gates and explicitly blocks deployment; it is not a passing
baseline.
