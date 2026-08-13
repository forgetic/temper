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
tokens, or terminal events. Scope ancestry is different: malformed or missing
ancestry remains an ingestion error rather than becoming unavailable summary
evidence. For an ingested trace, if any successful mutation lacks usable
model-turn identity, all three mutation-turn metrics are unavailable and the
summary emits `StructureEvidenceUnavailable` instead of reporting zero.

## Interpret graph discovery and decision relevance

Graph metrics are optional and appear when a `codebase_memory_*` call is
observed or a benchmark configures a decision-relevance rubric. A successful
RPC is **graph success**, not evidence that the result was useful. Decision
relevance uses this explicit rubric:

1. A benchmark declares an implementation, caller, or focused-test target and
   its exact provider-shaped producer target (`tool`, `target_kind`, and
   `target`).
2. The trusted wrapper must retain the corresponding complete typed correlation
   fingerprint for the successful producer. Generic provider summaries and
   result text are never matched.
3. A later declared graph/source consumer must carry its own exact typed
   correlation fingerprint in the same scope and after the producer. Direct
   successful `read`, `edit`, `write`, and patch mutations retain their exact
   target matching behavior.

Only that bounded typed correlation is a relevant result. A complete success
which does not satisfy a declared target is an irrelevant success. Missing,
malformed, or lossy producer/consumer correlation makes relevance unavailable;
it is never converted to zero. The declared target also identifies the decisive selection
independently of graph consumption. This keeps conventional discovery
comparable when graph results are irrelevant, graph calls fail, or a benchmark
condition disables graph calls entirely. Declare the rubric in the benchmark
manifest, for example:

```toml
discovery_command_prefixes = [["git", "grep"]]

[[graph_decision_targets]]
target = "repo/src/lib.rs"
kind = "implementation" # implementation, caller, or focused_test

[graph_decision_targets.producer]
tool = "search_graph"
target_kind = "graph_query"
target = "repair retry affinity"

[[graph_decision_targets.consumption]]
tool = "search_code"
target_kind = "pattern"
target = "retry_worker_topic"
```

Conventional discovery counts `grep`, `find`, and `read` calls before the first
successful selection whose complete arguments contain a declared target. Shell
classification uses a deliberately narrow command-list parser: unquoted and
unescaped words joined with `&&`, `||`, `;`, or newlines. Each parseable segment
matching a manifest `discovery_command_prefixes` argv prefix and containing an
argument beyond that prefix counts as discovery. Parseable non-matches count as
zero. Quoting, escaping, expansions, pipelines, redirects, grouping, globbing,
comments, missing discovery arguments, omitted command content, and other
ambiguous syntax remain unknown rather than being guessed.

`shell_command_classification_coverage` measures fully captured and parsed
shell *calls*, while `classified_shell_segments` counts matching segments in
those calls. Direct `grep`, `find`, and `read` components remain visible when a
shell call is unknown, but `total_calls` is omitted unless every shell call
before selection is classified. Aggregate and comparison artifacts likewise
omit the all-component total and the shell-segment distribution for incomplete
shell coverage; do not use component medians as a material-improvement gate. A
configured rubric still emits known zero graph counts in a graph-disabled
condition instead of dropping that trial.

Failure categories come from typed safe diagnostics. Every category except
`invalid_model_input` is systemic for the immediate-repeat metric: a repeat is
counted only when the next tool started in that scope is another graph tool.
`circuit_open` followed by grep/find/read is therefore visible as bounded
fallback, while an immediate graph retry remains visible as a repeat. Readiness
wait and graph execution duration use trusted numeric timing components and
carry per-call coverage. Older events without those components remain
unavailable or partial. Aggregate distributions include timing, status,
failure, repeat, and shell-classification values only for trials with complete
coverage; run reports still expose partial totals with their denominators.

Run reports place task correctness and host validation beside the outcome.
Task correctness is available only when the run failed/cancelled or a successful
direct run exercised host validation; a successful offline trace alone does not
prove correctness. Graph success, graph relevance, conventional discovery cost,
task correctness, and host validation are separate report values.

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
For the controlled routing fixture, the lane also proves that a fixture-only
stable upsert's normalized identity is separately confirmed with a targeted
ready status and canonical root before serving a cold targeted result and a
later warm result. Its enabled path is a five-call chain: targeted graph search,
symbol refinement, caller tracing, then two current-root source snippets before
the exact mutation. Relevance credits only the manifest-declared ordered
graph-to-graph, graph-to-source, and exact-source-selection consumption modes. A run
summary exposes only call ordering, safe tool names, declared targets/kinds, and
consumption modes; it excludes tool arguments, source, and provider results.
The diagnostic trace retains controlled source snippets and therefore remains a
restricted review artifact. The lane also proves that a parseable compound shell
command has complete classification coverage, and that one forced-unavailable
request is safely bounded before conventional fallback.
It proves neither live cache behavior nor an effectiveness or deployment gate;
the fixture Jigs prescribe tool calls and must not be interpreted as model
decisions. Run the repository lane with:

```sh
cargo dev-benchmark-harness
```

Live mode uses the normal Temper provider and model configuration. A manifest
with a `condition_profile` additionally requires one of
`--condition codebase-memory-enabled`, `--condition codebase-memory-disabled`,
or `--condition codebase-memory-unavailable`. The runner records that condition
in every run and aggregate artifact. Enabled live runs retain the selected
production codebase-memory profile, disabled runs remove only that toolset, and
unavailable runs retain its mode, role, index, and timeout policy while routing
graph requests to the benchmark's systemically failing fixture provider.

Live execution is an explicit operator action and refuses to start unless opted
in:

```sh
manifest=benchmarks/agent-sessions/codebase-memory-routing-repair/benchmark.toml
cargo build -p temper-agent-session --bin temper-agent
TEMPER_BENCHMARK_LIVE=1 \
  cargo run -p temper-benchmark-cli -- run \
  --benchmark "$manifest" \
  --mode live \
  --condition codebase-memory-enabled \
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
model output, tool arguments, and tool results. Bounded successful graph-result
text is retained only as `operator_transcript_v1` records in the local diagnostic
export; it is excluded from normalized durable activity and run/aggregate
summaries. Treat the complete export as operator-sensitive and keep it private.

## Repetitions and artifacts

`--repetitions N` overrides the manifest default. Every repetition receives a
fresh fixture copy and baseline commit. The artifact root contains
`aggregate.json` and `aggregate.md`; each `repetitions/NNN/` directory retains
the manifest and context snapshots, canonical diagnostic export (including any
operator-local transcript records), workspace result, validation and diff
evidence, and JSON/Markdown run summary. A manifest with an
`expected_patch` also snapshots it as `expected.patch` and records the host-owned
exact comparison in `validation.json`.

Use repeated live runs with enough repetitions to expose variance before drawing
behavioral or performance conclusions, then interpret min, p25, median, p75,
and max together. Aggregate and comparison artifacts retain trial counts and
sample counts for graph relevance, categorized failures, repeats, conventional
discovery, task correctness, host validation, and graph timing, so unavailable
trials remain visible rather than becoming zeroes. Timing is advisory and never
a pass/fail gate. Harness repetitions test deterministic plumbing and structure,
not model behavior, provider variance, or performance.

Artifact retention is caller-owned. Keep the exact candidate and baseline
artifact directories together with the Temper revision and relevant run notes.
Upload them only to access-controlled storage, set an explicit retention period,
and delete local traces when no longer needed. Review trace capture mode and
contents before sharing; do not publish source or model/tool content without the
same privacy review applied to the repository and work item.

## Compare a caller-owned baseline

The condition is part of benchmark identity metadata, so a comparison report
shows the base and head conditions while allowing pairwise comparison under the
same benchmark name and mode. For the checked-in routing benchmark, compare the
enabled aggregate to the disabled aggregate only after all trials pass fixture,
submit, host-command, and exact-patch validation. The benchmark README
predeclares its 20% median improvement criterion.

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
regardless of delta size. Review correctness and host validation first, then
graph success versus relevance, categorized graph failures/repeats,
conventional discovery before selection, and the existing structural changes:
turns, model attempts, retries/failures, tokens, tool and mutation counts,
validation invalidations, and diff size. Treat graph readiness/execution, model,
tool, TTFT, and wall-time deltas as advisory. Verify that mode, benchmark, provider/model identity, capture
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
