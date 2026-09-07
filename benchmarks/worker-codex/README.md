# Worker versus Codex delivery benchmark

This is the frozen task and correctness contract for comparing a real Temper
worker with Codex on the same Rust change. It extends the dependency-free
`delivery-policy` seed from
[`production-shaped-rust-change`](../agent-sessions/production-shaped-rust-change/README.md).
The seed has five implementation modules and a thin library facade. The task
adds configurable policy parsing, typed errors, normalization, evaluation,
reporting, and compatibility across those responsibilities.

The performance target is a Temper median coding-session duration no greater
than Codex's, with both passing the same behavioral acceptance checks. This
single task measures performance on this workload; it does not establish parity
on all coding work. The live harness is `benchmark.py`; follow
[Benchmark the worker against Codex](../../docs/how-to/benchmark-worker-vs-codex.md)
for prerequisites, commands, private artifacts, and metric interpretation.

## Inputs and correctness

| Input | Role |
| --- | --- |
| [`fixture/repo/`](fixture/repo/) | Identical starting checkout, AGENTS.md, and existing tests for both agents. |
| [`task.md`](task.md) | Exact common issue body / Codex task, including public API and observable semantics. |
| [`acceptance/`](acceptance/) | Host-owned external integration tests, never copied into the agent workspace. |

Freeze a Temper repository commit before a campaign. Record that commit and
the Git object IDs of `fixture/repo`, `task.md`, and `acceptance`, for example
`git rev-parse <revision>:benchmarks/worker-codex/fixture/repo`. Export the seed
from that revision into a new repository; do not copy a developer's working
tree. Record the resulting seed commit, task SHA-256, and any identical
provisioner-added delivery files. Any task, seed, or oracle change creates a new
benchmark version and requires a fresh baseline.

Both agents see the same task and seed tests. The host invokes the independent
oracle against each final candidate checkout:

```sh
python3 benchmarks/worker-codex/acceptance/run.py /path/to/candidate \
  --json-output target/worker-codex/acceptance.json
```

The runner creates a temporary standalone Cargo package with an absolute path
dependency on the candidate, copies the host tests into that package, and uses
its own target directory. It does not write tests into the candidate or accept
the candidate's test modifications as the oracle. All tests use the public API
specified in the task; no exact diff, implementation layout, or reference
solution is required. The current oracle contains 17 integration tests.

The unmodified seed must pass `cargo fmt --all -- --check` and
`cargo test --offline --quiet`, while failing the external oracle because the
new API is absent. Before landing an oracle change, prove that it accepts an
independently implemented temporary reference candidate kept outside the
tracked repository. Also check that broken behavior is rejected. Keep the
reference solution out of benchmark inputs and agent context.

Agents add their own focused tests and documentation. The host runs the common
format/test gates on each final candidate and reviews that existing tests were
not weakened, dependencies were not added, and README/CHANGELOG were updated.
The fixture's Temper pre-push gate assumes the writable workspace repository
is named `repo`; a provisioner using another name must rewrite only that gate's
`cwd` consistently before freezing the common seed. CI applies the same common
gates. Success for Temper additionally requires a passing PR to land and the
merged commit's tree to pass the independent oracle. A generated diff or an
opened PR alone is incomplete delivery.

## Real execution and fairness

Use a new isolated real Forgejo, host `forgejo-runner`, standalone Temper, and
real OpenAI model for the Temper arm. File exactly one ordinary code issue for
the basic delivery workflow to pick up, implement, validate, open as a PR, and
land. Record issue/PR/merge evidence. Do not use the existing Temper deployment
to implement the benchmark or replace real model execution with Jig. Existing
Jig scenarios remain separate workflow-validation evidence.

Run the Codex arm on a fresh equivalent checkout with
`codex --dangerously-bypass-approvals-and-sandbox`, passing exactly `task.md` as
the task. Use GPT-6 Astra with `xhigh` reasoning for both arms. Record the
requested settings, effective client configuration, and any available
provider-reported model identities and reasoning settings, plus Codex version
and Temper revision. Reject observed model/effort mismatches or configuration
drift. Public CLI events may omit provider metadata; keep it unavailable rather
than inferring confirmation from the request or a shared model nickname.

Disable sub-agent delegation in both contestants for this single-agent
comparison. Codex's default multi-agent feature is explicitly disabled; its
normal graph provider remains enabled. This keeps child models and unreported
child tool activity outside the comparison rather than hiding their work.

Use the same host, available CPU/memory, Rust toolchain, network path, task
deadline, and delivery validation. Build infrastructure binaries and warm Rust
toolchain/download caches before measurements. Use a cold workspace and cold
code graph for every arm: a new checkout, empty repository target directory,
fresh session state, and a new project identity in the provider cache. Record
whether the provider daemon is already running; both arms may reuse that daemon,
but neither may reuse a graph for its checkout. MCP startup and indexing belong
inside the measured coding session. Do not seed one arm with
another run's patch, transcript, graph, or solution. Do not run arms concurrently
or perform heavy unrelated builds during a measurement. Record unavoidable
provider-side prompt-cache usage; clients cannot force that cache cold.

Confirm Codex has the actual `codebase-memory-mcp` provider configured and its
tools discoverable before a campaign. Provide the same provider build and
graph-first AGENTS.md to Temper. A smoke check may confirm MCP availability on
a disposable unrelated repository; delete its state and do not index this
fixture before the timed session. Record availability separately from observed
usage: configuration alone does not prove an agent invoked graph tools. Report
provider outages and fallbacks as observed; do not silently relabel such a run
as a matched graph-enabled comparison.

## Session boundaries and records

Use monotonic clocks for durations and UTC timestamps for artifact correlation.
Infrastructure startup is a separately reported setup cost. For each coding
agent invocation, start before session bootstrap or MCP indexing. Codex starts
at CLI process spawn and ends at process exit. Standalone Temper's native agent
runs in process: use the worker's monotonic `agent.finished.duration_ms`, whose
timer covers the invocation through trace acknowledgement and cleanup, and
record this runtime choice. Its daemon startup belongs to setup time. Stop
after successful terminal submission (`submit_for_pr` for Temper; final task
completion for Codex). Include session startup, discovery, model time, tool execution,
format/tests, retries, and submission. A failing exit, missing submission, or
timeout is a failed session. Record the full elapsed duration, including the
timeout wait; never present a failed quick exit as a successful speedup.

The primary per-run coding time is the sum of **all** coding-agent session
durations needed for that issue, including CI repair sessions. Also report
first-session time and session count. Include all agent work required to reach
the final code result; do not move implementation or validation into an
unmeasured preparatory agent. Codex's final checkout is validated by the host
after the session ends, as is Temper's merged checkout; oracle runtime is
reported separately and excluded from both coding times.

Temper delivery wall time starts when the issue is filed and stops when its PR
is confirmed merged. Report polling/queueing, branch/PR publication, CI,
merge/convergence, and any remaining unclassified elapsed time separately from
coding time. Do not claim delivery wall time and Codex session time have the
same boundary. Fix and record an explicit per-arm timeout before the first
pair (recommended initial limit: 30 minutes). A timeout or failed acceptance
counts as a failed attempt, even if a follow-up diagnostic rerun succeeds.

Retain one machine-readable run record and restricted raw trace per attempt:

- campaign, pair, arm, order, repetitions, seed/task/oracle identities;
- binary versions, effective model/effort, toolchain, resource/cache conditions;
- session starts/stops, total coding time, delivery time and phase breakdown;
- completion status, common gates, independent acceptance, issue/PR/merge IDs;
- model turns, input/output/reasoning/cache tokens when available, billed cost
  when available, and failed/retried model requests;
- tools attempted, tools executed, errors, execution duration, and counts by
  canonical tool name; unavailable metrics remain `null`, never zero;
- MCP attempts and actual provider invocations, names, durations, index calls,
  failed calls, fallback, repeated identical discovery, and focused snippet
  versus broad file-read behavior.

Normalize tool namespaces for comparison, retaining the original name. Count
each function call once, even when several calls share a model turn. Separately
count shell commands that call MCP and actual provider invocations so wrappers
do not hide work or cause double counting. Classify repeated discovery from
tool arguments plus source revision; revisiting a graph after a mutation is not
automatically wasted work. Fewer calls are beneficial only when correctness and
useful discovery remain intact. Exclude credentials from retained artifacts;
keep raw source/tool traces out of checked-in benchmark data.

Codex public JSONL events can report command/tool completions and cumulative
usage without exposing every model request. A public `turn.completed` event
is not proof of one model call. Preserve those events with host monotonic
receipt timestamps, leave unavailable model-call counts `null`, and identify
any tool timing inferred from receipt intervals rather than execution clocks.

## Paired experiment and decision

Capture an initial baseline before performance changes. Tune against separate
exploratory runs, retaining failures. Freeze both implementations and their
configuration before final measurement, then perform five final paired runs:

| Pair | First arm | Second arm |
| --- | --- | --- |
| 1 | Temper | Codex |
| 2 | Codex | Temper |
| 3 | Temper | Codex |
| 4 | Codex | Temper |
| 5 | Temper | Codex |

Keep every scheduled attempt in the results. Report success counts over five,
each paired duration and ratio, medians, minimum/maximum, and tool/MCP totals
with their variability. A campaign establishes the target only if both arms
pass all five attempts, Temper's median total coding time is no greater than
Codex's, and common gates plus independent acceptance pass with complete
Temper merge evidence. Failed attempts remain in the table with duration and
reason; do not compute a success-only median and claim victory. If either arm
fails, show successful-run timing descriptively and mark the target unmet.

Five pairs provide a practical initial comparison, not a precise confidence
bound. Discuss provider noise and outliers. If more runs are needed, declare
the additional complete pairs before running them; retain all earlier pairs.
Publish aggregate evidence and artifact references outside the frozen inputs;
do not check credentials, live databases, generated traces, or mutable timing
baselines into this directory.
