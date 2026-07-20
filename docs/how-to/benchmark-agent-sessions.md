# Benchmark agent sessions

Use `temper-benchmark` to summarize a coding-agent trace, run the direct agent
boundary repeatedly, or compare retained summaries. The command measures this
boundary:

```text
prepared workspace + WorkspaceContext -> temper-agent -> WorkspaceResult
```

It does not replace workflow validation. See the
[benchmark corpus](../../benchmarks/README.md) for the checked-in input and the
[scenario corpus](../../scenarios/README.md) for whole-workflow validation.

## Analyze an existing trace

Analyze a journal run directory, its `events.jsonl`, or a versioned export:

```sh
cargo run -p temper-benchmark-cli -- analyze \
  --trace /path/to/trace-or-events.jsonl \
  --output-dir /tmp/temper-benchmark/analysis
```

The output contains `run.json`, `run.md`, and a canonical
`trace.export.jsonl`. The report derives model calls and attempts, TTFT
coverage, tokens, tool use and timing, retries, terminal status, and observable
mutation/validation ordering.

Historical traces cannot be enriched with facts they never recorded. Final diff
statistics, `WorkspaceResult`, and host validation evidence are unavailable for
an offline trace. Metadata-only capture can omit command arguments needed to
identify validation boundaries, and older or partial traces can omit timings,
tokens, or terminal events. In particular, traces without usable tool-call
scope and model-turn identity report mutation-turn batching metrics unavailable
with a diagnostic, not as zero. The summary marks unavailable values and emits
observability diagnostics instead of inventing evidence.

## Interpret model-turn batching

Model-turn batching is multiple cohesive tool calls emitted by one model
response. The structural metrics count distinct mutation turns, turns with
exactly one successful mutation, and the maximum successful mutations in any
one turn. They describe response structure; they do not imply concurrent tool
execution. Independent reads may execute concurrently, while `write`, `edit`,
process, network, and other barrier calls remain serialized.

## Choose harness or live mode

Harness mode runs the real agent process and tools against deterministic Jig
responses. It proves runner plumbing, trace extraction, artifact generation,
submit gates, serialized barrier handling, and structural metrics. It is safe
for ordinary CI, but its model latency, TTFT, tokens, and wall time are **not
representative LLM performance**. Deterministic Jig timing is not a CI gate.
Run the repository lane with:

```sh
cargo dev-benchmark-harness
```

Live mode uses the normal Temper provider and model configuration. It is an
explicit operator action and refuses to start unless opted in:

```sh
manifest=benchmarks/agent-sessions/cross-cutting-rust-change/benchmark.toml
cargo build -p temper-agent-session --bin temper-agent
TEMPER_BENCHMARK_LIVE=1 \
  cargo run -p temper-benchmark-cli -- run \
  --benchmark "$manifest" \
  --mode live \
  --agent-bin target/debug/temper-agent \
  --config /path/to/config.toml \
  --secrets /path/to/credentials.toml \
  --pool engineers \
  --repetitions 5 \
  --output-dir /secure/artifacts/candidate
```

Do not put provider credentials in a manifest, command transcript, checked-in
file, or CI lane. Harness mode needs none. Live mode redacts resolved credential
values from artifacts, but diagnostic traces can still contain source, prompts,
model output, tool arguments, and tool results.

## Repetitions and artifacts

`--repetitions N` overrides the manifest default. Every repetition receives a
fresh fixture copy and baseline commit. The artifact root contains
`aggregate.json` and `aggregate.md`; each `repetitions/NNN/` directory retains
the manifest and context snapshots, canonical trace, workspace result,
validation and diff evidence, and JSON/Markdown run summary.

Use repeated live runs with enough repetitions to expose variance before drawing
behavioral or performance conclusions, then interpret min, p25, median, p75,
and max together. Timing is advisory and never a pass/fail gate. Harness
repetitions test deterministic plumbing and structure, not model behavior,
provider variance, or performance.

Artifact retention is caller-owned. Keep the exact candidate and baseline
artifact directories together with the Temper revision and relevant run notes.
Upload them only to access-controlled storage, set an explicit retention period,
and delete local traces when no longer needed. Review trace capture mode and
contents before sharing; do not publish source or model/tool content without the
same privacy review applied to the repository and work item.

## Compare a caller-owned baseline

Temper does not ship a live timing baseline, database, or dashboard. Choose and
retain a suitable artifact directory for the provider, model, host class, and
benchmark you are evaluating. Compare it without rerunning either side:

```sh
cargo run -p temper-benchmark-cli -- compare \
  --base /secure/artifacts/baseline \
  --head /secure/artifacts/candidate \
  --output-dir /secure/artifacts/comparison
```

The comparison is report-only and exits successfully for a valid comparison,
regardless of delta size. Review structural changes first: turns, model
attempts, retries/failures, tokens, tool and mutation counts, validation
invalidations, and diff size. Treat model, tool, TTFT, and wall-time deltas as
advisory. Verify that mode, benchmark, provider/model identity, capture
coverage, host metadata, and repetition counts are comparable before drawing
conclusions.

## Benchmark or scenario?

Use `temper-benchmark` when the question is about a direct coding-agent session:
trace structure, model/tool usage, repeated live performance, or comparison with
a caller-owned baseline.

Use `temper-scenario` when the question is whether a workflow converges through
real Forge state, worker/daemon orchestration, CI, and expected artifacts.
Scenarios are validation inputs and their manifests do not carry benchmark
repetition, baseline, provider-performance, or timing-gate semantics.
