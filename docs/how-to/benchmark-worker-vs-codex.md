# Benchmark the Temper worker against Codex

Use `benchmarks/worker-codex/benchmark.py` to run a frozen coding task through
real Temper delivery and Codex, with independent host validation. Read the
[task and comparison contract](../../benchmarks/worker-codex/README.md) before
interpreting results. This procedure does not imply the performance target has
been achieved.

## Prepare the campaign

Use a host with Python 3.12+, Git, ripgrep (`rg`), Rust/Cargo, Codex, and the real
`codebase-memory-mcp` executable. The fixture runs Forgejo and its runner
locally; CI jobs execute directly on the host. The existing Temper deployment
is not used. Build the three required binaries before timing any arm:

```sh
cargo build -p temper --bin temper
cargo build -p temper-testing --bin temper-benchmark-stack
cargo build -p temper-benchmark-cli --bin temper-benchmark

benchmark_bins=$(mktemp -d /var/tmp/temper-benchmark-bins.XXXXXX)
cp target/debug/temper target/debug/temper-benchmark-stack \
  target/debug/temper-benchmark "$benchmark_bins/"
git rev-parse HEAD > "$benchmark_bins/source-revision.txt"
task_revision=$(git rev-parse HEAD)
```

Keep these binaries fixed for the campaign. Copying them outside `target/`
protects them from pre-PR cleanup. `--task-revision` selects committed task,
fixture, and oracle inputs using `git archive`; working-tree edits are not
inputs. Record the source revision used to build each candidate separately
when it differs from the task revision. A task/fixture/oracle change requires
a fresh baseline.

Codex's usual configuration must enable an MCP server named
`codebase-memory-mcp`. Preflight checks that setting, hashes the configuration
and binaries, and requires Codex's OpenAI provider with its default service
tier. Both arms use the executable selected by `--mcp-bin`, wrapped by the
transparent invocation recorder. Configured availability and observed graph
usage are separate evidence.

Pass Temper an existing **pi-format** OAuth file, commonly
`~/.pi/agent/auth.json`, with an `openai-codex` entry. Do not pass Codex's
`auth.json` as that file. Preflight compares its account ID with Codex's normal
OAuth account and rejects a mismatch. Credentials are read locally; do not
paste tokens into commands or benchmark files.

Optionally check provisioning and host CI without a coding issue or model job:

```sh
python3 benchmarks/worker-codex/stack_smoke.py \
  --temper-bin "$benchmark_bins/temper" \
  --fixture-bin "$benchmark_bins/temper-benchmark-stack" \
  --auth-file "$HOME/.pi/agent/auth.json"
```

Use the same host, toolchain, timeout, model settings, and provider build for
all arms. Avoid concurrent campaigns or heavy builds. The account-wide MCP
daemon may stay warm. Every arm gets a fresh checkout, target directory, and
project identity; native stacks use unique Forgejo owners while keeping the
repository basename `repo`. Do not pre-index the task or share a previous
solution. Do not set a different `CBM_CACHE_DIR` per arm: the installed provider
uses one account-wide daemon and can reject conflicting cache directories.

## Run exploratory and final pairs

The output directory must be new and outside the source repository. It is
private and contains prompts, source, patches, and diagnostic traces. Keep
artifacts out of Git and review them before sharing.

```sh
TEMPER_BENCHMARK_LIVE=1 python3 benchmarks/worker-codex/benchmark.py \
  --repository "$PWD" \
  --task-revision "$task_revision" \
  --temper-bin "$benchmark_bins/temper" \
  --fixture-bin "$benchmark_bins/temper-benchmark-stack" \
  --analyzer-bin "$benchmark_bins/temper-benchmark" \
  --auth-file "$HOME/.pi/agent/auth.json" \
  --codex-bin "$(command -v codex)" \
  --mcp-bin "$(command -v codebase-memory-mcp)" \
  --output /var/tmp/temper-worker-codex-exploratory-001 \
  --pairs 1 --timeout-seconds 1800
```

One pair is exploratory and cannot establish the target; the command exits
nonzero while `performance_target_met` is false. After investigation, freeze
both implementations and configuration, choose another new output directory,
and run at least `--pairs 5`. Order alternates Temper/Codex, then Codex/Temper.
Defaults are five pairs and 1,800 seconds per arm. Codex and MCP executable
flags are optional when those programs are on `PATH`.

The harness pins GPT-6 Astra and `xhigh`; Codex runs with
`--dangerously-bypass-approvals-and-sandbox`. Both contestants disable sub-agent
delegation so the comparison covers one coding agent per invocation. Temper receives one `code` +
`ready` issue, produces its PR, passes CI on that exact head, and mechanically
merges it. The source issue must close. Both final candidates undergo the
common gates and the frozen external oracle; Temper's validated checkout must
match the merged commit. Every scheduled failure or timeout stays in the
campaign. Keep exploratory reruns separate from final pairs.

Setup checks Temper's resolved standalone provider/model settings before filing
the issue. Native trace events must also identify the expected model on every
observed request; a different model or missing evidence invalidates the attempt.

## Read the evidence

`campaign.json` records the frozen inputs, configuration, binary hashes, and
order. Each `pairs/NNN/<arm>/trial.json` records an attempt. `comparison.json`
retains every trial and reports complete-campaign medians and the target result.
Missing attempts, correctness failures, or incomplete required tool evidence
prevent a successful comparison; failures are never dropped to improve a median.

Native coding time sums host `agent.finished.duration_ms` values for the
required in-process agent invocations, including retries and CI repair work.
That timer covers invocation setup, MCP preparation, execution, cleanup, and
trace acknowledgement. Daemon/Forgejo setup is separate. Codex time covers CLI
process start through exit, including startup. Native issue-to-merge time is a
separate delivery measure. Host oracle time is excluded from both coding times.

Inspect native `temper.log`, `delivery.json`, `attempt.json`, journal and
`analysis/` files; Codex retains `session/process.json`, timestamped public
`events.jsonl`, and its final message. `mcp.jsonl` records actual provider
`tools/call` invocations, including worker/bootstrap calls. Model-selected graph
calls are separate: native wrappers can deny an attempt before provider
execution. Repeated identical requests are observations, not automatically
wasted work. Codex tool durations use event receipt times. Summed tool durations
can overlap; they are not a serial wall-time breakdown.

Token totals are normalized: Codex input includes cached input; native input
is uncached input, so its total adds cache reads. Compare
`total_input_tokens`, `uncached_input_tokens`, and `cached_input_tokens`.
Unavailable model counts, retries, durations, or token evidence remain `null`;
a public Codex `turn.completed` is not a count of underlying model requests.

Requested model/effort and effective client configuration are distinct from
provider-reported identity, effort, or service tier. Public CLI events may omit
provider metadata. Treat it as unavailable; never manufacture confirmation
from the requested values. Report observed mismatches and the remaining
verification limits with any performance result.
