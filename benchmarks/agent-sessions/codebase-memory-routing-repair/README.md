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

The enabled harness starts with a confirmed missing stable project, performs a
blocking stable upsert in an isolated fixture-state file outside the repository.
That upsert returns only the fixture's normalized provider identity
`temper-benchmark-codebase-memory-routing-repair` and `indexed` status; it does
not acknowledge a root. A second targeted `index_status` request for that
normalized identity returns `ready` plus the canonical `root_path`. The
requested opaque stable key remains in fixture state for comparison, but cannot
serve status or graph reads.

The enabled Jig begins with two useful independent roots for routing and focused
behavioral evidence. A later parallel batch refines and traces `worker_slot`
while reading the root-bound regression test from the confirmed current root.
Before the final caller source completes the decision chain, two duplicate
refinements reach the provider as consecutive non-progressing batches and
exhaust normal exploration. The machine then reports exactly `caller` as
missing, reserves the four-call targeted current-root recovery allowance, and
denies a broad architecture request and another duplicate refinement locally
without consuming that allowance. One targeted current-root caller source read
reaches the provider and completes the chain. Completion then freezes graph
exploration: a broad architecture call and two targeted post-decision attempts
are denied locally while conventional shell and source reads remain available
for the exact patch.

The fixture numbers actual provider invocations in its private responses. The
harness therefore bounds the enabled run to sixteen model turns, distinguishes
thirteen model graph attempts from eight provider invocations, proves the two
useful roots, their descendants, both admitted duplicates, and the targeted
recovery reached the provider, and checks that the two recovery denials and all
three post-completion denials did not. Both recovery denials retain the exact
missing-kind guidance and the unchanged allowance of four. All eight successful
results retain complete typed relevance and current-root lineage, so
deterministic relevance remains above the frozen 50% aggregate gate without
treating any denied attempt as useful.

The manifest permits only declared provider-shaped typed producer and consumer
targets, followed by graph-to-graph, graph-to-source, and final exact
source-selection consumption links. Provider summaries are not relevance
markers. The run summary retains ordered call IDs, safe tool names, declared
target/kind, and consumption mode (`graph`, `source`, or `selection`); it never
retains tool arguments, source text, or provider results as decision evidence.
Diagnostic traces do contain the controlled source snippets, so they remain
local review artifacts and must not be published without the same
source/privacy review required for live traces.

The controlled Jigs also run the same closed ordinary-tool recovery sequence in
every condition after source selection: one `bash` execution reports a typed
failure, the identical invocation is redirected locally without execution, and
a corrected invocation succeeds. Run summaries count the execution failure and
single circuit redirect by closed category/reason, with complete coverage; they
never retain the command, raw error, or the process-local invocation
fingerprint. The redirect remains bounded to one event, and disabled and
unavailable profiles must expose identical ordinary metrics.

Every condition uses the same parseable compound shell fallback before the
first exact selection: `cd repo && rg ...`. The manifest counts its one `rg`
segment only when full shell coverage is present; it does not infer a count
from an unparseable command. The disabled Jig begins at that
fallback because no graph tools are registered. The unavailable Jig makes one
systemically failing graph request, then takes the same fallback without an
immediate graph retry. Disabled and unavailable runs do not synthesize graph
consumption evidence. The unavailable provider response includes secret-shaped
text that must never survive the wrapper's typed safe diagnostic or retained
trace.

These scripted cold/warm, relevance, shell-classification, and unavailable
checks prove deterministic harness plumbing and bounded wrapper behavior only.
They do not simulate model choice, production cache behavior, or deployment
approval; live comparisons keep one model/task and must satisfy the frozen
criterion independently.

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

## Exact-head acceptance verification

The manifest declares the frozen provider/model, cache annotation, smoke and
matrix sizes, typed decision evidence, aggregate relevance, privacy, bounded
unavailable fallback, and conventional-discovery improvement gates. After a
fresh smoke and three fresh five-run live condition roots have been produced,
evaluate them with the gate-bearing command (the separate `compare` command
remains report-only):

```sh
cargo run -p temper-benchmark-cli -- verify \
  --benchmark benchmarks/agent-sessions/codebase-memory-routing-repair/benchmark.toml \
  --candidate-commit "$CANDIDATE_COMMIT" \
  --smoke "$SMOKE_ROOT" --enabled "$ENABLED_ROOT" \
  --disabled "$DISABLED_ROOT" --unavailable "$UNAVAILABLE_ROOT" \
  --output-dir "$ACCEPTANCE_ROOT"
```

The command writes only a typed, privacy-safe `acceptance.json` gate summary
and exits nonzero when any gate fails. It accepts runner artifact roots, not
Markdown reports, so historical evidence cannot authorize a current candidate.
Global privacy fragments reject fixture secrets and authorization-shaped values
anywhere in the evidence roots. Aggregate-only fragments additionally reject
raw fixture commands, source, host-local paths, and diagnostic result text while
allowing those controlled values to remain in restricted per-run diagnostics.

Run and freeze a candidate in this order: one enabled smoke; five enabled
matrix repetitions; five disabled repetitions; five forced-unavailable
repetitions; then one `verify` invocation over those four immutable roots. Do
not selectively rerun or reorder a failed condition. A candidate, provider,
model, cache annotation, baseline, or manifest change invalidates every root and
restarts the sequence at the smoke. Missing relevance, shell classification,
or comparison samples remain unavailable and fail closed rather than becoming
zero.

## Privacy-reviewed live evidence

The feature #1009 exact-final-tree smoke and subsequent frozen 5-by-3 matrix are
recorded in
[`live-evidence-2026-08-13.md`](live-evidence-2026-08-13.md). Only aggregate
counts and gate outcomes are checked in; diagnostic exports remain private.

The earlier production-provider trial report is
[`live-evidence-2026-08-06.md`](live-evidence-2026-08-06.md). It records failed
or unavailable gates and explicitly blocks deployment; it is not a passing
baseline.
